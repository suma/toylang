# TODO - Interpreter Improvements

## 完了済み ✅

> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-08-16
- **PTR-READ-ENUM: enum の byte layout を関数境界の flatten に統一** — enum に**サイズが 3 つ**あった (`__builtin_sizeof` の `1 + max(payload)` / tree-walker の「手元の variant 依存」/ 関数境界 flatten の `u64 tag + 全 variant 連結`)。3 番目に寄せ、`Vec<Option<T>>` と `enum List { Cons(i64, ptr), Nil }` が 3 バックエンドで動くように。`Vec` の `elem_size` は最初に push した要素から採るので、variant 依存のサイズは stride がバラつくバグでもあった。
- **`__builtin_ptr_read` が user 定義型名の注釈を受けるように** — `val n: Node = __builtin_ptr_read(p, off)` が通る。user 型は注釈に `TypeDecl::Identifier` で来るので型検査のヒント許容リストから落ち、lowering も名前 → `StructId` を解決していなかった (generic 実体化中の `T` だけ見ていた)。**E0013 が勧める raw ptr の逃げ道が「書けるが読み出せない」状態だったのを解消**。enum 名は per-leaf read model に layout が無いので専用の診断で拒否。
- **RECURSIVE-TYPES step 1: 再帰型を診断で拒否 (E0013)** — 間接化なしで自分を含む struct / enum は有限な layout を持てないのに、型検査を素通りして lowering (`instantiate_struct` / `instantiate_enum` は memo 化の**前**にメンバを lower する) で host stack を食い潰し、**exit 134 / メッセージ無し**で abort していた。型引数経由 (`Vec<Tree>`) も同じ経路で落ちるので同じ検査に含む。`compiler_lower` 側にも in-progress guard を入れ、abort を通常の lowering error に落とす二段構え。`Box<T>` (BOX-T) は別途。
- **parser: 改行前 `(` は method call に継続しない** — `b.v\n(x as i64)` が `b.v(...)` と parse され、ユーザが書いていない呼び出しについて型エラーが出ていた。
- **CONCRETE-IMPL-Phase-2c (generic-wildcard 完遂)** — 型チェッカの method registry を `Vec<MethodSpec>` 化し、3 層 (型検査 / interpreter / compiler) の dispatch を exact → wildcard → lone-spec に統一。concrete impl が generic impl を override できる。
- **STR-INTERP-COMPOUND-EXTEND-ENUM** — enum 値の補間を AOT / compiler JIT で (tag brif chain + variant ごとの concat)。interpreter JIT が tag を出力していたバグも修正。
- **MATCH-LET-RHS-PAYLOAD-INFER 完遂** — method call / field-access レシーバの method call を scrutinee に持つ val/var 右辺 match。

### 2026-08-15
- **`--check` が満たせないサイズの確保で Rust panic していたのを修正** — 確保失敗は全バックエンドで null ポインタに統一。
- **lexer エラーを診断として報告 (E0012)** — 従来は `Ok(None)` に潰れて無関係な行の型エラーに化けていた。`--explain` / `--diagnostics=json` も対応。
- **struct field / enum payload の初期化形を 4 形に揃えた (AOT/JIT)** — literal / 既存束縛 / call / associated fn / method call を、struct 型・tuple 型の両方で。`String` フィールドを持つ struct が AOT で構築できるようになった。
- **compound 値の読み出し側を AOT/JIT で** — `val inner: Inner = o.i` / `println(o.i)` / `"{o.i}"`。新しい名前は同じ leaf locals を引き受ける (copy しない)。
- **receiver を読まない method の AOT panic を修正** — `self` を一度も書かないプログラムでは symbol が intern されず receiver が parameter 列から落ちていた。
- **非 ASCII のソースリテラルの化けを修正** — UTF-8 scalar 単位で写す。`\xHH` の `HH >= 0x80` は lex error に。
- **テストスイートを 5.15s → 4.4s に (-15%、CPU 90s → 82s)** — rayon の oversubscription / example sweep の二重パース / shard 4 → 12 / discard budget。**この suite は CPU 律速で下限にほぼ到達**していることが分かった (詳細は TEST-PERF)。

