# TODO - Interpreter Improvements

## 完了済み ✅

> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-08-13
- **str リテラルの数え差 (interpreter だけ +1 確保) を解消** — MEMORY-PROFILING M1 で「表現の差」として記録していた `String::from_str` の差異を除去。IR VM の `ConstStr` / `ConstStrBytes` が str リテラルを `HeapManager::alloc_uncounted` で counter-free に実体化するようにした (コンパイル系の `.rodata` と同じ「確保として数えない」扱い)。`allocator レジストリ` の `__builtin_record_allocator_layout("SlotRegion", ...)` がこの差異を毎回踏んでいた (interpreter だけ 19 bytes 余分)。`--all-backends --profile=mem` がリテラル込みで完全一致するようになった。テスト `string_literals_allocate_on_the_interpreter_but_not_when_compiled` は `string_literals_no_longer_allocate_differently_across_backends` に置換。1752 tests のまま (置換)。

- **Drop 内で `&mut self` フィールドを free すると use-after-free するバグを修正** — `fn new() -> Region { var r = ...; r }` のように **struct 束縛を return すると、ローカル `var r` に scope-exit の Drop が発火して `r.ptrs` を解放し、戻り値が dangling になった** (呼び出し側の Drop が二重解放)。AOT は segfault / IR VM は panic。真因は `lower_expr_block` がブロック末尾の `pop_and_emit_drops` で「return される束縛」も drop していたこと。**関数本体ブロック** (`drop_scopes` が空のとき) かつ tail が struct 束縛のときだけ、`pending_struct_value` の leaf locals に一致する DropTarget を retain で除去するようにした (ネストブロックの tail は `val` 束縛や分岐への copy なので対象外)。`SlotRegion` の Drop を slots 解放込みに戻した (登録のみの workaround を撤去 → AOT で free_count == alloc_count、リーク無し)。`a_struct_returned_from_its_constructor_is_not_dropped` で pin。1751 → **1752 tests pass**。

- **ポインタ演算 builtin (`__builtin_ptr_offset`) を追加** — MEMORY-PROFILING M3 の残。`__builtin_ptr_offset(base: ptr, offset: u64) -> ptr` で interior pointer を作る。offset ベースの free-list / region allocator を書くためのプリミティブ。**AOT / IR VM / compiler JIT は `BinOp::Add` に lower するだけ** (ptr は u64、新 InstKind 不要)。interpreter は `HeapManager::resolve_block` を追加し、`read_u64` / `write_u64` / `get_memory_slice` が interior アドレスを「含むブロックの逆引き」で処理するようにした (bump allocator は contiguous + 1-based なので `addr - 1` がそのままメモリ offset、typed_slots は `(addr, off)` キーのまま整合)。interpreter 側 JIT は reject (fallback)。`interior_pointers_read_and_write_independently` / `interior_pointers_compose` で 3 backend pin。1749 → **1751 tests pass**。

- **allocator レジストリ: `layout_report` を `--profile=mem` に自動で載せる** — MEMORY-PROFILING M3 の残。**Drop フック方式**: `__builtin_record_allocator_layout(name, managed, live, free_blocks, largest)` を全レイヤーに追加 (frontend enum + type checker + interpreter + IR VM + AOT `toy_record_allocator_layout` + compiler JIT mirror)。`SlotRegion` に `impl Drop` を足し、破棄時に `layout_report()` のフィールドを登録 → main 終了時点の最終レイアウトが自動で入る。レポートに `allocator layouts` 節 (テキスト/JSON、全バックエンド byte-identical)。`--all-backends` が layout 節も突き合わせる。**実装で見つかった既存バグ 1 件**: `__builtin_heap_free` を `&mut self` フィールド (`self.ptrs`) に Drop 内で呼ぶと、`&mut self` メソッド呼び出しが先行しない限り AOT segfault / IR VM panic (`value not defined`)。`SlotRegion` Drop は当初登録のみに留めたが、真因は「return される束縛も drop される」ことで、後続で修正済み (同上エントリ)。**str 名の読み取りは `read_unaligned` 必須** — `[bytes][NUL][u64 len]` レイアウトの len フィールドは 8 バイト整列しない (`byte_start + len + 1`)、Rust の `*const u64` 直 deref は debug で trap。1746 → **1749 tests pass**。

