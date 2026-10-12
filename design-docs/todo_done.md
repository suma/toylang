# 完了済みの記録 (2026-09-19 以前) と測定の記録

> [`todo.md`](todo.md) から移した。**常時は読まない**ファイル。
> todo.md の完了済み節は直近だけを持ち、それより前の 1 行サマリと、
> 2026-08 時点のテスト・ビルド時間の測定記録はここに置く。経緯・測定値の
> 詳細は git log のコミットメッセージにある。
> 他の文書が「todo.md の XXX」と名指す完了項目は、ここを探すこと。

## 完了済み (2026-09-19 以前)

### 2026-09-19

- **MATCH-PAYLOAD-COPY — `match` の腕は payload を名指すのであって 複製しない**

### 2026-09-18

- **STR-PTR-UNCOUNTED — `str::as_ptr` が確保カウンタを動かさなくなった**

### 2026-09-17
- **RANGE-FOR — `for i in r` が範囲値で動き、範囲値が 3 レーンに 入った**
- **MODULE-EXPR-REMAP — モジュールの body に配列の添字を書くと integration が落ちていた**

### 2026-09-11
- **TEST-PARALLEL P0〜P3 — `toy test` が並列に走る**

### 2026-09-06
- **TRY-COMPOUND / COMPOUND-BLOCK-RHS / COMPOUND-GENERIC-INSTANCE — `?` が compound を運べるようになった**

### 2026-09-10
- **REF-REBORROW の残り 2 経路 — module 呼び出しと generic 推論**