### 2026-08-13
- **`str == str` を内容比較に統一** — interpreter (tree-walker) だけが内容比較で、他 4 実装は runtime handle の整数比較だった (**型は通るが答えが違う** divergence)。`InstKind::StrEq` + 4 ランタイム実装。
- **`Display` trait — 型が自分の見せ方を決める** — `core/std/display.t`。型検査器が `println(v)` → `println(v.to_str())` に**引数を**書き換えるので、バックエンドは通常の method 呼び出ししか見ない。ディスパッチは method の有無 (`==` → `eq` と同じ流儀)。前提として `__builtin_str_from_bytes` を新設。
- **str リテラルの数え差 (interpreter だけ +1 確保) を解消** — IR VM が str リテラルを counter-free に実体化 (コンパイル系の `.rodata` と同じ扱い)。`--all-backends --profile=mem` がリテラル込みで完全一致。
- **Drop 内で `&mut self` フィールドを free すると use-after-free するバグを修正** — **struct 束縛を return すると、そのローカルにも scope-exit の Drop が発火**して戻り値が dangling になっていた (AOT segfault / IR VM panic)。関数本体ブロックの tail 束縛だけ DropTarget から除去。
- **ポインタ演算 builtin (`__builtin_ptr_offset`)** — interior pointer。offset ベースの free-list / region allocator を書くためのプリミティブ。interpreter 側 JIT は reject。
- **allocator レジストリ: `layout_report` を `--profile=mem` に自動で載せる** — `__builtin_record_allocator_layout` + `impl Drop` フック。レポートに `allocator layouts` 節 (全バックエンド byte-identical)。
- **エンジン fallback 時の副作用重複実行を修正** — IR VM が出力後に diverge すると tree-walker の再実行で `println` が 2 回出ていた。成功時のみ captured 出力を replay。

### 2026-08-10
- **MEMORY-PROFILING M0〜M5 完了** — 設計は [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md)。M0 用語の固定 (`MemoryStats`、全項目を「プログラムが要求した内容」で定義) → M1 `--profile=mem` / `TOY_PROFILE_MEM=1` の 4 バックエンド byte-identical レポート → M2 サイト帰属 + リーク検出 (**ソース位置そのものを site ID にする** `(line << 32) | column`) → M3 `trait Alloc::layout_report` (断片化。**`Arena` / `FixedBuffer` は領域を管理していないので「報告しない」**が正しく、実際に領域を持つ `SlotRegion` を stdlib に追加) → M4 `--profile-format=json` + カウンタ builtin 6 種 → M5 `__builtin_ptr_offset`。**設計上の要点**: 断片化はランタイムでなく `trait Alloc` の責務に置いた (ユーザ定義 allocator も同じレポートに乗る)、プロファイル無効時に **0 を返すのは答えないより悪い**ので `InstKind::MemStatEnable` をカウンタを読むプログラムにだけ挿す。
- **暗黙の impl 型パラメータ (`impl Container<T>`)** — `docs/language.md` が明記していたのに**未実装**で、リファレンス自身の例が型検査を通らなかった。parser に「型名 → 宣言された generic params」の表を持たせ、宣言に載っている名前だけを parameter に昇格。副産物として `GENERIC-ENUM-HOF-USER` も解消。**制約**: 暗黙形は struct/enum 宣言が impl より前にある必要がある。
- **stdlib の generic enum HOF を全 backend で** (`Option::map` / `Result::map` / `map_err` / `unwrap_or_else`) — enum receiver の generic method target 解決 / 関数型パラメータ内にしか現れない method-only generic param の推論 / その monomorphisation の 3 つのギャップ。
- **MATCH-LET-RHS-PAYLOAD-INFER (第一段)** — 全 arm が payload 束縛の match を val/var 右辺に。enum の同定は pattern 名でなく **scrutinee** から (generic enum は instantiation ごとに別物)。
- **AOT-MATCH-SCRUTINEE-EXPAND** — enum を返す**関数呼び出し**を match scrutinee に許可 (`while val Some(x) = func(i)`)。`resolve_call_target` を通すので closure 束縛と generic 単相化も同じ経路。
- **INCREMENTAL-COMPILATION Phase 5** — 実測して再スコープ。warm AOT 67ms の 70% は `cc` で、per-module IR が狙えるのは ~4ms だけと判明。代わりに codegen の非決定性 (lowering / codegen の HashMap 反復 2 箇所) を潰し、全ミスしていた link cache を機能させて **67ms → 19ms**。**dependency graph / cascade invalidation は現在の粒度では不要**と実測で確認 — cross-module 派生物をキャッシュして初めて要る。
- **LLM-LOOP-FIX: 式の span を full extent に** — postfix / unary / literal 系が「自分を名付けるトークン」しか指しておらず、field access と `if` / `match` / `with` は**次の文の先頭**を指していた (P2 が潰したはずの「無関係なコードを自信満々に指す」形)。`Call` は callee 名のまま (P2 の意図)。
- **LLM-LOOP-FIX: 壊れた `as` キャスト提案 / 引数型不一致 E0010 → E0001 / JIT 列の空洞化** — machine-applicable 提案が `f(a) as i64` を生成して元のエラーを解決していなかった。`compiler` の `interpreter` 依存が `default-features = false` で `jit` feature を落としており、cross-backend suite の JIT 列が単体ビルドで tree-walker の複製になっていた (`interpreter::jit_available()` で assert)。
- **LLM-LOOP P7: 補助 CLI** — 型ホール `val x: _ = expr` (専用コード E0011)、`--api <file>` (シグネチャ一覧)、`--explain [<CODE>]`。`must_use` と `--watch` は不採用。
- **DEV-LOOP D6: `--all-backends` + stdin 入力** — `compiler f.t --all-backends` が 3 バックエンドを 1 コマンドで実行し一致なら 1 行。入力名 `-` で stdin。
- **DEV-LOOP D5: `CODE_MAP.md` 新設 + `CLAUDE.md` から履歴を分離** — 常時ロードされる 48 KB の 30% が changelog だった。履歴は `FEATURE_NOTES.md` へ**移動** (削除ではない — 一部はここにしか無かった)。48 KB → 33 KB。
- **LLM-LOOP P6-3: u64 アンダーフローの trap** — `0u64 - 1u64` の wrap を全バックエンドで trap 化。guard は lowering に置いて AOT / IR VM / compiler JIT を 1 箇所で賄う。**スコープは符号なし減算のみ** (加算・乗算の overflow と narrow unsigned は wrap のまま)。