- **エンジン fallback 時の副作用重複実行を修正** — IR VM は既定エンジンだが、出力をしてから diverge すると tree-walker が再実行して `println` が 2 回出ていた (M4 でカウンタ側だけ巻き戻した残り)。`execute_entry_with_values` の IR VM 試行を `output::with_capture` で包み、**成功したときだけ** captured 出力を replay、fallback 時は破棄する。JIT は実行前に eligibility 判定で返るので対象外 (実行後に失敗する経路が無い)。`fallback_does_not_duplicate_stdout` で pin — IR VM eligible な「print して panic」プログラムの stdout が `hello` 1 回だけであることを assert。1745 → **1746 tests pass**。

### 2026-08-10
- **MEMORY-PROFILING M4: JSON 出力 + カウンタ builtin** — (1) `--profile-format=json` (両バイナリ) / `TOY_PROFILE_MEM=json` (AOT 単体)。`leaks` は空でも `[]` を出す — テキスト版が節ごと省略するのは人間には正しいが、消費側は「漏れていない」と「リーク報告より前の生成器」を区別できなくなる。JSON も**両側手書き**でテスト突き合わせ (テキスト版と同じ理由)。`--all-backends` の子は常にテキスト (境界を渡るのは数値、形は driver が自分の stderr について決めること)。受け入れ基準「run 間で bit-identical」を 2 テストで pin、期待値はテスト内に書き下し。(2) **カウンタ builtin 6 種** (`__builtin_live_bytes()` 等、`() -> u64`) を `requires` / `ensures` / `test` から読める。名前はレポートのフィールドと同一 (同じ数値に 2 語彙は間違える箇所)。`peak_at_request` は出さない (レポートの時間軸であってプログラムが意見を持つ量ではない)。AST は `BuiltinFunction::MemStat(MemStat)` の**ペイロード付き 1 変種**なのでバックエンドごとに match arm 1 本。**要点**: M1 の「プロファイル無効時は計数もしない」を素直に守ると AOT が常に 0 を返し、`ensures __builtin_live_bytes() <= N` が何も検査せず通る — **0 を返す方が答えないより悪い**。`InstKind::MemStatEnable` を足し、**カウンタを読むプログラムだけ** `main` 先頭に置く (計数だけ入れる。出力は `TOY_PROFILE_MEM` のまま)。走査で導出するのでフラグが古くならず、const はコンパイル時評価なので「先頭」が本当に先頭。**builtin を入れて初めて観測できた数え間違いが 2 つ**: (a) インタプリタは 1 プロセスで複数 run できるのにカウンタがプロセス生存期間で累積していた (コンパイル済みバイナリと必ず食い違う) → `execute_entry` 冒頭で reset、(b) run は JIT → IR VM → tree-walker と試し、途中で失敗したエンジンの確保も数えていた → fallback 時に snapshot/restore で巻き戻す。(b) が無いと**メモリ契約の違反が漏らしていない方の関数に帰属**した。どちらも `--profile=mem` は run の最後に読むので表に出なかった。**副産物**: `BuiltinFunctionSymbols` に名前を足すと以後の全シンボル ID がずれ、`DefaultSymbol` をそのまま保存する `.toycache` の古いエントリが化ける (無関係な `val a: u64 = 5u64` が stdlib 由来の型エラー 3 件で落ちた) → `FULL_AST_CACHE_SCHEMA_VERSION` を 3 に上げ、**「interner に事前投入する名前の集合」も bump 対象**だと doc コメントに明記。1738 → **1745 tests pass**。
- **MEMORY-PROFILING M3: `trait Alloc::layout_report` (断片化)** — **着手時の調査で前提が 1 つ崩れた**: `Arena` / `FixedBuffer` は**領域を管理していない** (どちらも個々の確保を default allocator に委譲して記帳するだけ、`FixedBuffer` の `cap` はバッファでなく quota)。よって両者に報告すべき断片化は存在しないので、`Global` を含む 3 つとも既定実装のまま**「報告しない」** (`known == false`) にした — **「領域を持たないので報告できない」と「断片化していない」は別の主張**であり、0 と書けば後者を意味してしまう。実装者ゼロの飾りにしないため、実際に領域を管理する **`SlotRegion`** を stdlib に追加。当初は 1 ブロックから offset で切り出す free-list allocator にする予定だったが、**toylang にポインタ演算の builtin が無く内部ポインタを作れない**ため書けないと判明し、等サイズスロット方式にした (各スロットが独立確保なので既存 builtin で書け、連続スロットの run として本物の断片化が起きる)。外部断片化は float でなく permille で返すのでバックエンド間で厳密に一致。**数値が飾りでないことの検証**: 1 つおきに解放して空き 48 バイト・最大 run 16 バイトの状態を作り、48 バイトの確保が実際に失敗することをテストで pin した。`layout_report` を `--profile=mem` に自動で載せるには allocator レジストリ (ランタイム → toylang のコールバック) が要るので保留。1735 → **1738 tests pass**。
- **MEMORY-PROFILING M2: サイト帰属 + リーク検出** — 設計を 1 点変えた。論点 4 は「lowering で静的サイト ID を採番し `site_id → ソース位置` の表をプログラムに持たせる」としていたが、**位置そのものを ID にする** (`(line << 32) | column`) 方が良いと実装時に判明: (1) AOT バイナリに文字列テーブルを埋め込んでランタイムに登録するという M2 の一番面倒な部分が丸ごと消える、(2) 全バックエンドが同じ `location_pool` を読むので**一致が構造的に保証**され ID を突き合わせる仕組みが要らない、(3) 単独で走る AOT バイナリも位置を出せる。`HeapAlloc` だけが site を運び、`realloc` は**ブロックが既に持っている site を維持**する (リークは「最後に伸ばした場所」ではなく「どこから来たか」を指すべき)。ABI は `toy_dispatched_alloc(handle, size)` → `(handle, size, site)` — 定数レジスタ 1 本の方が site を設定する別呼び出しより安い (codegen は実行時にプロファイルが有効か知らない)。レポートは interpreter / JIT / AOT で byte-identical、`--all-backends --profile=mem` が総計とリーク節の両方を突き合わせる。**帰属の粒度は確保サイトであって呼び出しパスではない** — `keep()` を 2 箇所から呼べば 1 サイトに集約される。テストで記録済み。
- **MEMORY-PROFILING M1: AOT 側の同一計数 + `--profile=mem`** — `interpreter --profile=mem` / `compiler --all-backends --profile=mem` / AOT バイナリは `TOY_PROFILE_MEM=1`。**計数の実装は 3 つ** (interpreter / C ランタイム / compiler JIT ミラー) — 1 つに寄せるには C を compiler バイナリにリンクする必要があるが JIT 側の print ヘルパ (stdout キャプチャのための別実装) と衝突するので、**一致は構造ではなくテストで強制**する (D7 と同じ)。算術だけ `MemoryStats::record_obtained/released` を公開して共有した。レポートは 3 実装で byte-identical、数値は humanize しない (「38.2 KB」は 1 バイト違う run を同じに見せる)。`realloc` の旧サイズは libc が返さないので C 側に open-addressing のポインタ→サイズ表を書いた (プロファイル無効時は表も作らないので通常実行の挙動は不変)。**受け入れ基準達成**: 生 heap builtin と Vec の成長 (realloc 4 回) で 4 バックエンドの厳密系メトリクスが完全一致。**この道具が最初の実プログラムで差異を検出した** — `String::from_str` を含むと interpreter だけ 1 確保 20 bytes 多い (interpreter は `str` をヒープに実体化、コンパイル系は `.rodata` を指す = STR-PTR-LEN)。表現の差であって計測のバグではないので、アドレス再利用差と同じくテストに記録した。1730 → **1733 tests pass**。
- **MEMORY-PROFILING M0: 用語の固定 + interpreter 側の確保計数** — 設計は [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md)。**用語を決めることが主目的**で、カウンタはその適用。`MemoryStats` を新設し、全項目を「プログラムが要求した内容」で定義した (allocator が何をしたかでは定義しない) — これが M1 以降で 4 バックエンドの数値を一致させる前提。具体的には **realloc を 1 件の resize として数える** (この実装はブロックを移動するが、in-place で伸ばす allocator も同じ数値を出さねばならないので、realloc が内部で使う alloc/free は計数しない)。実測 3 の用語ずれも修正 — `Arena::bytes_used` は **live** で (free が no-op なので reset までは累積と一致するだけ)、`FixedBuffer::used()` は真の live、`CLAUDE.md` の「累積追跡バイト数」も訂正。**interpreter が確保アドレスを再利用せず AOT は再利用する**という既存の差は `assert_consistent` では pin できない (一致しないことが要点) ので、interpreter=0 / AOT=1 を直接 assert する テストとして記録した。オーバーヘッドは 20 万回ループで計数なし 270ms / 計数あり 252ms — **言えるのは回帰なしまで**で、速くなったとは言わない (誤差)。1723 → **1730 tests pass**。
- **暗黙の impl 型パラメータ (`impl Container<T>`) を実装** — `docs/language.md` が「型パラメータリストは暗黙 — struct で宣言したパラメータを再利用する」と明記していたが**未実装**で、**リファレンス自身の例が型検査を通らなかった** (`Cannot unify Identifier(T) with Int64`)。`T` が「T という名前の具体型」として parse されていたのが原因で、動くのは `impl<T> Container<T>` だけだった (stdlib が全部 explicit 形なのでこれまで露見しなかった)。パーサに「型名 → 宣言された generic params」の表を持たせ、**型引数を parse した後に**宣言と照合して、宣言に載っている名前だけを parameter に昇格する。`u8` は型キーワードなので決して一致せず、CONCRETE-IMPL (`impl Vec<u8>` / `impl C<u8>` と `impl C<i64>` の併存) は無傷。最初の実装は宣言を丸ごと採用して concrete impl を template に変えてしまい、`no method C::tag` で回帰した — consistency test で pin 済み。副産物として `GENERIC-ENUM-HOF-USER` (user 定義 generic enum の HOF) も解消。**制約**: 暗黙形は struct/enum 宣言が impl より前にある必要がある (explicit 形には無い)。
- **stdlib の generic enum HOF を全 backend で** (`Option::map` / `Result::map` / `map_err` / `unwrap_or_else`) — interpreter でだけ動いていた。3 つのギャップが積み重なっていた: (1) `resolve_method_target` の generic 分岐が **enum receiver で `Ok(None)`** を返すため、target を先に解決する呼び出し側 (val 束縛 / print / match scrutinee) が「compound-returning method を expression position で使えない、`val` で束縛せよ」と — 既に `val` で束縛しているコードに — 言っていた。(2) 関数型パラメータの中にしか現れない method-only generic param (`map<U>(f: fn (T) -> U)`) を推論できなかった (引数の IR 型は U64 ポインタで戻り型の情報を持たない) ので closure literal の宣言シグネチャから取るようにした。(3) その `fn (T) -> U` パラメータが active monomorphisation を適用せずに lower され `Function([Generic(T)], Generic(U))` のまま落ちていた。**`closure_tests.rs` の「型チェッカの unifier が弾く」というコメントは 2026-05-17 に解消済みの古い記述**で、2 か月後に未解決課題として引用されていた (todo 整理時に拾ってしまった) ため訂正。
- **MATCH-LET-RHS-PAYLOAD-INFER** — 全 arm が payload 束縛の match を val/var 右辺に置けるように。`value_scalar` は `&self` なので lowering 時の `arm_body_type` のように pattern を束縛して再帰できず、代わりに **enum 定義から payload の宣言型**を読む。enum の同定は pattern の名前ではなく **scrutinee** から行う — generic enum は instantiation ごとに intern されるので `Option<i64>` と `Option<u64>` は base name が同じでも別物。残るのは method-call scrutinee のみ (target 解決に `&mut self` が要る)。
- **AOT-MATCH-SCRUTINEE-EXPAND** — enum を返す**関数呼び出し**を match scrutinee に許可 (`while val Some(x) = func(i)`)。既存の method-call 経路と同形で、free function は self を持たないので receiver leaves も `&mut self` writeback も無い (引数の `&mut T` writeback は従来どおり)。解決は `resolve_call_target` を通すので closure 束縛と generic 単相化も同じ経路に乗る。
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