### 2026-09-05
- **toylang 側のリファクタリング (stdlib / poc)**
- **REF-REBORROW — `&mut` 引数の転送に借用を書かなくてよくなった**
- **BY-VALUE-SELF-ALIAS — 値渡しの引数は callee 自身のコピーになった**
- **CODE-SIZE-SELF-ABI — 演算子オーバーロードと method の参照引数も番地で渡す**
- **CODE-SIZE-SELF-ABI S3b — 幅の広いローカル束縛を stack slot に常駐**
- **CODE-SIZE-SELF-ABI S3a — `&T` / `&mut T` の compound 引数もポインタで渡す**
- **CODE-SIZE-SELF-ABI S1+S2 — 幅の広い by-reference receiver をポインタで渡す**
- **CODE-SIZE-WB-PRUNE — `&mut self` が書かない leaf を返さなくした**
- **リファクタリング (frontend / compiler / interpreter)**
- **MODULE-IMPORTS D1 — `import a.b as h` が効くようになった**
- **`toy version`** — toy / compiler / interpreter / stdlib の version + git revision + **パス**を 1 行ずつ。
- **TEST-TOOL T3 — `core/std/testing.t`**
- **TEST-TOOL T4 — `test "..." panics { }`**
- **TEST-TOOL T5 — golden ファイルと `toy test --bless`**
- **TEST-TOOL T1 — compiled レーンで `test` が走るようになった**
- **TEST-TOOL T2 — `main` の無いファイルの AOT が無関係なエラーを出す件**
- **`--test` が IR VM で全テストを黙って pass させていた**
- **`toy clean`** — `build/{debug,release}/` を消す。
- **`toy` の出力レイアウトを決めた** — `build/{debug,release}/` で profile を分ける (`--release` は契約を消すので**別のプログラム**であり、 同じパスだとディスク上…
- **`toy build` / `check` / `test` の既定を AOT にした**
- **BARE-NAME-COLLISION — 後の module root が bare 名を勝ち取るようにした**
- **`ambiguous` の診断が用途で分かれた** — bare 呼び出しに 「Two modules cannot share a file name — rename one of them」と 言っていたが、`std::bas…
- **TEST-TOOL T0 — module の中に `test` ブロックを書けるようにした**
- **BUILD-TOOL B2 — `toy test`**
- **BUILD-TOOL B4 — bare 名の衝突を事前に報告する**
- **BARE-NAME-COLLISION の実例が 3 件見つかった**
- **BUILD-TOOL B0 — `--core-modules` を繰り返し指定できるようにした**
- **BUILD-TOOL B1/B3 — `toy` コマンドを追加した**
- **`Bits` / `Checked` も借用にした**
- **`Ord` / `Hash` と container の読み取りメソッドを借用にした**
- **STRING-NO-DROP — `String` が自分のバッファを解放するようにした**
- **IR VM の `value not defined` が場所を言うようになった**
- **STDLIB-FS-HANDLE — `fs::File` (開いたファイル) を入れた**
- **`Result<(), E>` を `val` に束縛すると lowering が panic した**
- **METHOD-ARG-UNCHECKED — method 呼び出しの引数を型検査するようにした**
- **METHOD-MUT-PARAM-REBORROW — method が自分の `&mut` パラメータを 再借用できるようになった**
- **MEMORY-ACCESS M3 — `Span<T>` の範囲演算**
- **MEMORY-ACCESS M2 — stdlib の読み出し 130 箇所を `::<T>` 形へ移行**
- **MEMORY-ACCESS M0 — `mem_move` / `mem_set` が全バックエンドで動く ようになった**
- **MEMORY-ACCESS M1 — `__builtin_ptr_read::<T>(p, off)`**
- **IRVM-BOOL-STRIDE — IR VM が bool 配列を 8 バイト間隔で書いていた**

### 2026-09-04
- **JSON-RESULT-READER — json の reader が `Result` を返すようになった (`json::parse` が設計どおりの入口になった)**
- **MODULE-TRY-REMAP — stdlib の body に `?` を書くと integration が 落ちていた**
- **TRY-STDLIB-ALIAS — user が `Result` / `Option` を影にすると stdlib の `?` / `??` が壊れる**
- **STDLIB-CRYPTO C0/C1 — SHA-256 / SHA-224 (`core/std/crypto/`)**
- **WIDE-RETURN — 戻り値の leaf が返却レジスタを超える compound を compiled lane が返せるようになった**
- **VEC-CONTRACTS #1〜#3 — `Vec<T>` の境界を `requires` にした**

### 2026-09-03
- **STDLIB-NUMERIC 完了 (N0〜N6) — 残っていた `shuffle` / `checked_pow` / `clamp_f32`**
- **UNBOUNDED-GENERIC-PARAM — bound の無い `<T>` が「宣言されていない」 扱いだった**
- **QUALIFIED-GENERIC-CALL-SCOPE — module 修飾の generic 呼び出しが 呼び出し側の束縛を消していた**
- **QUALIFIED-GENERIC-CALL-LOWER — 同じ呼び出しが lowering でも 落ちていた**
- **TYPECHECK-BODY-KEY — 同名の module 関数の body が丸ごと未検査だった**
- **CODEC-SIMD — hex / base64 の 4 カーネルを SIMD 化**
- **SIMD-INTRINSIC-4 — `__simd_bitmask` / `__simd_swizzle` / `__simd_bitcast` / `__simd_shuffle` (intrinsic 13 → 17、**
- **STDLIB-SERIALIZE S1/S3/S4/S5 — JSON (`core/std/json.t`)**
- **STDLIB-SERIALIZE S0/S2 — hex / base64 (`core/std/hex.t` / `base64.t` / `codec.t`)**
- **STDLIB-LOG — レベル付きログ (`core/std/log.t`)**
- **STDLIB-FS-PATH — path 操作とファイルシステム (`core/std/path.t` / `fs.t`)**
- **STDLIB-TIME — 単調時計・sleep・CPU 時間・日付 (`core/std/time.t`)**
- **GENERIC-SCALAR-REF 解消 — `&T` が primitive でも通る**
- **RETURN-LOCAL-DROP (tree-walker) 解消 + `Vec` / `Box` の `Clone`**
- **PTR-READ-ASSIGN 解消 + `str` の比較演算子 (STDLIB-ORD)**
- **STDLIB-TRAIT-BASE B5 — 戻り位置からの型引数推論と `Default`。 これで B0〜B5 すべて landing**
- **STDLIB-TRAIT-BASE B0 / B2 — stdlib の反復子が trait を名乗るように なった**
- **STDLIB-TRAIT-BASE B1 / B3 / B4 — bound を書いた先で何かできるように なった**
- **STDLIB-TEXT T3〜T5 — テキストの分野が完了**
- **STDLIB-TEXT T0〜T2 + STDLIB-ORD (`str`) — `str` / `String` の境界が 決着**
- **STDLIB-ERROR-MODEL E0〜E5 — 失敗の運び方が決着**
- **COLLECTIONS C5 — `PriorityQueue<T: Ord>` (`core/std/collections/priority_queue.t`)**
- **COLLECTIONS C4 — `Deque<T>` (`core/std/collections/deque.t`)**
- **COLLECTIONS C3 — `Vec` の `insert` / `remove` / `swap_remove` / `contains` / `index_of` / `reverse` / `sort_by`**
- **COLLECTIONS C2 — `Set<T>` (`core/std/collections/set.t`)**

### 2026-09-02
- **BUMP-CHUNK-OVERSIZE — 1 MiB を超える確保がチャンクをはみ出していた**
- **COLLECTIONS C1 — `Dict` が hash 表になった**
- **TREE-WALKER-SIZEOF-STR — `__builtin_sizeof` が `str` 値に答えるようになった**
- **COLLECTIONS C0 (a) — generic な `==` の相手に `eq` が無いと型エラー (`E0010`)**
- **COLLECTIONS C0 (b)(c) — hash の土台**
- **WINDOW-ESCAPE — 窓がバッファより長生きできなくなった (`[E0026]`)**
- **MUST-USE — 捨てられた `Result` を警告するようにした (`[E0025]`)**
- **OP-OVERLOAD-CHAIN — overload の結果が普通の値になった**
- **TREE-WALKER-SELF-TYPE-ARG — 宣言戻り型が `Self` の型引数を名指すようにした**
- **ENUM-ASSOC-FN-PRODUCER — enum を返す associated function を enum 生成位置に書けるようにした**
- **SUBDIR-ASSOC-FN は解消済みだった** — サブディレクトリの `core/std/collections/vec.t` から `Span::from_parts` / `Ptr::try_from_raw` が呼べない…
- **NUM-W-FOR-RANGE — narrow int の `for` 範囲が 4 レーンで一致した**
- **COMPOUND-ARG-CALL — compound を返す呼び出しを引数位置に書けるようにした**
- **CHAR-LITERAL-GENERIC-ARG — レシーバが決めた型引数を パラメータ型に流すようにした**
- **DIAG-SYMBOL-NAME-LOWER — lowering の診断も名前を綴るようにした**

### 2026-09-01
- **ENUM-ARG-NEST — enum を返す呼び出しを引数と payload に書けるようにした**
- **ENUM-VARIANT-ARG — enum の構築を引数位置に書けるようにした**
- **代入は `Unit` — ブロックの末尾に置いても値を持たない**
- **NUM-W-ENUMERATION — レシーバ表の 6 コピーを 1 つにし、隠れていた JIT の符号バグを 3 件出した**
- **DIAG-SYMBOL-NAME — 診断が名前を綴るようにした**
- **NET N5 — 名前解決。これで NETWORK_IO.md の N0〜N5 が全部埋まった**
- **NET N4 — UDP / アドレス / socket option**
- **NET N3 — Poller (epoll / kqueue の統一形)**
- **narrow int のビット演算が tree-walker で落ちていた**
- **NET N2 — TCP server (`TcpListener`)**
- **COMPOUND-BLOCK-RHS — `match` から compound を取り出せる**
- **UNIT-TYPE-ARG — `Result<(), E>` / `Option<()>` が 4 レーンで動く**
- **NET N1 — TCP client (tree-walker のみ)**
- **EXTERN-BUF の借用が typed slot を見ていなかった**
- **impl メソッドの move が解析されていなかった**
- **IMPL-BLOCK-VISIBILITY — impl メソッドから stdlib が見えるようになった**

### 2026-08-31
- **RUNTIME-TRAP-NARROW — `checked_*` / `saturating_*` が全 8 幅で使える**
- **NET N0 — プラットフォーム切り替えの足場** — `#[cfg_attr(path)] mod sys;` 1 箇所で epoll / kqueue を選び、未対応 OS は `compile_error!` で落ちる。
- **EXTERN-BUF — `extern fn` が toylang のメモリに届く**
- **CONV-SPAN — 既にあるバッファを `Span<T>` として見られるようになった**
- **primitive レシーバの method call が全幅で動く**
- **GENERIC-IN-ENUM-PAYLOAD / SELF-IN-TYPE-ARG — `Option<Ptr<T>>` が 書けるようになった**
- **DOD Phase 3 — 配列要素としての enum + tag 列**
- **DOD Phase 1 — 列の窓 `ps.mass`**
- **DOD Phase 0.5 — 列の tight pack**

### 2026-08-30
- **DOD Phase 0 — `soa [T; N]` landing**
- **DOD Phase 2 — `soa Vec<T>` landing**
- **STDLIB-FREE-FN-UNCHECKED 解消 — stdlib の free function body も型検査する**
- **CHAR-LITERAL-NUM — char リテラルが位置の整数型を取る**
- **`fn f() -> ()` が書けるようになった**
- **RUNTIME-LIB P0-B — `core/std/parse.t` (`parse::to_u64` / `to_i64` / `to_f64` / `to_bool`)**
- **RUNTIME-LIB P0-A — io 書き込み系 (`write_file` / `append_file` / `eprint` / `eprintln` / `io::exit`)**
- **POINTER P6 — `unsafe fn` の宣言と強制 (`[E0024]`)**
- **POINTER P5 — `Ptr<T>` の non-null 不変 + `Option<Ptr<T>>`**
- **POINTER P4 — `core/std/span.t` の `Span<T>`**
- **POINTER P3 — `core/std/ptr.t` の `Ptr<T>`**
- **POINTER P2 — `__getitem__` / `__setitem__` の 2 バグ + compiled レーン dispatch**
- **POINTER P1 — `__builtin_sizeof::<T>()` 型引数形**
- **SIMD-VM-SLOT — 測って払うと決めた** — IR VM の `RawSlot` 8 → 16 バイト化の代償を 4 ワークロードで実測: call 中心 +2.0% / ループ +5.3% / struct −0.5%…
- **SIMD Phase 3 (戦略 B) — stdlib kernel を SIMD 化**
- **AOT の ISA を baseline 固定に** — `make_object_module()` が `cranelift_native::builder()` (ビルドマシンの CPU 機能を検出) から `isa::look…
- **SIMD Phase 2 — 128bit vector を型にし、演算子を lane-wise に効かせた**
- **SIMD-F32 の残 (一部)** — IR VM の `to_string_value` に `F32` の行が無く、`"{x}"` が生ビット (`1069547520`) を出していた。
- **SIMD-F32 — `f32` を primitive 型として追加 (SIMD.md 論点 1 解決)**
- **NULL-COALESCE — `a ?? b` 演算子**
- **FROM-INTO-ENUM-ERR — `?` の cross-error 変換が enum エラー型でも 3 バックエンドで動く**
- **RUNTIME-IO — `read_file` / `env_var` / `read_line` が `Result<_, IoError>` を返す**
- **TRY-ERR-RETYPE — `?` が success 型の変更を跨げる + `return` の型検査**

### 2026-08-29
- **ENUM-EQ-ESCAPES-TYPECHECK**
- **METHOD-ARG-AUTOBORROW** — frontend が認める `T` → `&T` の auto-borrow を lowering が実体化していなかったので、値がポインタのスロットに入り、**IR VM は…
- **注釈の有無で operator overload の到達可否が変わっていたのを修正**
- **`&Self` / `&mut Self` を compiled レーンが lower できるようにした**
- **COMPOUND-ASSIGN-BITWISE** — ビット系の複合代入 5 種 (`&=` / `|=` / `^=` / `<<=` / `>>=`) を足した。
- **戻り型を書かない `main` の panic を修正**
- **DBC-LISKOV (E0023)** — trait method の `impl` が自分の `requires` を足すのを型検査で拒否するようにした。
- **CONTRACT-ELISION 拡張 (制御フロー)**
- **REGION Phase 1 (E0022)** — スコープ付き allocator (`with allocator = arena { ... }`) から確保したメモリが arena より長生きする形を型検査で拒否する。
- **EFFECT-SYSTEM** — 到達可能性で判定する 3 つの検査 (`never_allocates` / `const fn` / 契約の純粋性) が各自持っていた「禁止 builtin」テーブルを 1…

### 2026-08-28
- **CLOSURE-CAPTURE E0〜E3 / E5 (診断) / E6**

### 2026-08-27
- **DEBUG-OBS D3 の残: `HeapAlloc` の `SiteId` 移行**
- **DEBUG-OBS: tree-walker への replay を落とした**
- **DEBUG-OBS: 値を持つ文言を全実行系に** — D0 の目標表が決めていた 3 件 (`u64` underflow の `1 - 5` / 配列 OOB の `index 5, length 3` / 契約違反の `(wi…
- **DEBUG-OBS D6: 再帰深度と stdlib の境界**
- **DEBUG-OBS D5: ユーザ API と機械可読出力**
- **DEBUG-OBS D4: shadow stack**
- **DEBUG-OBS D3: IR の `SiteId`**
- **DEBUG-OBS D2: `FileId` と `SourceMap`**
- **DEBUG-OBS D1: interpreter の backtrace の穴埋め**
- **DEBUG-OBS D0: 診断の比較レーン** — 実行時の失敗の**文言**を突き合わせるレーンを `compiler/tests/consistency/diagnostics.rs` に作り、目標文言を `DEBUG_OB…

### 2026-08-26
- **COMPILE-TIME-EVAL C5: 配列長に `const` / `const fn` / 式**
- **COMPILE-TIME-EVAL C4: 契約との接続 + warning 機構**
- **COMPILE-TIME-EVAL C2: IR の定数畳み込み**
- **COMPILE-TIME-EVAL C6: 評価器の一本化**
- **COMPILE-TIME-EVAL C0/C1/C3: `const fn`**
- **COMPOUND-BLOCK-RHS: struct / tuple を産む composite を `val` の右辺に**

### 2026-08-25
- **MATCH-STRUCT-ARM: composite tail の struct / tuple 戻り値がゼロになる**
- **STRUCT-UPDATE: `P { x: 5i64, ..base }`**
- **リファクタリング一巡** — 巨大関数 6 本を機能別に分割 (`lower_program` / `lower_builtin_call` / `lower_instruction` / `evaluat…

### 2026-08-24
- **NUMBER-HINT: 既定は `u64` で確定 + 位置の網羅**
- **NUMBER-HINT: suffix なし整数リテラルの位置ベース解決**
- **FOR-RANGE-END: `for i in 0u64 to n {` が parse エラーだった件**
- **NEWTYPE: tuple struct (`struct Meters(i64)`)**
- **CHECK-NONTERMINATION: `--check` の trial にステップ予算**
- **DBC-CHECK-METHODS: `--check` がメソッドも掃く**
- **CONTRACT-ELISION の残 3 件** — 符号付き添字 / `!=` 由来の 0 除算 / `for` 範囲からの境界 guard を落とせるようになった。

### 2026-08-23
- **AOT-GENERIC-THROUGH-STRUCT: struct 引数越しの generic 型引数推論 (AOT / compiler JIT)**
- **TEST-PERF-CHECK-TRIALS: `--check` の trial ごとの registry 再構築を共有化**
- **TYPE-NAME-SPELLING: 名前型の 3 つの綴りを統一**
- **JIT-enum-1: struct のフィールドに enum を置けるように (3 backend)**
- **159: interpreter JIT の generic struct 対応**
- **CALL-ARG-COMPOUND-LITERAL: compound literal を call 引数に直接渡せるように**
- **STRUCT-FIELD-GENERIC-ENUM: struct のフィールドに enum を書けるようにした**
- **PATTERN-OR-NESTED: sub-pattern 位置の or**
- **PATTERN-RANGE: 範囲を実 pattern 形にし、被覆判定を入れた**
- **PATTERN-AT-BINDING: `@` を実 pattern に (`Pattern::Binding`)**
- **PATTERN-COMPOUND-LOWER: struct / tuple パターンを lowering 対応**
- **PATTERN-STRUCT: struct パターン**
- **CONTRACT-ELISION: 添字境界の guard も消す**
- **NEVER-ALLOCATES の残件 2 つを解消**
- **DBC-CHECK-CASES: `--check` が「どれだけ試したか」を出す**

### 2026-08-21
- **NEVER-ALLOCATES: 静的な「確保しない」**
- **MEM-COUNTER-INTERP-DRIFT: アロケーションカウンタの定義を固定**
- **ALLOC-CONTRACT-SUGAR: `ensures allocates(N)` / `retains(N)` / `allocations(N)`**
- **DBC-TRAIT-INHERIT: trait の契約を impl に継承**
- **CONTRACT-ELISION: 契約が RUNTIME-TRAP の guard を消す**
- **ALLOC-CONTRACT: `ensures` の `old(expr)`**

### 2026-08-20
- **TYPECHECK-LIES: `null` を型検査で拒否 (E0015)**
- **RUNTIME-TRAP: 算術 / 添字の実行時トラップを 4 バックエンドで統一**
- **TEST-PERF: with-core フロントエンドパスを全レーンで共有 (4 レーン → 1 パス)**
- **BUILD-PERF 運用: cargo-sweep 導入 + CLAUDE.md に定期 GC コマンドを明記**
- **FRONTEND-PERF: frontend の O(n²) を 3 点で解消 (★★★)**

### 2026-08-19
- **TEST-PERF: AOT の demand-driven lowering + codegen 刈り込み (★★★)**

### 2026-08-18
- **tree-walker の関数再帰ガード (call-depth)**
- **`null` / `is_null()` の扱いを確定 (docs の仕様に実装を追従)**
- **型不一致診断の user 型を source 綴りに (interner 経由)**
- **`str.substring` / `str.split` の dispatch を接続**
- **`str + str` を型検査で拒否 (E0002)**
- **CLAUDE.md の `--message-format=short` 案内を `--format=json` に誘導**
- **INCR-INTEGRATE: 統合パスを placeholder 2 パス + HashMap から 1 パス + オフセット演算に**
- **PATTERN-EXTEND: or / 範囲 / `@` パターン (3 バックエンド)**
- **INTERP-DIAG-SPAN: 補間内の診断が実際の位置を指すように**
- **STR-INTERP-FMT: 補間の format spec (`"{x:.2}"`, 3 バックエンド)**
- **DOC-DRIFT 解消** — `docs/language.md` の *Generics and bounds* が「bound は parse されるが強制されない」と書いていたのを実際の挙動 (cal…
- **STDLIB-ORD-BOUND: impl block の generic bound を call site で強制**
- **STDLIB-ORD: `Ord` trait + `Vec::sort` (3 バックエンド)**
- **RUNTIME-IO 拡張: 乱数シード / 時刻フォーマット / 環境変数一覧 (3 バックエンド)**
- **TRAIT-BOUND: generic trait の bound を call-site で強制**
- **FROM-INTO: `.into()` と `?` の cross-error 変換**

### 2026-08-17
- **STDLIB-ITER-ADAPT: `VecIter` に `map` / `filter` / `enumerate` / `zip` / `collect` (3 バックエンド)**
- **STDLIB-ITER-ADAPT (Dict / String 版)**

### 2026-08-16
- **RUNTIME-PORT R0+R1: ランタイムを C から Rust に移植**
- **RUNTIME-PORT R2 + FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]`**
- **RUNTIME-PORT R3: toylang 化の計測と判断 (移動は中止)**
- **RUNTIME-PORT R4: f64 整形の toylang 化は計測で却下、byte 一致は固定**
- **RUNTIME-IO: 最小 I/O セット (3 バックエンド)**
- **STDLIB-ITER: `Vec` / `Dict` / `String` に `iter()` (3 バックエンド)**
- **DROP-GLUE: 移動先が再帰的に解放される (3 バックエンド + IR VM)**
- **BOX-T Phase E+F: stdlib `Box<T>` (`core/std/box.t`)**
- **BOX-T Phase D: 移動された束縛は drop しない (3 バックエンド)**
- **BOX-T Phase C: 所有権の移動と use-after-move チェック (E0014)**
- **BOX-T Phase A+B: 型引数経由の再帰を通す**
- **PTR-READ-ENUM: enum の byte layout を関数境界の flatten に統一**
- **`__builtin_ptr_read` が user 定義型名の注釈を受けるように**
- **RECURSIVE-TYPES step 1: 再帰型を診断で拒否 (E0013)**
- **parser: 改行前 `(` は method call に継続しない**
- **CONCRETE-IMPL-Phase-2c (generic-wildcard 完遂)**
- **STR-INTERP-COMPOUND-EXTEND-ENUM**
- **MATCH-LET-RHS-PAYLOAD-INFER 完遂**

### 2026-08-15
- **`--check` が満たせないサイズの確保で Rust panic していたのを修正**
- **lexer エラーを診断として報告 (E0012)**
- **struct field / enum payload の初期化形を 4 形に揃えた (AOT/JIT)**
- **compound 値の読み出し側を AOT/JIT で**
- **receiver を読まない method の AOT panic を修正**
- **非 ASCII のソースリテラルの化けを修正** — UTF-8 scalar 単位で写す。
- **テストスイートを 5.15s → 4.4s に (-15%、CPU 90s → 82s)**

### 2026-08-13
- **`str == str` を内容比較に統一** — interpreter (tree-walker) だけが内容比較で、他 4 実装は runtime handle の整数比較だった (**型は通るが答えが違う** diver…
- **`Display` trait — 型が自分の見せ方を決める**
- **str リテラルの数え差 (interpreter だけ +1 確保) を解消**
- **Drop 内で `&mut self` フィールドを free すると use-after-free するバグを修正**
- **ポインタ演算 builtin (`__builtin_ptr_offset`)**
- **allocator レジストリ: `layout_report` を `--profile=mem` に自動で載せる**
- **エンジン fallback 時の副作用重複実行を修正**

### 2026-08-10
- **MEMORY-PROFILING M0〜M5 完了**
- **暗黙の impl 型パラメータ (`impl Container<T>`)**
- **stdlib の generic enum HOF を全 backend で**
- **MATCH-LET-RHS-PAYLOAD-INFER (第一段)**
- **AOT-MATCH-SCRUTINEE-EXPAND**
- **INCREMENTAL-COMPILATION Phase 5**
- **LLM-LOOP-FIX: 式の span を full extent に**
- **LLM-LOOP-FIX: 壊れた `as` キャスト提案 / 引数型不一致 E0010 → E0001 / JIT 列の空洞化**
- **LLM-LOOP P7: 補助 CLI** — 型ホール `val x: _ = expr` (専用コード E0011)、`--api <file>` (シグネチャ一覧)、`--explain [<CODE>]`。
- **DEV-LOOP D6: `--all-backends` + stdin 入力**
- **DEV-LOOP D5: `CODE_MAP.md` 新設 + `CLAUDE.md` から履歴を分離**
- **LLM-LOOP P6-3: u64 アンダーフローの trap**

### 2026-08-09
- **LLM-LOOP P0〜P6** — 設計は [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md)。
- **DEV-LOOP D1〜D4, D7** — 設計は [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md)。
- **D7 sweep が検出した潜在バグ 5 件** — AOT の代入式が値を produce していなかった / f64 `%` の明示エラーが cranelift assertion に化けていた / struct field…
- **`else if` の拒否 + パースエラーの握り潰し解消**
- **`var` の型注釈チェック追加** — `var w: bool = 1u64` が通り `println(w)` が `1` を出していた (`val` は正しく拒否)。

### 2026-05-31
- **Interpreter IR VM 化 Phase 0〜4**
- **意味論の決着 3 件** — git だけでは追いにくいので残す):
- **負数 array index を compiler 側にも実装**
- **clippy fixes across workspace**

### 2026-05-23
- **core/std 並列パース (Phase 1)** — module integration を parallel pre-parse + sequential integrate の 2 段に分割、二重パースを除去。
- **Cranelift 関数 codegen 並列化 (Phase 2)**
- **Incremental compilation Full AST cache (Phase 4)**

### 2026-05-19
- **`dyn Trait` Phase 2 (A5-P2-MVP-A〜F)**
- **Bare-name imported function calls (Phase 1)**
- **`Program` → `File` rename (Phase 4)**

### 2026-05-18
- **`dyn Trait` Phase 1 (A5-P1)**
- **Trait 多重 bound `<T: A + B>` (A2)**
- **Trait デフォルトメソッド本体 (A1)** — AST mutation pre-pass で impl に synthesize。

### 2026-05-17
- **`?` (Try) early-return operator**
- **`loop {}` + comparison chain**
- **GENERIC-ENUM-MATCH-HOF** — `Option::map` / `Result::map` / `map_err` を stdlib に追加。

### 2026-05-10
- **DEBUG-BUILTINS Phase A+B+C**

### 2026-05-09
- **IF-VAL (`if val` / `while val`)**

### 2026-05-08
- **LABEL (labelled break / continue)**
- **OP-OVERLOAD 完全コレクション** — 同型 struct ペアの全 binary + unary operator を user method に dispatch。
- **STRING-NOMINAL + STR-INTERP-COMPOUND**
- **AOT lower 系の汎用拡張** — `__builtin_sizeof` の compound 対応、`__builtin_ptr_write/read` の compound 対応。

### 2026-05-07
- **ITER-PROTOCOL-TRAIT** — generic trait 宣言 `trait Foo<T, U>` と `impl Foo<i64> for Counter`。
- **ITER-PROTOCOL-AOT** — `for x in EXPR` を AOT でも動作。
- **STR-INTERP Phase 2 (AOT + cranelift JIT)**

### 2026-05-06
- **STR-INTERP Phase 1 (interpreter)**
- **ITER-PROTOCOL Phase 1 (interpreter + JIT)**

### 2026-05-05
- **NUM-LIT-SEPARATORS** — `1_000_000u64` 等。
- **CLOSURES Phase 1〜8** — frontend / 型検査 / interpreter / AOT (direct / indirect / capturing / narrow int / return…
- **DOCS-2026-05-05** — / **NUM-W-JIT** / **ZERO-MEMCOPY-FIX** (`size==0` の libc parity) / **TYPE-ALIAS 周辺整備**。

### 2026-05-04
- **エスケープシーケンス** — `\u{HEX}` / `\xHH` / char literal `'a'` (u32)。
- **TYPE-ALIAS / GENERIC-TYPE-ALIAS**
- **GENERIC-RAII** — user struct の `impl Drop` を scope-bound auto-call (interpreter + AOT)。
- **ALLOCATOR Phase 5** — `trait Drop` + temporary-form の auto-cleanup。
- **REF-Stage-2** — `&T` / `&mut T` の borrow + writeback、escape rule の構文 reject。
- **121-Phase-B-rest** — arena / fixed_buffer の native runtime、`with` body 早期 exit の cleanup。
- **TEST-PERF-lazy-core** — / **STRING stdlib** / **CONCRETE-IMPL Phase 1〜2b**。
- **NUM-W (Phase 1〜6 + AOT + AOT-pack + signed-hash)**
- **DICT 系まとめ** — `Dict::new()` の AssociatedFunctionCall 経路、per-monomorph generic substitution。

### 2026-05-03
- **VEC-collection** — user-space `Vec<T>` (`core/std/collections/vec.t`)。
- **STR-LEN-O1 / STR-PTR-LEN** — AOT で `__builtin_str_len` を O(1) 化、`.rodata` layout を `[bytes][NUL][u64 len LE]` に。
- **121-Phase-B-min / Phase-A**
- **MUT-SELF-Stage-1** — `&mut self` receiver。
- **96残-前半** — match の deep exhaustiveness check。

### 2026-05-02 以前 (大きめのマイルストーン)
- **#183 コンパイラ MVP** — IR / cranelift-object backend で実行ファイル生成。
- **per-module function namespacing (#193 / #202)**
- **コア・モジュール auto-load (#193)**
- **Extension trait 全 backend 対応 (#191、Step A〜F)**
- **Math externalisation (#190、Phase 1〜4)**
- **Option / Result stdlib (#203)**
- **Value/Reference 分離 Phase 1〜5**
- **panic / assert / DbC (#166〜#175)**
- **言語仕様拡充 (#161〜#165)** — f64、`%`、複合代入、タプル JIT、ネスト分解、match arm guard。
- **#184 Trait + impl** — / **#170 top-level const** / **#169 `docs/language.md` 新設**。
- **MODULE-SYSTEM P1 — stdlib の配置と名前**
- **MODULE-SYSTEM P2 — qualifier がモジュールのフルパスになった**

## 測定の記録: テスト時間 (TEST-PERF、2026-08 時点)

> 数字は 2026-08 のもの。現在の値は todo.md の「現状の把握」にある。

2026-08-23: `--check` (property) の trial が毎回 program 全体の registry を
  再構築していたのを共有化 (TEST-PERF-CHECK-TRIALS、完了済み節)。最長の
  単一テストだった `a_pass_records_how_much_was_actually_tried` が
  4.85s → 0.40s、`builtin_test_and_check_tests` が 7.0s → 1.6s (CPU)。
  以降の数値は 2026-08-19 の測定。**負荷が低い状態での再計測待ち**。
  クレート別 CPU:
  | | CPU | テスト数 | 平均 |
  |---|---|---|---|
  | compiler | 78.3s (52%) | 408 | 192ms |
  | interpreter | 55.5s (37%) | 984 | 56ms |
  | frontend | 15.6s (10%) | 583 | 27ms |
  - **`compiler::consistency` + `example_consistency` が suite CPU の 45%** ★★★ — 46.5s (317 テスト) + 22.0s (14 shard) = 68.5s。`consistency` の分布は二峰性で、**lite パスで完結するテストと full core にフォールスルーするテストで 1 桁違う**。**2026-08-20 のフロントエンドパス共有で 46.5s → 34s / 22.0s → 16s 相当 (下記)** まで下がったが、残りは AOT の codegen + link + spawn (cache warm でも ~50ms/テスト) と JIT の native compile が本質的なので、ここから先はバックエンド実行そのものの削減になる。
    **「lite → full 二重パス」は 2026-08-18 に潰したが、それ自体はコストではなかった**と分かったので記録しておく: `assert_consistent` の let-chain は**最も安いレーン (no-core の tree-walker) で短絡する**ので、stdlib を使うソースが捨てられる AOT codegen / link / spawn まで到達することは元から無かった。捨てていたのは parse + no-core 型検査 ~2ms だけ。実際に効いたのは同時に入れた**フロントエンドパスの共有**の方 (下記)。
  - **core module のロードが 1 プロセスあたり 27ms** ★★★ — trivial プログラムを空 core dir と比べた実測 (2026-08-18、debug ビルド): **33.5ms → 6.1ms**。nextest は 1 テスト 1 プロセスなので、interpreter の 984 テストはそれぞれこれを払う = ~26s CPU ≈ wall 1.3s。内訳は 2026-08-15 時点の計測 (integrate ~43% 削減が landing する前) で `integrate_modules` 11.3ms / `execute_entry` の context 構築 5.2ms / stdlib 40 impl block の型検査 2.5ms / その他の型検査 1.1ms。
    **測って分かった否定的な結果を 3 つ記録しておく**: (1) 「stdlib 本体を型検査しない」は採らない (測定時点では free function の body を `take(user_func_count)` で user 分しか検査しておらず、削れるのは impl block の 2.5ms だけだった。2026-08-30 にその `take` も外し、今は stdlib も全部検査する)。**型検査器は body を書き換える** (`?` の desugar、`Display` の `to_str` 挿入) ので、stdlib の body を検査しないと**書き換え前の AST がバックエンドに流れる** — 今の stdlib は `?` も補間も使っていないので通ってしまい、使った日に壊れる罠になる。(2) `remap_symbol` の memo 化 (module symbol → main symbol を Vec でキャッシュ) は**効果ゼロ**だった。integrate の時間は文字列ハッシュではなく AST を pool に複製する作業そのもの。(3) **「型検査済み core をプロセス内で使い回す」は unit テストには効かない** — nextest は 1 テスト 1 プロセスなので、そもそもプロセス内に 2 回目の呼び出しが無い。
    したがって残る手は (a) stdlib を使わないテストを `test_program_no_core` に寄せる (実測: `test_program` を no-core にすると interpreter の 879 テスト中 **797 が通り**、その binary は 2.3s → 1.3s。ただし stdlib 同居時の回帰を見なくなる = coverage を実際に落とす)、(b) **プロセスを跨いで**型検査済み core を再利用する (INCREMENTAL-COMPILATION 側の仕事。`File` が `Rc` を持つので素朴な in-memory memo 化はできない — 別スレッドから clone すると refcount が壊れる)、(c) 1 プロセスで core を複数回ロードしている `consistency` を直す — **解消 (2026-08-20)**: 4 レーンが 1 フロントエンドパスを共有するようになり、AOT / JIT レーンが毎回 core をロードし直す重複が無くなった (consistency -27% CPU)。
  - **プロセス起動が ~5ms × 1999 ≈ 10s CPU (約 7%)** ★ — 起動フロアの実測は空 core dir の trivial 実行 6.1ms。nextest は 1 テスト 1 プロセス。テストを機能別に束ねれば減るが、失敗の切り分けと引き換え。
  - ~~`serial_test` (`oop_tests.rs`) の並列化~~ — **効果ゼロと分かったので却下 (2026-08-18)**。`#[serial]` が付いているのは 8 テストで合計 **0.193s CPU (suite の 0.12%)**、1 本 18〜34ms と既に起動フロア。しかも `serial_test` のロックはプロセスローカルなので、**nextest では各テストが別プロセスに散る = 元から直列化していない**。

## 測定の記録: ビルド時間 (BUILD-PERF、2026-08 時点)

- **BUILD-PERF** — **ビルドはテスト実行より桁で高い**。クリーンな target で、`interpreter/src/lib.rs` を 1 行触ってからの再ビルドが **2.35s**、テストファイル 1 個なら **1.08s**、クリーンからのフルビルド (テストターゲット全部) が **23.7s** — 対して全 1999 テストの実行が 7.5s (2026-08-19 実測、20 コア)。ここは 2026-08-19 に一度片付けたので、**残っているのは運用の話**:
  - **target を肥大させないこと — 実測 49x で、他のどの施策より大きい** ★★★ — 同じ「lib を触って再ビルド」が、**95GB / 1,248,912 ファイル**まで育った target の上では **1m55s**、`cargo clean` 直後の 1.7GB / 7,729 ファイルでは **2.35s**。消えた 125 万ファイルの 99% は過去のビルドの残骸。理由は cargo が rustc に `-L dependency=target/debug/deps` を渡すことで、**リンカが毎回 125 万エントリのディレクトリを走査する**。「user 45s に対し sys 6分」という異常な比率の正体がこれで、リンカが遅いのではなくディレクトリが大きすぎた。**古い成果物を定期的に GC すること** — **運用セットアップ済み (2026-08-20)**: `cargo-sweep` を導入し、`cargo sweep --time 1` (世代 GC) を CLAUDE.md に明記。この状態に戻ると下の施策は全部誤差に埋もれる。
  - **テストバイナリは 1 クレート 1 本** (2026-08-19 landing、73 → 12) — cargo は `tests/*.rs` を **1 ファイル 1 バイナリ**でリンクするので、64 ファイルは ~27MB の実行ファイルを 64 回リンクすることを意味していた (各々が frontend / interpreter / cranelift を静的に抱える)。`autotests = false` + `[[test]]` 1 個 + `#[path]` でモジュール取り込み。ファイルは 1 つも移動していない。**クリーン比較で フルビルド 37.5s → 23.7s / CPU 8m45s → 3m13s、lib 変更ループ 4.33s → 2.35s**。代償はテストファイル 1 個の編集が 0.88s → 1.08s (クレートの suite 全体が再コンパイルされる) と、テスト名にファイル名が前置されること。
  - **third-party の opt-level は 0、ただし cranelift だけ 2** (2026-08-19) — 全 deps を 3 で焼くのはビルド時間の払い損だった。**テスト実行が速さを感じる dep は cranelift だけ** (各テストが小さなプログラムを JIT / AOT する) なので、そこだけ残した。**テスト実行は劣化していない** (7.5s)。綴りに 2 つ罠があり、**どちらも間違えても cargo はエラーを出さない**: キーは `overrides` ではなく **`package`** (`overrides` は 1.41 以前の名前で、`unused manifest key` として黙って無視される)、そして **`cranelift` 単体は umbrella crate にしか当たらない** (実体は `cranelift-codegen` 以下 12 crate なので個別に列挙する。一致しない package spec は警告なしで「オーバーライド無し」になる)。
  - **測って外れた仮説を 2 つ記録しておく**: (1) **デバッグ情報の削減は効かない** — `[profile.dev]` / `[profile.test]` に `debug = "line-tables-only"` を入れて 2m06s (対照 1m55s)、改善ゼロ。27MB の中身はデバッグ情報ではなく cranelift のコード。(2) **リンカ差し替え (lld) と Spotlight 除外は、上を片付けた後では測る意味がない** — 絶対値が 1〜2 秒台まで落ちているので削り代が残っていない。target が肥大していた頃の「リンクが遅い」という観察は、リンカの速度ではなくディレクトリ規模の問題だった。

- **TEST-PERF** — ワークスペース全体で **~6.5s** (2026-08-19 実測、20 コア、warm、`cargo nextest run`、1999 テスト。AOT demand-driven lowering で 7.8s → ~6.5s)。**この suite は wall ではなく CPU 律速**: テスト時間の総和 (149.4s CPU は demand-driven lowering 前の値) / 20 コア が下限で、実測 wall はそこに張り付いている。**ビルド時間は別問題で、そちらの方が大きい — BUILD-PERF を見ること**。最長の単一テストも 2.11s (`example_consistency` shard_8) なので critical path 律速でもない。したがって**並列度を上げる策は効かず、効くのは CPU そのものを減らす策だけ。換算レートは 20:1** (CPU を 20s 削って wall 1s)。