### 2026-08-09
- **LLM-LOOP P0〜P6** — 設計は [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md)。P0 bare-name 呼び出しのレキシカルスコープ修正 (3 バックエンド 4 箇所)、P1 診断の一括報告 (文単位のエラー回復 + パースエラー全件報告)、P2 Span 化と全診断への location 強制、P3 構造化診断 (`--diagnostics=json`) と修正提案、P4 `test` ブロック + `--test`、P5 契約ベースプロパティテスト (`--check`)、P6-1 panic の位置と backtrace、P6-2 契約違反時の値キャプチャ。
- **DEV-LOOP D1〜D4, D7** — 設計は [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md)。D1 テスト出力を failure-first に (1641 行 → 7 行)、D2 黙って効いていない設定の修正と flaky test 解消、D3 設定構造体の `#[non_exhaustive]` 化、D4 `CLAUDE.md` Commands 節を検証済み内容に更新、D7 `example_consistency.rs` で全 example を 3 バックエンド掃引。
- **D7 sweep が検出した潜在バグ 5 件** — AOT の代入式が値を produce していなかった / f64 `%` の明示エラーが cranelift assertion に化けていた / struct field 欠落がコンパイラクラッシュになっていた / JIT の `main` キャッシュが `File` のポインタ同一性をキーにしており別プログラムが前のコードを実行しえた (`File::id` 導入)。
- **`else if` の拒否 + パースエラーの握り潰し解消** — `else if` は「未サポートだが無害」ではなく 3 通りに壊れていた (偶然動く / 実行時 null エラー / ファイル残りを黙って破棄)。根本原因は `parse_program` が収集済みパースエラーを捨てていたこと。
- **`var` の型注釈チェック追加** — `var w: bool = 1u64` が通り `println(w)` が `1` を出していた (`val` は正しく拒否)。`val` / `var` が別経路なのが原因。

### 2026-05-31
- **Interpreter IR VM 化 Phase 0〜4** — `compiler_ir` / `compiler_lower` を crate として切り出し (循環依存の解消)、`interpreter/src/ir_vm/` に VM を新設。scalar → compound / heap / string → closure / dyn Trait / reference + `&mut self` writeback / contract と段階的に拡張し、4-way consistency (interpreter / AOT / JIT / IR VM) に統合。**IR VM を既定エンジン化**、非対応プログラムは silent fallback。
- **意味論の決着 3 件** (git だけでは追いにくいので残す):
  - **`self: Self` は by-value、mutation は伝播しない** — `docs/language.md` が canonical で、AOT / IR VM が仕様準拠、tree-walker の `Rc<RefCell>` 共有が違反。伝播させたいなら `&mut self`。
  - **`val` の再代入は compile-time error** — tree-walker は runtime 拒否、compiler は素通しで divergence していた。型検査で拒否するのが canonical。
  - **`__builtin_sizeof` は type-based** — 値ベースではなく型から算出するのが canonical。
- **負数 array index を compiler 側にも実装** — tree-walker のみ対応していた `a[-1]` を lowering に移し 3 バックエンド統一。**定数 index のみ** (runtime 負数 index は未対応)。
- **clippy fixes across workspace** — auto-fixable lint 適用 + 手動修正。以後 **clippy 無警告が既定状態**。

### 2026-05-23
- **core/std 並列パース (Phase 1)** — module integration を parallel pre-parse + sequential integrate の 2 段に分割、二重パースを除去。
- **Cranelift 関数 codegen 並列化 (Phase 2)**。
- **Incremental compilation Full AST cache (Phase 4)** — 一度 revert された後の再挑戦で成功。**真因は `string-interner` crate の serde 非対称バグ** (serialize が `usize`、deserialize が `u32`) で、0.20 への upgrade + bincode varint encoding で解決。`FULL_AST_CACHE_SCHEMA_VERSION` の bump 忘れは**プログラムが黙って壊れる**ので注意。

### 2026-05-19
- **`dyn Trait` Phase 2 (A5-P2-MVP-A〜F)** — AOT の動的ディスパッチを empty struct → scalar field → nested struct + `&mut dyn` writeback → struct return → tuple / enum return → `&mut self` + compound return の順に landing。fat pointer ABI と vtable の設計は [`DYN_TRAIT_AOT.md`](DYN_TRAIT_AOT.md)。
- **Bare-name imported function calls (Phase 1)** — `enforce_import_namespace` を削除し import した `pub fn` を bare name で呼べるように。
- **`Program` → `File` rename (Phase 4)** と後方互換 alias の削除。