- **MATCH-LET-RHS-PAYLOAD-INFER (residual)** ★ — 全 arm が payload を束縛する match を val 右辺に置く形は、**scrutinee が method call のときだけ**まだ「could not infer scalar type for val/var rhs」で落ちる。識別子・関数呼び出しは 2026-08-10 に対応済み。method target の解決は `&mut self` を要るので、read-only な `value_scalar` からは引けないのが理由。回避策は arm の 1 つにリテラル body を持たせるか、呼び出しを先に local に束縛すること。
- **159. JIT の generic struct 対応** ★★ — `struct_layouts` を type-args 別に持つ refactor。踏むと `JIT: skipped (... see #159)` が出るので診断から辿れる (`jit_skip_reason_for_generic_struct` で wording を pin)。
- **160. タプルの JIT 対応 (ネスト)** ★ — `((a,b),c)` と tuple-of-struct。`ParamTy::Tuple(Vec<ScalarTy>)` を tree 構造にする 100+ 箇所の refactor。inline tuple literal を call 引数に渡す件も残り。
- **JIT-enum-1 (residual)** ★ — ネストした generic enum payload (`Option<Option<T>>`)、enum 型の struct field、payload に struct / tuple を持つ enum。
- **STR-INTERP-COMPOUND-EXTEND-ENUM** ★★ — enum 値の補間 (AOT)。tag local を読んで variant 別 format を出すために cranelift の if-elif chain が要る。interpreter は既に動作。
- **CONCRETE-IMPL-Phase-2c** ★★ — annotation hint (`var v: Vec<u8> = Vec::from_str(...)`) を interpreter / compiler の lookup まで thread して lone-spec fallback を狭める。型チェッカの `struct_methods` registry を `Vec<MethodSpec>` 形に refactor (現状 same `(struct, method)` で異 args の impl は last-wins)。
- **NUM-W-AOT-pack Phase 3** ★ — compound element 配列の tighter layout (`[PackedRgba; N]` が 4 バイト相当のところ 32 バイト消費)。メモリ効率のみで機能差はない。
- **195b. `extern fn` の monomorph 化** ★ — generic extern は現状 interpreter の type-erased registry でのみ動く。JIT / AOT には mangled symbol の emit と Rust 側実装の登録が要る。実需要なし。
- **185残. 3+ part qualified call** ★ — `std::math::abs(x)`。現状は `import std.math` 経由のみ (parser が last 名だけを採る)。auto-load があるので実害は限定的。
- **121-Phase-B-rest-leftover** ★ — `AllocatorBinding::Generic/Local/Ambient` の lower 配線 (perf のみ、観察可能な振る舞い変化なし)、`__builtin_default_allocator()` の戻り型を `u64` にして生比較を許すかの API 判断。
- **REF-Stage-2 (residual)** ★ — compound `&mut T` の真の pointer-passing、`&T` compound の RefScalar 経路活用。どちらも copy 削減で機能差はない。
- **183. コンパイラ MVP の残** — compound-returning method の expression position 制約。個別項目は上記に分解済み。