### 2026-05-18
- **`dyn Trait` Phase 1 (A5-P1)** — interpreter で `&dyn Trait` を landing。
- **Trait 多重 bound `<T: A + B>` (A2)** — call-site の bound check は AND、method dispatch は OR。
- **Trait デフォルトメソッド本体 (A1)** — AST mutation pre-pass で impl に synthesize。3 バックエンド動作。

### 2026-05-17
- **`?` (Try) early-return operator** — 型チェッカが `match` に in-place rewrite するのでバックエンドは Try を観測しない。
- **`loop {}` + comparison chain** — parser-level desugar。
- **GENERIC-ENUM-MATCH-HOF** — `Option::map` / `Result::map` / `map_err` を stdlib に追加。

### 2026-05-10
- **DEBUG-BUILTINS Phase A+B+C** — `__builtin_source_file/line/column` と `assert_eq` / `assert_ne` / `dbg` の parser-level macro 群。

### 2026-05-09
- **IF-VAL (`if val` / `while val`)** — pure parser desugar。`let` ではなく `val` に揃えた。

### 2026-05-08
- **LABEL (labelled break / continue)** — `@label:` 形式 (Rust 風 `'label:` は char literal と衝突するため)。3 バックエンド。
- **OP-OVERLOAD 完全コレクション** — 同型 struct ペアの全 binary + unary operator を user method に dispatch。`&&` / `||` と chain は対象外。
- **STRING-NOMINAL + STR-INTERP-COMPOUND** — `String` を `Vec<u8>` alias から**独立した nominal struct** に変更。
- **AOT lower 系の汎用拡張** — `__builtin_sizeof` の compound 対応、`__builtin_ptr_write/read` の compound 対応。

### 2026-05-07
- **ITER-PROTOCOL-TRAIT** — generic trait 宣言 `trait Foo<T, U>` と `impl Foo<i64> for Counter`。
- **ITER-PROTOCOL-AOT** — `for x in EXPR` を AOT でも動作。bare identifier の場合は synthetic temporary を skip して `&mut self` writeback を効かせる。
- **STR-INTERP Phase 2 (AOT + cranelift JIT)** と **STR-INTERP-INTERP-JIT** — interpreter 側 JIT では `str` を **function 境界 (param / return) で禁止** (Object lifecycle 整合性のため)。

### 2026-05-06
- **STR-INTERP Phase 1 (interpreter)** — `"hello {name}"` を lexer + parser-level desugar で。
- **ITER-PROTOCOL Phase 1 (interpreter + JIT)** — structural (duck-typed) で、generic trait `Iterator<T>` 自体は使わない。

### 2026-05-05
- **NUM-LIT-SEPARATORS** — `1_000_000u64` 等。lexer-only。
- **CLOSURES Phase 1〜8** — frontend / 型検査 / interpreter / AOT (direct / indirect / capturing / narrow int / return / struct field 格納)、`fn (T) -> R` 関数型構文。
- **DOCS-2026-05-05** / **NUM-W-JIT** / **ZERO-MEMCOPY-FIX** (`size==0` の libc parity) / **TYPE-ALIAS 周辺整備**。

### 2026-05-04
- **エスケープシーケンス** — `\u{HEX}` / `\xHH` / char literal `'a'` (u32)。
- **TYPE-ALIAS / GENERIC-TYPE-ALIAS** — parse 時即時展開 + generic alias。
- **GENERIC-RAII** — user struct の `impl Drop` を scope-bound auto-call (interpreter + AOT)。
- **ALLOCATOR Phase 5** — `trait Drop` + temporary-form の auto-cleanup。
- **REF-Stage-2** — `&T` / `&mut T` の borrow + writeback、escape rule の構文 reject。
- **121-Phase-B-rest** — arena / fixed_buffer の native runtime、`with` body 早期 exit の cleanup。
- **TEST-PERF-lazy-core** / **STRING stdlib** / **CONCRETE-IMPL Phase 1〜2b**。
- **NUM-W (Phase 1〜6 + AOT + AOT-pack + signed-hash)** — 狭い数値型 (u8/u16/u32/i8/i16/i32) の interpreter + AOT 完全対応。
- **DICT 系まとめ** — `Dict::new()` の AssociatedFunctionCall 経路、per-monomorph generic substitution。

### 2026-05-03
- **VEC-collection** — user-space `Vec<T>` (`core/std/collections/vec.t`)。
- **STR-LEN-O1 / STR-PTR-LEN** — AOT で `__builtin_str_len` を O(1) 化、`.rodata` layout を `[bytes][NUL][u64 len LE]` に。
- **121-Phase-B-min / Phase-A** — Allocator builtin 群、heap / pointer builtins。
- **MUT-SELF-Stage-1** — `&mut self` receiver。
- **96残-前半** — match の deep exhaustiveness check。

### 2026-05-02 以前 (大きめのマイルストーン)
- **#183 コンパイラ MVP** — IR / cranelift-object backend で実行ファイル生成。struct / tuple / enum / generic / trait / DbC / allocator builtin / 配列 / cast / f64 / panic-assert / print まで網羅。
- **per-module function namespacing (#193 / #202)** — IR + 型検査 + 実行時の関数テーブル分離。
- **コア・モジュール auto-load (#193)** — `<repo>/core/` を起動時に再帰的にロード。
- **Extension trait 全 backend 対応 (#191、Step A〜F)** — primitive 型への trait impl。
- **Math externalisation (#190、Phase 1〜4)** — `extern fn` 経由の f64 math intrinsic。
- **Option / Result stdlib (#203)** — `core/std/option.t` / `result.t` と AOT enum receiver dispatch。
- **Value/Reference 分離 Phase 1〜5** — fibonacci -8% / for_loop -12%。
- **panic / assert / DbC (#166〜#175)** — `requires` / `ensures` 全 backend 対応。
- **言語仕様拡充 (#161〜#165)** — f64、`%`、複合代入、タプル JIT、ネスト分解、match arm guard。
- **#184 Trait + impl** / **#170 top-level const** / **#169 `docs/language.md` 新設**。

## 未実装 📋

> 完了した項目はここに残さない (完了済み節と二重になる)。優先度は
> ★ = あると良い / ★★ = 効果が見えている / ★★★ = ロードマップ級。

### バックエンドのカバレッジ

- **159. JIT の generic struct 対応** ★★ — `struct_layouts` を type-args 別に持つ refactor。踏むと `JIT: skipped (... see #159)` が出るので診断から辿れる (`jit_skip_reason_for_generic_struct` で wording を pin)。
- **160. タプルの JIT 対応 (ネスト)** ★ — `((a,b),c)` と tuple-of-struct。`ParamTy::Tuple(Vec<ScalarTy>)` を tree 構造にする 100+ 箇所の refactor。inline tuple literal を call 引数に渡す件も残り。
- **JIT-enum-1 (residual)** ★ — ネストした generic enum payload (`Option<Option<T>>`)、enum 型の struct field、payload に struct / tuple を持つ enum。
- **NUM-W-AOT-pack Phase 3** ★ — compound element 配列の tighter layout (`[PackedRgba; N]` が 4 バイト相当のところ 32 バイト消費)。メモリ効率のみで機能差はない。
- **195b. `extern fn` の monomorph 化** ★ — generic extern は現状 interpreter の type-erased registry でのみ動く。JIT / AOT には mangled symbol の emit と Rust 側実装の登録が要る。実需要なし。
- **185残. 3+ part qualified call** ★ — `std::math::abs(x)`。現状は `import std.math` 経由のみ (parser が last 名だけを採る)。auto-load があるので実害は限定的。
- **121-Phase-B-rest-leftover** ★ — `AllocatorBinding::Generic/Local/Ambient` の lower 配線 (perf のみ、観察可能な振る舞い変化なし)、`__builtin_default_allocator()` の戻り型を `u64` にして生比較を許すかの API 判断。
- **REF-Stage-2 (residual)** ★ — compound `&mut T` の真の pointer-passing、`&T` compound の RefScalar 経路活用。どちらも copy 削減で機能差はない。
- **183. コンパイラ MVP の残** — compound-returning method の expression position 制約。個別項目は上記に分解済み。

### 標準ライブラリ・実行環境 (STDLIB-RUNTIME)

> 2026-08-16 に「言語機能として何が残っているか」を実際に叩いて洗い出した結果。
> 言語のコアはほぼ揃っており、**実プログラムを書けなくしているのはこの節**。

- **STDLIB-ITER: `Vec` / `Dict` / `String` に `iter()`** ★★★ — `for x in v { ... }` が
  **動かない** (`[E0010] Method 'next' not found for struct 'Vec'`)。iterator
  protocol も `trait Iterator<T>` も実装済みで、**標準コレクションが誰も
  `next()` を持っていない**だけ。stdlib のみで閉じ、バックエンド変更が
  要らないので費用対効果が最も高い。
- **STDLIB-ITER-ADAPT: iterator アダプタ** ★★ — `map` / `filter` / `enumerate` /
  `collect` / `zip` 相当。generic enum 側 (`Option::map` 等) はあるのに
  コレクション側が空。**前提**: STDLIB-ITER。`fn map<U>(...)` を struct に
  持たせる形は generic method-only param の推論が既に通っている。
- **RUNTIME-IO: 最小 I/O セット** ★★★ — `print` / `println` 以外の I/O が
  **ひとつも無い** (stdin / ファイル / `argv` / 環境変数 / 時刻 / 乱数)。
  つまり**入力を受け取るプログラムが 1 本も書けない**。経路は `extern fn` +
  [`FFI_PLAN.md`](FFI_PLAN.md)。`extern fn` の generic monomorph が JIT / AOT
  未対応 (#195b) なので、非 generic な最小セット (`read_line() -> str` /
  `args() -> Vec<str>` / `read_file(path) -> Result<String, _>`) から入る。
  **API 判断が要る点**: 失敗をどう返すか (`Result` 一択にするか)、
  auto-load される core module に置くか明示 import にするか。
- **STDLIB-ORD: 順序比較 trait とソート** ★ — `trait Hash` はあるが `Ord` /
  `PartialOrd` 相当が無く、`Vec` のソートも無い。`<` の演算子オーバーロード
  (`lt` / `le` / `gt` / `ge`) が既にあるので、規約をそちらに寄せるか
  trait を切るかの設計判断から。

### 型システム (NEW-TYPE-SYSTEM)

- **BOX-T: `Box<T>`** ★★★ — heap 間接の first-class 化。**再帰型を書ける
  ようにする道**であり (拒否は landing 済み、E0013)、**A5-P4
  (`Box<dyn Trait>`) の前提**でもある。**設計は 2026-08-16 に決定**:
  - **軸 1 (cycle の切り方) = 一般則**。「型引数が `ptr` フィールド越しに
    しか現れない struct」を遅延辺として扱う。`Box` を特権化しないので
    **`struct Tree { kids: Vec<Tree> }` も一緒に通る** (今 E0013 で
    落ちている最も自然な形)。`Box` だけを既知名として特別扱いする案と、
    `indirect` マーカーを言語に足す案は不採用。
  - **軸 2 (所有権) = 現行の scope-bound Drop に乗せる**。move semantics
    も refcount も入れない。
  - **実測した前提** (すべて 3 バックエンド一致で確認):
    - `val b = a` は**共有**で値コピーではない (`b.x = 42` が `a.x` に出る)。
      以前ここに「値コピーなので二重解放になる」と書いていたが**誤り**。
    - `Drop` は**値ごとに 1 回**、構築した束縛のスコープ退出で鳴る。
      alias / 引数渡し / struct field 格納のいずれでも 1 回。
    - **user 空間の `Box` 相当は既に書けて動く** (generic struct +
      `impl<T> Drop` + heap builtin)。足りないのは cycle の切り方だけ。
  - **軸 2 の帰結 (既知の限界として明記すること)**: `Box` が構築した
    束縛より長生きする場所 (`Vec` / 別 struct の field / 外側スコープ)
    に格納されると、構築スコープの退出で free され dangling になる。
    実測では interpreter が値を返し **AOT バイナリは SIGTRAP (exit 133)**。
    規約は「`Box` は構築したスコープが所有する」。再帰構造の所有
    (ノードが自分の子 `Box` を持つ) はこの規約に収まる。将来 move
    semantics を入れれば消える性質なので、`Drop` を後から足す方向 =
    互換、という前提は保たれる。
  - **実装フェーズ**:
    1. `instantiate_struct` / `instantiate_enum` の two-phase interning
       (id を先に予約してからメンバを埋める)。今は memo 挿入がメンバ
       lower の後なので、`Box<List>` の layout が有限でも
       `(Box, [Enum(List)])` の型引数解決で `List` に再入する。
    2. 遅延辺の規則を 2 箇所に入れる — `check_recursive_types` の辺集合と
       lowering の型引数解決。**片方だけだと「型は通るが lower で落ちる」**。
    3. stdlib `core/std/box.t` (`new` / `get` / `set` / `as_ptr` +
       `impl<T> Drop`)。
    4. 3 バックエンド一致テスト (再帰 enum / 再帰 struct / `Vec<Tree>`) と
       `--profile=mem` の一致。
    5. 限界を `docs/language.md` と `--explain E0013` に書き、E0013 の
       メッセージから `Box` へ誘導する。
  - **残る宿題**: 長い連結リストの `Drop` は実行時に深い再帰になるので、
    tree-walker の関数再帰 abort (既知の不具合) を踏む。
  - **手書きの逃げ道は 3 つとも動く** (2026-08-16): arena + index
    (`example/linked_list_arena.t`)、struct + raw ptr
    (`example/linked_list_ptr.t`)、enum payload に raw ptr
    (`consistency.rs::an_enum_through_a_ptr_round_trips`)。
- **NEWTYPE: tuple struct / newtype (`struct Meters(i64)`)** ★ — parse エラー。
  単位型・ID 型のラップが「1 フィールドの struct + 冗長な field 名」になる。
  parser + 位置指定のフィールドアクセス (`m.0`) が要る。
- **Trait 拡張** ★★★ (大規模、ロードマップ)
  - **A3: trait inheritance (`trait B: A`)** — 中。super trait 経由で `A` の method を `B` impl からも要求。
  - **A4: associated types (`trait Iterator { type Item }`)** — 中〜大。
  - **A5-P3-interp: interpreter 側 JIT の `dyn Trait`** ★ — `ScalarTy::from_type_decl` が `TypeDecl::Dyn` で `None` を返し silent fallback。correctness 問題はなく、compiler 側 JIT が実用的な高速化を担うので優先度は低い。
  - **A5-P4: `Box<dyn Trait>`** — owned trait object + `Vec<Box<dyn Trait>>`。**前提**: `Box<T>` 自体が未実装。
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`、generic trait の default body 内での `T` 参照。
- **Trait-bounded generic API** ★★ — `fn first<I: Iterator<i64>>(iter: I)` の bound check。`<T: Trait>` は struct で動くが generic trait の bound は未強制。
- **`From` / `Into`** ★★ — `val s: String = "hi".into()`。`?` の cross-error 変換にも要る。
- **`must_use` / unused-Result 警告** ★★ — `?` の補完。**警告の emit 経路が無い**ので (`Severity::Warning` は型としては存在するが未使用)、そこから作る必要がある。
- **slice 型 `&[T]`** ★ — 配列 borrow を first-class に。中〜大。
- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣の候補 (NEW-FEATURES、未着手)

- **OP-OVERLOAD-CHAIN** — `a + b + c` の chained position。現状は let-rhs のみ。binary struct literal operand も対象外。
- **`??` (null-coalesce)** ★ — `opt ?? default` で `unwrap_or` の糖衣。
- **raw / multi-line string literal** ★ — `r"\path"` / `"""..."""`。lexer 拡張のみ。
- **PATTERN-EXTEND: or / 範囲 / `@` バインディングパターン** ★★ — いずれも
  parse エラー。`1i64 | 2i64 => ...` / `0i64..5i64 => ...` / `n @ 2i64 => n`。
  parser + **網羅性・到達性チェックの拡張**が要る (or は「複数 variant を
  1 arm が覆う」、範囲は整数の被覆判定)。バックエンドは既存の arm に
  展開できるので手を入れずに済むはず。タプル / ガード / ネストパターンと
  `val (a, b) = ...` の分解は既に動く。
- **STR-INTERP-FMT: 補間の format spec** ★★ — `"{x:.2}"` が parse エラーで、
  **f64 の桁数指定手段が言語に無い** (`println(3.14159f64)` の出方を
  ユーザが選べない)。lexer の `{...}` 切り出しに spec 部を足し、
  `__builtin_to_string` 系に幅 / 精度 / 基数を渡す形。`Display` の
  `to_str(&self)` は引数を取らない規約なので、**user 型に spec を渡すか
  (`fn to_str(&self, spec: str)`) は API 判断**。
- **STRUCT-UPDATE: struct update 構文 (`P { x: 5i64, ..a }`)** ★ — parse エラー。
  「1 フィールドだけ差し替えた copy」が全フィールド列挙になる。

### インクリメンタルコンパイル

- **INCREMENTAL-COMPILATION の残** — Phase 1〜5 は完了 (設計と実測は [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md))。**残るのは「16 個の core module のキャッシュ読み込み + 統合に毎回 ~10ms」** (lowering の 2.5 倍)。per-module IR compilation + IR linker は warm 19ms のうち ~4ms しか狙えないので保留 — 着手するなら、大きめの実プログラムで lowering が支配的になることを**再測定してから**。

### テスト・ドキュメント

- **TEST-PERF** — ワークスペース全体で **~4.4s** (2026-08-15 実測、20 コア、`cargo nextest run`)。**この suite は wall ではなく CPU 律速**: 合計 ~82s CPU / 20 コア ≈ 4.2s が下限で、実測がほぼそこにある。したがって「並列度を上げる」策はもう効かない (`-j 24` で 1%、shard を増やしても critical path が下限を割らない)。**残る削り代は CPU そのもの**:
  - **core module のロードが suite CPU の約半分** ★★★ — stdlib を auto-load した trivial プログラム 1 実行 ~24ms の内訳 (2026-08-15 実測、debug ビルド):

    | 区間 | 時間 | 割合 |
    |---|---|---|
    | `integrate_modules` (preparse ~4ms + 逐次 integrate ~7.8ms) | 11.3ms | 48% |
    | `execute_entry` の準備 (registry 構築自体は 0.4ms、残りは context 構築) | 5.2ms | 22% |
    | impl block の型検査 (stdlib の 40 block) | 2.5ms | 10% |
    | その他の型検査 (alias 解決 0.5 / trait default 0.11 / setup 0.18 / stmt 0.16) | 1.1ms | 5% |
    | user プログラムのパース | 0.25ms | 1% |
    | プロセス起動 | ~2-3ms | ~10% |

    **測って分かった否定的な結果を 2 つ記録しておく**: (1) **free function の body は既に user 分しか検査していない** (`take(user_func_count)`) ので「stdlib 本体を型検査しない」で削れるのは impl block の 2.5ms だけ。しかも**型検査器は body を書き換える** (`?` の desugar、`Display` の `to_str` 挿入) ので、stdlib の body を検査しないと**書き換え前の AST がバックエンドに流れる** — 今の stdlib は `?` も補間も使っていないので通ってしまい、使った日に壊れる罠になる。(2) `remap_symbol` の memo 化 (module symbol → main symbol を Vec でキャッシュ) は**効果ゼロ**だった。integrate の時間は文字列ハッシュではなく AST を pool に複製する作業そのもの。
    したがって残る手は (a) stdlib を使わないテストを `test_program_no_core` に寄せる (実測: `test_program` を no-core にすると interpreter の 879 テスト中 **797 が通り**、その binary は 2.3s → 1.3s。ただし stdlib 同居時の回帰を見なくなる = coverage を実際に落とす)、(b) 型検査済み core をプロセス内で使い回す (INCREMENTAL-COMPILATION 側の仕事。`File` が `Rc` を持つので素朴な memo 化はできない — 別スレッドから clone すると refcount が壊れる)。
  - **プロセス起動が ~4.4ms × 1784 ≈ 8s CPU (約 10%)** ★ — nextest は 1 テスト 1 プロセス。テストを機能別に束ねれば減るが、失敗の切り分けと引き換え。
  - `serial_test` (`oop_tests.rs`) の並列化 ★。
- **65. frontend リファクタリング** — (a)〜(g) は完了。残: doc コメント拡充、プロパティベーステスト追加。
- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。
- **26. ドキュメント整備** — 残: API リファレンス、advanced topics。
- **DOC-DRIFT (2026-08-16 実測)** ★ — `docs/language.md` の *Generics and bounds*
  が「`<T: SomeBound>` は parse されるが型検査器は bound を強制しない」と
  書いているが、**実際は強制されている** (`fn g<T: Z>(x: T)` に `g(1u64)` は
  `[E0010] ... bound violation` で拒否される)。`CLAUDE.md` 側の記述が正しい。
  同節の *Known limitations* も enum 補間 / MATCH-LET-RHS-PAYLOAD-INFER を
  未対応として残しているが、どちらも 2026-08-16 に解消済み。

## 検討中の機能

* FFI / 拡張ライブラリ — 設計は [`FFI_PLAN.md`](FFI_PLAN.md) (未着手)
* モジュール拡張 — バージョニング、リモートパッケージ
* 言語内からの AST 取得・操作
* LSP 対応 — 補完 / go-to-definition / hover / 診断 / フォーマット。frontend の AST・型チェッカ・`SourceLocation` を再利用できる。ただし**エージェントは LSP より CLI クエリを使いやすい**ので、LLM ループの観点では `--api` / 型ホール (P7 で landing 済み) の方が先だった


## 現状の把握

> 言語仕様は [`../docs/language.md`](../docs/language.md) が正本、日常的に踏む
> 要点は [`CLAUDE.md`](../CLAUDE.md)、実装の場所は
> [`CODE_MAP.md`](CODE_MAP.md) にある。
> **機能一覧をここに再掲しない** — 3 重管理になり、実際に食い違った
> (この節は `String` を「`Vec<u8>` alias」と書き続けていたが、
> 2026-05-08 に nominal struct へ変わっていた)。

### テスト状況
- 合計 **1839 テスト** (100% 成功、2026-08-16 時点)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は interpreter / JIT / AOT の 3 経路一致を保証する。
- ワークスペース全体で ~5s。`compiler/build.rs` が `toylang_rt.c` を pre-build し、リンク結果は `TOY_LINK_CACHE_DIR` で content-addressed にキャッシュされる (キャッシュが効くにはコード生成が決定的である必要がある — `compiler/tests/reproducible_build.rs` が pin)。

### 既知の不具合

- **tree-walker の関数再帰が ~200 フレームで abort する (2026-08-16 実測)** —
  IR VM (既定エンジン) はヒープにフレームを積むので 200000 段でも通るが、
  IR VM が lower を諦めて **tree-walker に fallback したプログラムだけ**
  host stack を使い、debug ビルドで ~200 段で `fatal runtime error:
  stack overflow` (exit 134) になる。`max_recursion_depth: 1000`
  (`evaluation/mod.rs`) のガードは**式評価の入れ子しか数えていない**ので
  先に host stack が尽きる。再帰型 (E0013) と違い、これは診断化ではなく
  ガードの数え方を関数フレームに変える話。
- **型不一致診断が `Identifier(SymbolU32 { value: 40 })` と Debug 表記を
  漏らす** — `TypeCheckErrorKind::TypeMismatch` の `Display` が `{:?}`
  なので、解決前の user 型名が生の symbol id で出る。`source_name` /
  `type_name_for_error` に寄せる。

### パーサーの既知制限事項
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- 関数のネスト定義 (`fn` の中の `fn`) は不可 — closure (`fn(x: T) -> R { ... }`) を使う。
- デフォルト引数 / 名前付き引数は不可 (`f(a: u64, b: u64 = 1u64)` / `f(a: 1u64)`)。導入予定も無い。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省いているのはこのため)。
- 3-part qualified call (`std::math::abs(x)`) は parser が **last 名だけを採る**。名前が一意なら結果的に解決するが、意図した経路ではない (`#185残`)。