### 型システム (NEW-TYPE-SYSTEM)

- **Trait 拡張** ★★★ (大規模、ロードマップ)
  - **A3: trait inheritance (`trait B: A`)** — 中。super trait 経由で `A` の method を `B` impl からも要求。
  - **A4: associated types (`trait Iterator { type Item }`)** — 中〜大。
  - **A5-P3-interp: interpreter 側 JIT の `dyn Trait`** ★ — `ScalarTy::from_type_decl` が `TypeDecl::Dyn` で `None` を返し silent fallback。correctness 問題はなく、compiler 側 JIT が実用的な高速化を担うので優先度は低い。
  - **A5-P4: `Box<dyn Trait>`** — owned trait object + `Vec<Box<dyn Trait>>`。**前提**: `Box<T>` 自体が未実装。
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`、generic trait の default body 内での `T` 参照。
- **Trait-bounded generic API** ★★ — `fn first<I: Iterator<i64>>(iter: I)` の bound check。`<T: Trait>` は struct で動くが generic trait の bound は未強制。
- **`Display` trait** ★★ — user-defined `to_string` で文字列補間の既定動作を拡張可能に。A1 完了で前提は揃っている。
- **`From` / `Into`** ★★ — `val s: String = "hi".into()`。`?` の cross-error 変換にも要る。
- **`must_use` / unused-Result 警告** ★★ — `?` の補完。**警告の emit 経路が無い**ので (`Severity::Warning` は型としては存在するが未使用)、そこから作る必要がある。
- **slice 型 `&[T]`** ★ — 配列 borrow を first-class に。中〜大。
- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣の候補 (NEW-FEATURES、未着手)

- **OP-OVERLOAD-CHAIN** — `a + b + c` の chained position。現状は let-rhs のみ。binary struct literal operand も対象外。
- **`??` (null-coalesce)** ★ — `opt ?? default` で `unwrap_or` の糖衣。
- **raw / multi-line string literal** ★ — `r"\path"` / `"""..."""`。lexer 拡張のみ。

### メモリプロファイリング

- **MEMORY-PROFILING** ★★ — 設計は [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md)。実行時のメモリ使用量と断片化を記録し、実行後にレポート / 機械可読な数値を出す。**着手前の実測で 3 点判明済み**: (1) interpreter の `HeapManager` は再利用しない bump allocator なので **アドレス由来のメトリクスは全バックエンド共通にできない** (同じプログラムで interpreter は再利用せず AOT は再利用する)、(2) AOT の `toy_dispatched_alloc` は allocator handle を無視している、(3) `Arena::bytes_used` と `FixedBuffer::used()` は同名だが意味が違う (arena は free が no-op)。**断片化は `trait Alloc` の責務**にして allocator 自身に報告させる — ランタイムに閉じ込めると単なるツールだが、trait に置けばユーザ定義 allocator も同じレポートに乗る。**M0 完了 (2026-08-13)** — 用語を `MemoryStats` に固定し interpreter に計数を入れた。**M1 完了 (2026-08-13)** — `--profile=mem` / `--all-backends --profile=mem` / `TOY_PROFILE_MEM=1`。**M2 完了 (2026-08-13)** — サイト帰属とリーク検出。**M3 完了 (2026-08-13)**。**M4 完了 (2026-08-13)** — JSON 出力 + カウンタ builtin。**M5 完了 (2026-08-13)** — ポインタ演算 builtin (`__builtin_ptr_offset`)。

### インクリメンタルコンパイル

- **INCREMENTAL-COMPILATION** — 設計と実測は [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md)。Phase 1〜5 完了。**残る改善余地は「16 個の core module のキャッシュ読み込み + 統合に毎回 ~10ms」** (lowering の 2.5 倍)。per-module IR compilation + IR linker は warm 19ms のうち ~4ms しか狙えないので保留 — 着手するなら、大きめの実プログラムで lowering が支配的になることを**再測定してから**。

### テスト・ドキュメント

- **TEST-PERF** — ワークスペース全体で ~5s (2026-08-10 実測、`cargo nextest run`)。残: `interpreter/tests/` の 25 テストバイナリを機能別に統合 ★、core モジュールのパース結果を `thread_local!` でテストバイナリ内共有 ★★、`serial_test` (`oop_tests.rs`) の並列化 ★。
- **65. frontend リファクタリング** — (a)〜(g) は完了。残: doc コメント拡充、プロパティベーステスト追加。
- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。
- **26. ドキュメント整備** — `docs/language.md` / `compiler/README.md` / `interpreter/README.md` は最新化済み。残: API リファレンス、advanced topics。

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
- 合計 **1702 テスト**、31 skipped (100% 成功、2026-08-10 時点)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は interpreter / JIT / AOT の 3 経路一致を保証する。
- ワークスペース全体で ~5s。`compiler/build.rs` が `toylang_rt.c` を pre-build し、リンク結果は `TOY_LINK_CACHE_DIR` で content-addressed にキャッシュされる (キャッシュが効くにはコード生成が決定的である必要がある — `compiler/tests/reproducible_build.rs` が pin)。

### パーサーの既知制限事項
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省いているのはこのため)。
- 3-part qualified call (`std::math::abs(x)`) は parser が **last 名だけを採る**。名前が一意なら結果的に解決するが、意図した経路ではない (`#185残`)。
