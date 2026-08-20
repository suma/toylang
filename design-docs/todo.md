# TODO - Interpreter Improvements

## 完了済み ✅

> **この節は 1 行サマリだけを持つ。** 実装の経緯・測定値・ファイルパス・
> テスト数は git log のコミットメッセージにある。フェーズ設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) /
> [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) /
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) /
> [`FEATURE_NOTES.md`](FEATURE_NOTES.md) を参照。
> ここを段落で埋めると、常時読まれるファイルが changelog になる。

### 2026-08-20
- **TEST-PERF: with-core フロントエンドパスを全レーンで共有 (4 レーン → 1 パス)** —
  残っていた 2 レーン (AOT の `compile_file` / JIT の `run_source`) が
  with-core テストごとに `core/std/*.t` の integrate + 型検査を再実行して
  いたのを、**「型検査済み program を受け取る」ライブラリ入口**を足して畳んだ。
  (1) `compiler::compile_checked_program` を新設 — `compile_file` から
  parse + type-check 部分を分離し、`program` / interner / `ContractMessages`
  を受け取って lower + codegen + link だけを行う (CLI 挙動は不変)。
  (2) `consistency.rs` に `CheckedProgram` (program + interner +
  contract_msgs) を導入し、`assert_consistent` / `assert_stdout_consistent`
  の full path が parse + type-check を **1 回**にして tree-walker / IR VM /
  JIT / AOT の 4 レーンに配る (IR VM は MEMORY_PROFILING M4 の
  `reset_profile` を維持)。(3) `example_consistency.rs` も同構造に —
  `run_both_engines` の shared-program を AOT レーンまで拡張し、
  `run_compiled` が `compile_checked_program` 経由に。**実測 (warm、user CPU)**:
  `consistency` 37.5s → 27.5s (**-27%**)、`example_consistency` 12.1s →
  8.5s (**-29%**)、両者合わせて ~13.6s CPU 削減。テスト 1999 グリーン +
  clippy 無警告。
- **BUILD-PERF 運用: cargo-sweep 導入 + CLAUDE.md に定期 GC コマンドを明記** —
  「target を肥大させない」の運用を具体化。`cargo sweep --time 30` (世代 GC、
  --time より新しい成果物は残す) と `cargo clean` (全削除) を
  CLAUDE.md に追記し、`cargo install cargo-sweep` で導入済み。
  `cargo sweep --dry-run` で削除対象を先に確認する流儀も記載。

### 2026-08-19
- **TEST-PERF: AOT の demand-driven lowering + codegen 刈り込み (★★★)** —
  auto-load された stdlib の ~190 関数を**毎回全部 lower / codegen** していた
  のを、`main` + `test` ブロックから到達可能な transitive closure だけに。
  **設計判断**: (1) pass 1 の宣言は全関数のまま (FuncId 解決のため、安価) —
  本体 lower だけを需要駆動に。`schedule_from_ir` が「lower 済み body の IR を
  走査して参照された bodyless 関数を enqueue」する fixpoint ループで、
  generic instance / closure / drop glue / dyn thunk は従来どおり作成時点で
  自前キューに乗り `scheduled` set で二重 enqueue を防ぐ。(2) thunk は
  参照された vtable (`VtableAddr`) のみ body を lower。(3) `compiler_ir` に
  `Module::reachable_from` / `call_edges` を新設 (IR VM の eligibility.rs と
  共有。従来の `reachable_functions` / `call_edges` を削除)。(4) codegen の
  `declare_all` と `build_object_module` / JIT / `--emit=clif` は
  reachable-from-main のみ宣言・compile (bodyless の unreachable を
  cranelift に宣言すると `finish()` が body を要求して panic するため)。
  **測定** (debug, opt=none, link cache warm): lowering (`--emit=ir`)
  0.10-0.11 CPU → **0.03-0.04** (~3x)、フル AOT 0.18-0.21 → **0.09-0.10**
  (~2x)。`main` だけのプログラムは 196 関数中 1 個だけ lower される
  (String 使用なら 4 個)。**workspace テスト 7.8s → ~6.5s wall**。object
  bytes は再現 (reproducible_build / link cache グリーン)。全 1999 テスト +
  clippy 無警告。

### 2026-08-18
- **tree-walker の関数再帰ガード (call-depth)** — IR VM (既定エンジン) は
  ヒープにフレームを積むので 100000 段でも通るが、IR VM が lower を諦めて
  **tree-walker に fallback したプログラム**は host stack を使い、debug
  ビルドの 2 MiB テストスレッドで ~40 フレーム、main スレッド (8 MiB) で
  ~200 フレームで `fatal runtime error: stack overflow` (exit 134) に
  なっていた。既存の `recursion_depth` ガード (1000) は式評価の入れ子しか
  数えないので先に host stack が尽きる。**直し方**: `EvaluationContext` に
  `call_depth` / `max_call_depth` を追加し、
  `evaluate_function_with_values_writeback` (全関数呼び出しの共通入口) で
  increment + 上限チェック。decrement は requires エラー / 通常 return の
  全経路で実施 (ensures エラー経路は evaluate_block 後に decrement 済み
  なので二重 decrement に注意 — 1 回踏んだ)。`max_call_depth=30` —
  実測で 2 MiB スレッドの限界 (~40 フレーム) より手前で発火する安全値。
  IR VM は影響なし。テスト:
  `tree_walker_call_depth_guard_trips_before_stack_overflow` (struct 返し
  main で fallback を強制し、エラーを pin)。
- **`null` / `is_null()` の扱いを確定 (docs の仕様に実装を追従)** —
  方針 2 択を「**予約・実行時停止**」に決めた (docs/language.md が
  2026-08-18 の監査で既にそう確定していた。`null` は parser / 型検査は
  通すがどのバックエンドも評価せず、代替は `Option<T>`、raw pointer は
  `__builtin_ptr_is_null`)。実装側の曖昧さを除去: (1) `Expr::Null` の
  実行時メッセージを `Null reference error` → 「`null` cannot be
  evaluated: reserved / no backend implements it — model absence with
  `Option<T>`」に (docs も合わせて更新)、(2) `is_null()` の E0007 に
  「test raw pointers with `__builtin_ptr_is_null(p)` / absent values
  with `Option<T>::is_none()`」の誘導を追加、(3) 壊れた
  `interpreter/example/null_test.t` (`x.is_null()` を使う) を
  `__builtin_ptr_is_null` + `Option::is_none` の正しい example に書き換え、
  ERROR_EXAMPLES から外した (3 バックエンド一致 exit=42)。
  テスト: `is_null_suggests_the_supported_spellings` /
  `null_literal_stops_with_a_clear_message`。
- **型不一致診断の user 型を source 綴りに (interner 経由)** —
  `TypeCheckError` の `Display` は interner を持てないので、診断
  変換経路に interner を渡した: (1) `TypeDecl::spell_with(interner)`
  を新設 (`source_name` → 無ければ `display_name`)、(2)
  `TypeCheckError::message_with(interner)` を新設して `Display` は
  引数なしで委譲、(3) `Diagnostic::from_type_check_error(error, file,
  interner)` に interner を追加 — 呼び出し側 (interpreter の
  `check_typing_diagnostics`) は borrow 衝突を避けるため
  `tc.core.string_interner` (共有参照) 経由、(4) `type_name_for_error`
  の catch-all (`{:?}` を lowercase) を `spell_with` に置き換え —
  `Cannot convert 'u64' to 'identifier(symbolu32 { value: 41 })'` が
  `'Point'` になる。テスト: `user_type_mismatch_spells_the_source_name`
  (symbol id が漏れないことを pin)。
- **`str.substring` / `str.split` の dispatch を接続** — 型検査器は
  `BuiltinMethod::StrSubstring` / `StrSplit` を登録するのに、interpreter の
  `Object::String` レシーバ分岐 (`evaluation/call.rs`) の arm が `trim` /
  `to_upper` / `to_lower` で止まっており、実行時に
  `Internal error: Method 'substring' not found for String type` で停止
  していた。arm を 2 つ追加 (substring は 2 引数 u64 の byte range、
  split は `Object::Array` of `Object::String` — `builtin.rs` の
  `BuiltinMethod` 実装と同形)。`String` (stdlib struct) 版は struct
  method registry 経由なので影響なし、`str` 受け側のみの修正。
  AOT は従来どおり primitive builtin method を lower できない
  (interpreter-only は既存の仕様)。テスト 4 件
  (`str_substring_basic` / `_empty_range` / `_out_of_range_fails` /
  `str_split_basic` — 要素アクセス `parts[i]` 込み)。
- **`str + str` を型検査で拒否 (E0002)** — `visit_binary` が
  `str + str` を明示的に受理する arm があったが、**どのバックエンドにも
  実装が無い** (interpreter はゴミハンドル、AOT は bus error / exit 138)。
  型チェッカの doc コメントは「String concat is handled before the
  dispatch」と 5 バックエンドに無い前提を書いていた。arm を削除して
  `visit_arith_binary` に落とすと、`str` は primitive (overload 不可)、
  `String` も `add` overload を持たないので自然に E0002 になる
  (連結は `a.concat(b)`)。**診断の型名も source 綴りに**:
  `TypeDecl::display_name()` を新設して `TypeMismatch` /
  `TypeMismatchOperation` の `Display` が `{:?}` (String / Bool /
  SymbolU32) の代わりに `str` / `bool` / `u64` を出すようにした
  (display_name は interner 不要の primitive 表記で、user 型は
  Debug フォールバック — interner 経由の解決は残課題)。
  テスト: `str_plus_str_is_rejected_by_the_type_checker`
  (E0002 + `str and str` 表記をピン)。`docs/language.md` の Operator
  表も実態に合わせた。
- **CLAUDE.md の `--message-format=short` 案内を `--diagnostics=json` に誘導** —
  `--message-format=short` はどちらの CLI にも実装が無く、渡すと usage を
  出して終わるのに「診断を 1 行にする手段」として繰り返し勧めていた。
  実装せず、該当節を `--diagnostics=json` (severity / code / span /
  suggestions 付きの JSON 配列) の説明に差し替えた。
- **INCR-INTEGRATE: 統合パスを placeholder 2 パス + HashMap から 1 パス + オフセット演算に** —
  16 個の core module のキャッシュ読み込み + 統合 (~10ms) の削減。**計測** (release,
  warm): preparse (deserialize) ~1.5ms / sequential integrate ~3.2ms / 型検査 ~2.4ms /
  IR VM ~2ms。integrate() 本体が 1.3ms のうち、placeholder 2 パス + 2 つの
  `HashMap<u32, ExprRef>` が無駄だった。**設計判断**: (1) モジュールの pool を
  module index 順に追加するので `main_ref = base + module_index` は恒等式 —
  `expr_mapping` / `stmt_mapping` を削除し `map_expr` / `map_stmt` を純粋な
  オフセット計算に (前方参照は「base + index が確定していればスロットが未作成
  でも良い」)。placeholder フェーズも不要になり、copy は 1 パスに。
  (2) `remap_symbol` / `remap_type_symbol` は module シンボルごとの翻訳を
  `Vec<Option<DefaultSymbol>>` (dense symbol id を index に) でキャッシュ —
  同一シンボル (`u64` 等) が数百回現れても interner のハッシュ計算は初回のみ。
  (3) `module_path` / `shadowed_stdlib_types` はループ内 clone を参照渡しに。
  **結果**: integrate 本体 1.3ms → 0.74ms (**~43% 削減**)、sequential integrate
  3.2ms → 2.6ms、warm ラン全体 ~8-9ms。動作は「warm と cold で同一の
  `main_program` 状態」を構成で保証する設計のまま (置換は write 同順の
  add なので pool 内容は逐語一致)。全 1990 テスト + clippy グリーン。
- **PATTERN-EXTEND: or / 範囲 / `@` パターン (3 バックエンド)** —
  `1i64 | 2i64 => ...` / `0i64..5i64 => ...` / `n @ 2i64 => n`。
  **設計判断**: 新しい `Pattern` variant を足さず、**既に存在する形へ
  parser で desugar** した。(1) or は alternative ごとに arm を複製
  (**body の `ExprRef` は共有** — 走るのは 1 arm だけなので複製不要。
  型検査が body を複数回 visit するが、in-place rewrite (`?` / 補間 /
  Display) は 2 回目が no-op になるので安全。`test_or_pattern_shares_one_body`
  で pin)。(2) 範囲と `@` は **irrefutable な `Name` + 比較 guard**。
  guard 機構は既存なので網羅性の扱いも自動で正しい — **guard 付き arm は
  網羅に寄与しない**ので整数 match の `_` 必須は変わらず、or は guard が
  無いので `Color::Red | Color::Green` + `Color::Blue` が wildcard 無しで
  網羅になる。到達性 (`1i64 | 1i64`) も複製後の既存チェックが検出。
  **唯一のバックエンド改修**: AOT の match lowering が top-level
  `Pattern::Name` を拒否していたので追加 (scalar は StoreLocal、enum は
  storage の deep copy + drop target 登録)。interpreter JIT は
  top-level Name を元から reject するので範囲 / `@` は silent fallback。
  **制約**: 範囲は整数リテラル端点のみ、`@` は literal / 範囲のみ
  (enum variant への `@` は parser が明示的に拒否)、or は top-level arm
  のみ (sub-pattern 位置は未対応)。
- **INTERP-DIAG-SPAN: 補間内の診断が実際の位置を指すように** — 補間の中の
  型エラーが**ファイル先頭 (1:1)** を指していた (LLM-LOOP-FIX が潰した
  「無関係なコードを自信満々に指す」形が 1 箇所残っていた)。原因は
  desugar が合成 token を `insert_token` で挿していたこと — この関数は
  「カーソル位置のトークンの span」を借りるので、`>>` を `>` `>` に割る
  用途では正しいが、式まるごとを合成する desugar では**誰も占めていない
  位置**を主張することになる。**直し方**: (1) lexer が各 `{...}`
  セグメントの**絶対 byte offset** を `StringPart::Expr` に記録、
  (2) parser が sub-lexer の (0 始まりの) 位置にそれを足して
  `insert_token_at` で挿す。scaffolding (`concat` / builtin 名 / 括弧) は
  literal 全体の span。結果、`"first={a} second={a + b} third={a}"` の
  2 番目のセグメントを正確に指す。format spec のエラーも literal の
  span で報告 (parse 時点でセグメントの token はまだ無いため)。
- **STR-INTERP-FMT: 補間の format spec (`"{x:.2}"`, 3 バックエンド)** —
  `[align]['0'][width]['.'precision][type]` (`< > ^` / `x X b o`) の
  Rust サブセット。**f64 の桁数指定手段が言語に無かった**のを解消。
  **設計判断**: (1) spec は literal の一部で実行時値になりえないので
  **parse 時に検証して u64 1 個に pack** (`frontend/src/format_spec.rs`)
  — backend が見るのは `__builtin_format(value, <u64>)` というスカラー
  引数 1 個増えただけの形で、不正な spec は parse エラーになる。
  (2) 対象は **primitive のみ**。compound は再帰的な field walk で
  描画するので単一の width / radix に意味が無く、型エラーで拒否して
  `Display` の `to_str` に誘導する。(3) runtime helper は
  `toy_format_{i64,u64,f64,bool,str}` の 5 本。narrow int は codegen で
  sext/uext して自分の幅 (`bits`) を渡すので、`{-1i32:x}` が
  `ffffffff` (16 桁でなく 8 桁) になる。**pack の bit layout は
  `frontend/src/format_spec.rs` と `toylang_rt` の 2 箇所**に書かれる
  (後者は no_std で前者に依存できない) — `example/string_format_spec.t`
  の stdout 比較が両者を突き合わせる。**interpreter JIT は
  silent fallback** (`jit_format_<ty>` helper が未実装、correctness に
  影響なし)。`BuiltinFunctionSymbols` に名前を足したので
  `FULL_AST_CACHE_SCHEMA_VERSION` を 10 → 11。
- **DOC-DRIFT 解消** — `docs/language.md` の *Generics and bounds* が
  「bound は parse されるが強制されない」と書いていたのを実際の挙動
  (call site で強制、pass-through / generic trait の型引数一致 / 多重 bound)
  に直し、*Known limitations* から解消済みの 2 件 (enum 補間 /
  MATCH-LET-RHS-PAYLOAD-INFER) を削除。両方とも `--all-backends` で
  3 バックエンド一致を確認してから消した。
- **STDLIB-ORD-BOUND: impl block の generic bound を call site で強制** —
  `impl<T: Ord> Vec<T>` の method は receiver の型引数が bound を満たさないと
  `[E0010] Method 'sort' generic parameter 'T' bound violation`。以前は
  型検査を素通りし、interpreter runtime (`Method 'lt' not found`) /
  AOT compile まで落ちなかった。free function 側の検査
  (`visit_generic_call`) を `check_generic_bounds` に切り出して method 経路と
  共有。**要点**: (1) parser が impl-level bound を各 method の
  `generic_bounds` にマージ済みなので MethodSpec に bounds を足す必要はなく、
  足りないのは「impl の型パラメータ名 → receiver の具体型」の対応だけ
  (`impl<E: Ord> Vec<E>` は struct の `T` と名前が違いうる) —
  `MethodSpec.target_type_args` から作る。(2) substitution に無い
  パラメータは検査しない (method-only generic は引数から bind される
  前なので、false positive を出さない側に倒す)。(3) bound 違反の診断が
  `expected Identifier(Ord)` と内部表記を漏らしていたのも直した。
  generic **enum** receiver (`impl<T: Ord> Holder<T>`) も同じ検査を通る。
- **STDLIB-ORD: `Ord` trait + `Vec::sort` (3 バックエンド)** —
  `core/std/ord.t` に `trait Ord { fn lt(self: Self, other: Self) -> bool }`、
  `core/std/collections/vec.t` に `impl<T: Ord> Vec<T>::sort()` (安定
  insertion sort)、`core/std/string.t` に `impl Ord for String`
  (byte-wise)。**設計判断**: (1) method 名を `<` 演算子オーバーロードの
  `lt` と同じにした — `impl Ord` が `<` も自動で得る (3 backend で pin)。
  (2) receiver は `self: Self` (by value) — primitive には deref が無いので
  `&self` では値を比較できない。compound は alias 意味論なので sort が
  `key.lt(...)` を繰り返しても key は壊れない。**frontend 修正 2 件**:
  (1) bounded-generic receiver の method dispatch が `Generic` 形しか扱って
  おらず、`val key: T` (bounded impl 内) の `Identifier` 形で
  `key.lt(...)` が method not found になっていた — `Identifier` 形も
  bound の時に受理。(2) `satisfies_trait_bound` が primitive receiver
  (`min(5u64, 3u64)` の u64) を bound 違反にしていた — extension-trait の
  登録 (`impl Ord for u64` は `"u64"` symbol 配下) を確認する arm を追加。
  **制約**: `Vec<非Ord>::sort()` は型エラーにせず interpreter runtime /
  AOT compile で落ちる (method dispatch は impl の generic bound を検証
  しない — 今後の課題)。`str` の `Ord` は byte 比較が AOT で書けないため
  未提供。テスト: ord_tests 10 + consistency 1 (primitive / f64 / String /
  user struct / `min<T: Ord>` / `<` 演算子を 3 backend 一致で pin)。
- **RUNTIME-IO 拡張: 乱数シード / 時刻フォーマット / 環境変数一覧 (3 バック
  エンド)** — `core/std/io.t` に `random_seed(seed)` / `strftime(fmt, secs)` /
  `env_count()` / `env_name(i)` / `env_value(i)` を追加。**設計上の要点**:
  (1) `random_seed(s)` で `random()` が再現可能になる (0 も literal に保持、
  シーケンスは 3 バックエンド一致 — interpreter の xorshift と `toylang_rt` を
  逐語一致させた。初回 derive と explicit seed を区別する seeded フラグを
  ThreadState に追加)。(2) `strftime` は C `strftime` の文書化された部分集合で
  **UTC 固定** (ローカル時刻にしない) — 決定性を 3 バックエンドで保証するため。
  実装は `toylang_rt` の純関数 `strftime_utc` 1 本で、interpreter が
  `toylang_rt` を dependency にして**同じ関数を共有** (RUNTIME-PORT の
  「ミラーを作らない」方針)。(3) 環境変数一覧は `environ` 順の
  `env_count` / `env_name` / `env_value` (interpreter の `std::env::vars` も
  同じ順)。**挫折した設計**: `env_names() -> Vec<String>` は AOT の
  module-qualified compound return 制約 (`io::env_names()` が lower 不能) に
  当たるため stdlib から外した (raw accessor で十分)。テスト: io_tests 4 +
  consistency 1 (seeded random / strftime / env 一覧を 3 バックエンド一致で
  pin)、toylang_rt 単体 2、`io_demo.t` が example sweep に乗る。
- **TRAIT-BOUND: generic trait の bound を call-site で強制** —
  `fn first<I: Iter<i64>>(it: I)` が「型引数込みでその trait を
  実装している struct」だけを受け付けるように。従来は
  `Iter<i64>` が `Struct(iter, [i64])` に parse され、非 trait bound
  扱いの等価比較に落ちて **全呼び出しが false の bound violation** に
  なっていた (`impl Iter<i64> for Counter` が通らない)。
  (1) context に `trait_impl_type_args` (struct → trait → impl ごとの
  型引数リスト) を追加、`check_trait_conformance_with_args` が
  conformance 成功時に記録。(2) utility の
  `satisfies_trait_bound` / `trait_type_args_match` が impl 型引数と
  bound 型引数を照合 (bound 側は call-site substitutions で解決、
  generic impl `impl<T> Iter<T>` は wildcard)。(3) generics と
  struct_literal の bound check を trait bound (bare / generic /
  intersection) 統一ルートに。(4) method_call の bounded generic
  receiver は trait method を型引数置換付きで解決
  (`I: Iter<i64>` の `next()` は `Option<i64>`)。エラー表示も
  `Iter<i64>` と型引数込み。テスト: trait_tests 3 + consistency 2。
- **FROM-INTO: `.into()` と `?` の cross-error 変換** —
  `core/std/convert.t` に `trait From<T>` / `trait Into<T>`、`String`
  に `impl From<str>`。`.into()` は期待型 (型注釈) から
  `Target::from(expr)` への AST 書き換え (blanket `Into` は where 節が
  書けないので checker が導出)。`?` は enclosing 関数の戻り型
  (`Result<T, E2>`) と inner (`Result<T, E1>`) が異なり
  `E2: From<E1>` があれば error arm を `E2::from(e)` → `Result::Err`
  再構築 → bare-identifier return に書き換え、From が無ければ
  専用エラー。enum の associated function call (`MyErr::from`) も
  型チェッカで受理。**制約**: enum エラー型への変換は AOT/JIT が
  enum associated call を lower できないため interpreter のみ。
  `Expr::Try` に `converted_binding` / `result_binding` 追加、
  `FULL_AST_CACHE_SCHEMA_VERSION` 9 → 10。

### 2026-08-17
- **STDLIB-ITER-ADAPT: `VecIter` に `map` / `filter` / `enumerate` /
  `zip` / `collect` (3 バックエンド)** — `core/std/collections/vec.t`。
  アダプタは全て普通の `next(&mut self) -> Option<T>` struct なので
  for ループ desugar は無変更。**設計上の要点**:
  (1) **frontend の generic struct 経路に method-only generic param の
  推論が無かった** (`fn map<U>(&self, f: fn (T) -> U)` の U が解決されず
  戻り型が `MapIter<u64, U>` のまま。enum / non-generic struct 経路には
  あった)。`&self` / `&mut self` receiver は `method_func.parameter` に
  含まれないので arg index がそのまま param index に写る
  (`self: Self` は含まれる、`has_self_param` では判別できない —
  compiler_lower は parameter.len() > arg_refs.len() で判定)。
  (2) generic struct で method が見つからないとき field-call フォール
  バック (Closure Phase 8) に落ちなかった — 落ちるようにして、fn 型
  フィールドの型に struct の generic params を置換 (self.f の戻りが
  `Generic(U)` のままになるのを防ぐ)。impl スコープの Generic 戻りは
  デバッグチェックで許可。
  (3) compiler_lower 3 箇所: method-only param 推論の param offset、
  `bind_method_only_param` がネストした Generic (`other: VecIter<U>` の U)
  を bind、`lower_type_arg` が active_subst を参照 (generic method body の
  型注釈 `val src: VecIter<T>` の T を解決)、field-call の lower も
  `lower_scalar_with_subst`。
  (4) **レジスタ制約**: `&mut self` writeback + `Vec` 戻り (6+4=10) が
  cranelift の戻りレジスタ上限を超えるので collect は **by-value self**
  (`self: Self`、呼び出し側のイテレータは alias のまま = 再利用可)。
  ZipIter は 8 フィールド版がオーバー → `elems` に a/b の stride を
  32bit ずつパックして 5 フィールド (DictIter と同じ流儀)。
  (5) **enumerate / zip の collect は提供しない**: `Vec<(A, B)>` の
  push が `__builtin_sizeof` のタプル値解決 + compound 引数 lower の
  AOT 未対応に当たるため。map / filter / VecIter の collect のみ。
  consistency に 5 テスト、`std_iter_adapt.t` が example sweep に乗る。
- **STDLIB-ITER-ADAPT (Dict / String 版)** — `DictIter<K, V>` に
  `map` / `filter` (`core/std/dict.t`)、`StringIter` に `map` /
  `filter` / `enumerate` / `collect` (`core/std/string.t`)。3
  バックエンド。**Dict 版の AOT 制約**:
  (1) **タプル引数 closure は AOT 不可** (「closure parameter requires
  a primitive scalar type」) — closure は `fn (K, V) -> U` で k, v を
  別スカラー引数に取り、アダプタ側でタプルを分解してから呼ぶ。
  (2) **5-leaf DictIter + 2-leaf fn = 7 writeback + 2 return = 9 で
  レジスタ上限超過** — アダプタは source をネストせず state をフラット
  に持ち、`count` を `index` の上位 32bit にパックして 6 レジスタに
  収めた (VecIter 版の MapIter は source 4 + f 2 = 8 でギリギリ)。
  collect は Vec<(K,V)> になるため提供しない。**frontend 修正**:
  non-generic struct 経路の method-only param 推論も `&self` receiver
  では param_idx = i (self が parameter に入らない。enum 経路と同型の
  バグが VecIter 版では generic struct 経路にのみあった)。
  consistency に 6 テスト、`std_iter_adapt_dict.t` /
  `std_iter_adapt_string.t` が example sweep に乗る。

### 2026-08-16
- **RUNTIME-PORT R0+R1: ランタイムを C から Rust に移植** — `toylang_rt` crate
  (`compiler/runtime/toylang_rt/`、`no_std` + alloc、依存 0) が
  `toylang_rt.c` の全 53 シンボルを継承し、**AOT と compiler 側 JIT が同一
  ソースを実行**するように (旧 jit.rs の ~880 行ミラーを削除)。
  設計は [`RUNTIME_PORT.md`](RUNTIME_PORT.md)。**設計上の要点**:
  (1) 出力シンクは per-thread (`pthread_key` 経由の TLS で `ThreadState`
  1 個に sink / alloc stack / bump head / profiler / io args を収める。
  `#[thread_local]` は stable no_std に無い)、JIT の
  `run_capturing_stdout` は sink を差し替えるだけ — 旧ミラーの global
  Mutex alloc stack のレースも per-thread 化で解消。(2) `build.rs` は
  rustc 直呼び staticlib (`--cfg toylang_rt_standalone` で panic handler /
  malloc-backed global allocator / `rust_eh_personality` を付与。
  `--remap-path-prefix` で決定性)。(3) **f64 整形の正本を Rust `Display`
  に統一** (論点 4): AOT の出力が変わる (`0.1+0.2` → `0.30000000000000004`、
  `1234567.75` → `1234567.75` と精度が上がる方へ)。`docs/language.md` に
  明記、`f64_display_agrees_across_backends` (実測 1 の 3 式 + 境界値) を
  consistency に追加。(4) str の print は codegen が渡す **NUL 終端
  cstring (byte_start)** を受け取る — ヘッダの `toy_print_str` を参照。
  (5) macOS リンクに `-Wl,-dead_strip` を追加 (staticlib は 1 archive
  member なので全 helper が入ってしまうため)。単体テスト 8 本 (str layout /
  bump 冪等性 / f64 整形 / io args) が副産物として新設された。
  `LINK_CACHE_VERSION` 2、`--profile=mem` (text/JSON) と reproducible
  build はグリーンのまま。
- **RUNTIME-PORT R2 + FFI_PLAN P1: `extern fn ... from "lib" [as "sym"]`** —
  extern 宣言がシンボルとライブラリを直接名指しできるようになり
  (`Function.extern_link`)、`core/std/io.t` が `getchar` / `time` を
  `from "c"` で宣言して **`read_line` / `now` が toylang 実装**に (RUNTIME-IO の
  `toy_io_read_line` / `toy_io_now` を toylang_rt から削除)。残り 6 シンボルは
  `from "toylang_rt" as "toy_io_*"` で宣言 (extern 境界が C ポインタを deref
  できないため argv / FILE* は runtime helper に残る)。**設計上の要点**:
  (1) 型制約は scalar のみ (narrow int は R2 で許可に修正 — 整数レジスタクラス
  は同一、`getchar` の i32 が要る)。`from "toylang_rt"` は内部マーシャリング
  のため適用外。(2) interpreter は registry 優先 → registry に無い from-fn は
  `extern_ffi.rs` で libloading + レジスタクラス trampoline (arity ≤ 4)。
  io の libc 名は Rust std 実装の registry が応える (libc は dlopen しない)。
  (3) JIT は `symbol_lookup_fn` で宣言 lib を dlopen。`"c"` は RTLD_DEFAULT、
  `"toylang_rt"` は symbol map。(4) AOT は `-l<lib>` + `TOYLANG_LINK_PATHS`
  → `-L`、link hash に link_libs を追加 (`LINK_CACHE_VERSION` 3)。
  (5) JIT の io argv は compile 時に空リセット、`profiler_reset` は注入済み
  args を保持。(6) `ffi_tests.rs` + fixture C ライブラリが 3 者一致を pin。
  `FULL_AST_CACHE_SCHEMA_VERSION` 8。
- **RUNTIME-PORT R3: toylang 化の計測と判断 (移動は中止)** — `str_eq` /
  `str_concat` / `to_string_*` の toylang 実装をプロトタイプで書いて計測:
  既定エンジン (IR VM) で byte-walk パターンは **~20 倍遅い**
  (str==str 40000 回: 0.2s → 3.9s) で doc の中止条件「interpreter が
  目に見えて遅くなる」に該当 → Layer 1 に残す (設計判断)。
  profiler 集計も doc の「バグを切り離して調べたい = native に残す」基準で
  移動しない。**副産物の発見**: `str == str` は型チェッカに経路が無く
  if/while 条件の未検査を経由してのみ動作していた (value 位置は E0002)。
  この前提崩れの修正として `visit_compare_binary` に String ペアの arm を
  追加し、`str == str` を一級にした (consistency: `str_eq_value_pos`)。
  R3 の経緯と測定値は RUNTIME_PORT.md に記載。
- **RUNTIME-PORT R4: f64 整形の toylang 化は計測で却下、byte 一致は固定** —
  doc の手順「interpreter とバイト一致を先にテストで固定」どおりに進めた。
  (1) **テスト固定**: `f64_display_comprehensive_set_agrees_across_backends`
  (巨大/微小 magnitude・`.0` 規則・signed zero・inf/NaN の 17 値、
  3 バックエンド byte 一致) と `f64_display_canonical_is_rust_display`
  (正本 golden)。(2) **計測**: toylang の f64 整形プロトタイプ
  (桁ループ) は IR VM で **~1.1ms/回** (AOT 3µs、native Rust 整形とは
  ~1000 倍) — 中止条件に明確に該当、移動せず。(3) allocator / profiler は
  doc の「native に残す」基準で移動しない (allocator の policy 層は
  2026-05 に toylang 化済み)。経緯は RUNTIME_PORT.md。
- **RUNTIME-IO: 最小 I/O セット (3 バックエンド)** — `core/std/io.t` に
  `read_line()` / `argc()` / `arg(i)` / `env_var(name)` / `read_file(path)` /
  `file_exists(path)` / `now()` / `random()`。既存 extern fn 機構
  (`__extern_` 名 → interpreter registry / AOT `libm_import_name_for` → C
  runtime シンボル / JIT Rust ミラー) を流用。**設計上の要点**: (1) extern 境界は
  compound return を運べないので失敗は `""` 返し + `file_exists` プローブ
  (`Result` は将来の struct-return FFI まで保留)、(2) `str` 引数は toylang str
  layout (len フィールド先頭ポインタ) で渡り、C runtime / JIT ミラーが
  同じ layout で読む、(3) AOT は実行ファイル本体が toylang main なので argv を
  macOS `_NSGetArgc/_NSGetArgv`、Linux `/proc/self/cmdline` から取得、
  (4) compiler-side JIT は in-process なのでプログラム引数を thread-local で
  注入 (デフォルト空 = 引数なしバイナリと一致)、(5) `random()` は時計 + pid
  シードで**非決定的** (3-way 比較不能、テストは範囲のみ)、(6) `RunOptions.args` +
  CLI のファイル後ろ引数をプログラム引数に。interpreter の extern dispatch は
  `str` リテラル (ConstString) を heap String に正規化してから registry に渡す。
- **STDLIB-ITER: `Vec` / `Dict` / `String` に `iter()` (3 バックエンド)** —
  `for x in v.iter()` / `for kv in d.iter()` (キー順は挿入順、payload は
  `(K, V)` タプル) / `for b in s.iter()` (1 バイトずつ)。iterator struct は
  `Box<T>` と同じく型引数をフィールドに持たない形で per-monomorph 不要。
  **frontend 修正 3 件**: `is_equivalent` に Generic↔Identifier 同一記号と
  Tuple 要素ごとの arm を追加 (guard 付きで generic wildcard の leniency は
  温存 — 最初 guard なしで入れて `var result: Vec<String> = Vec::new()` が
  壊れた)、`lower_type_with_subst` / `lower_param_or_return_type` に Tuple
  arm、enum payload の narrow int (u8〜i32) を許可 (`StringIter::next ->
  Option<u8>`)。**ABI 制約**: `&mut self` の writeback return がレジスタ上限
  8 に当たるため `DictIter` は key/value の stride を 1 つの u64 にパックして
  5 フィールドに抑えた (コメントに明記)。`Dict` / `String` のバッファは
  依然 Drop なし (leak は 3 バックエンド一致)。
- **DROP-GLUE: 移動先が再帰的に解放される (3 バックエンド + IR VM)** — `Box` を `Vec` / struct field / enum payload に移しても、コンテナの死とともに中の値が free される。`Vec<T>` は要素 + buffer、struct の glue はフィールド、enum は active payload、`Box<T>` は slot の中身 → 自 slot。再帰は型ごとに合成した drop-glue 関数 (`Box<List>` → `List` → `Box<List>` は runtime 再帰) で、tree-walker は値駆動の iterative な walk。**設計上の要点**: (1) 言語の alias (`val b = a`、`get()` copy、共有された boxed node) は同じ値を複数の drop 経路から到達可能にするので、**free を全バックエンドで冪等**にした — interpreter は元々 address 冪等、AOT/JIT の C/Rust runtime に always-on のサイズレジストリ (double-free は no-op) と **never-reuse bump region** (解放済みブロックの内容が残るので 2 回目の visit が元の値を見る) を導入。`ptr_read` copy は alias として drop 登録しない。(2) 所有は**推移的** (`contains_drop`): `Vec<Box<i64>>` や payload に Box を持つ enum も移動 (E0014) と glue の対象。(3) match arm 束縛は per-arm で drop (共有 continuation block に載ると別 arm の経路が未初期化 local で発火する)。(4) 長いリストは glue が runtime 再帰なので tree-walker と同様にホストスタックを使う — イテレーティブなのは tree-walker 側のみ。`--profile=mem` が 3 バックエンド byte-identical (parity テストで tree-walker 対 IR VM も pin)。`interpreter_heap_does_not_reuse_addresses_but_the_aot_heap_does` は「両方 never-reuse」に更新。
- **BOX-T Phase E+F: stdlib `Box<T>` (`core/std/box.t`)** — `enum List { Cons(i64, Box<List>), Nil }` が 3 バックエンドで動く。`Box` は**言語側に特別扱いが無い**普通の struct で、型引数がフィールドに現れないという Phase B の規則だけで成立する。付随して (1) associated function の compound 引数 lowering (`Box::new(struct_value)` が "arg produced no value" で落ちていた)、(2) enum variant 構築の payload を move 位置として扱う、(3) JIT の Drop allow-list に `Box` を追加 (auto-load される `impl Drop` は JIT を全プログラムで無効化するため)。`&Arena` 移行と docs (`--explain E0013/E0014`、`docs/language.md` の Ownership 節) も。
- **BOX-T Phase D: 移動された束縛は drop しない (3 バックエンド)** — 実測していた use-after-free が解消。`File::transferred_bindings` (`val`/`var` 文の `StmtRef` 集合) を型検査が埋め、tree-walker の `register_drop_if_needed` と `compiler_lower` の `register_drop_for_struct_binding` が参照して登録をスキップ。移動先 (Vec / struct field / callee) には drop glue が無いので**解放されない = leak** になるが、UAF より安全側で `--profile=mem` の `leaks` に出る。`FULL_AST_CACHE_SCHEMA_VERSION` を 7 に bump。
- **BOX-T Phase C: 所有権の移動と use-after-move チェック (E0014)** — `impl Drop` を持つ型の値を「今のスコープより長生きする場所」(値渡し引数 / struct・tuple・array の要素 / 代入右辺) に置くと所有権が移り、以後その名前を読むと E0014。`val b = a` は**別名のままで移動ではない** (compound の alias は仕様でテストもある)、`&T` 引数は borrow、raw ptr builtin は無検査 (`Box::new` がそれで書かれている)。分岐 / ループ本体からの移動は drop flag が要るので専用診断で拒否。`frontend/src/type_checker/move_check.rs`、診断のみでランタイム変更は Phase D。
- **BOX-T Phase A+B: 型引数経由の再帰を通す** — `struct Tree { kids: Vec<Tree> }` が書けるようになった。(A) `instantiate_struct` / `instantiate_enum` がメンバを lower する**前**に id を予約 (`Module::reserve_*` / `fill_*`)。(B) 型引数が辺になるのは**渡し先がそのパラメータを by-value で持つときだけ**、という規則を `check_recursive_types` に (宣言グラフ上の fixpoint)。member 位置と型引数位置で instantiate の入口を分け、member 側は予約を見ないので `struct Node { next: Node }` の backstop (Guard) は生きたまま。
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

- **RUNTIME-IO: `Result` を返す IO** ★ — 失敗理由付き `read_file` 等。
  extern 境界が compound return を運べないため未対応 — 将来 FFI の
  struct-return 対応か builtin 化で (2026-08-18 に乱数シード / 時刻
  フォーマット / 環境変数一覧は landing 済み)。
- **STDLIB-ORD: `str` の `Ord` impl** ★ — byte 比較が heap copy を要求し、
  generic context で AOT が表現できないため未提供 (`String` は提供済み)。

### 型システム (NEW-TYPE-SYSTEM)

- **MOVE-CONDITIONAL: 分岐 / ループからの移動** ★ — 現状は E0014 で拒否。
  許すには実行時 drop flag (Rust と同じ) が要る。実プログラムで踏んだら着手。
- **MOVE-ALIAS-GAP: `val b = a` 後の `a`** ★ — alias なので `b` を移動しても
  `a` の読みは検出されない。DROP-GLUE の冪等 free + never-reuse ヒープが
  二重 drop を無害化しているので、これは診断の網羅性の問題 (読み放題)。
- **NEWTYPE: tuple struct / newtype (`struct Meters(i64)`)** ★ — parse エラー。
  単位型・ID 型のラップが「1 フィールドの struct + 冗長な field 名」になる。
  parser + 位置指定のフィールドアクセス (`m.0`) が要る。
- **Trait 拡張** ★★★ (大規模、ロードマップ)
  - **A3: trait inheritance (`trait B: A`)** — 中。super trait 経由で `A` の method を `B` impl からも要求。
  - **A4: associated types (`trait Iterator { type Item }`)** — 中〜大。
  - **A5-P3-interp: interpreter 側 JIT の `dyn Trait`** ★ — `ScalarTy::from_type_decl` が `TypeDecl::Dyn` で `None` を返し silent fallback。correctness 問題はなく、compiler 側 JIT が実用的な高速化を担うので優先度は低い。
  - **A5-P4: `Box<dyn Trait>`** — owned trait object + `Vec<Box<dyn Trait>>`。**前提**: `Box<T>` 自体が未実装。
  - **A5 残作業** — `&dyn Trait` の return / struct field 位置 (REF-Stage-2 の escape rule が阻む)、`dyn A + B`、`dyn Iterator<T>`、generic trait の default body 内での `T` 参照。
- **Trait-bounded generic API** ★★ — ~~`fn first<I: Iterator<i64>>(iter: I)` の bound check。~~ **解消 (2026-08-18)**: generic trait の bound (`Iter<i64>`) が call-site で型引数込みで強制される。残るのは generic **enum** payload 経由の AOT lower 制約のみ (下記 159 / JIT-enum-1)。
- **`From` / `Into`** ★★ — ~~`val s: String = "hi".into()`。`?` の cross-error 変換にも要る。~~ **解消 (2026-08-18)**: `.into()` は期待型から `Target::from(expr)` へ書き換え、`?` は `E2: From<E1>` で error 変換。残る制約: **enum エラー型**への変換は AOT/JIT が `MyErr::from(...)` の associated call を lower できないため interpreter のみ (struct エラー型は 3 バックエンド)。
- **`must_use` / unused-Result 警告** ★★ — `?` の補完。**警告の emit 経路が無い**ので (`Severity::Warning` は型としては存在するが未使用)、そこから作る必要がある。
- **slice 型 `&[T]`** ★ — 配列 borrow を first-class に。中〜大。
- **const generics** ★ — `struct Array<T, const N: usize>`。大規模。

### 構文糖衣の候補 (NEW-FEATURES、未着手)

- **OP-OVERLOAD-CHAIN** — `a + b + c` の chained position。現状は let-rhs のみ。binary struct literal operand も対象外。
- **`??` (null-coalesce)** ★ — `opt ?? default` で `unwrap_or` の糖衣。
- **raw / multi-line string literal** ★ — `r"\path"` / `"""..."""`。lexer 拡張のみ。
- **STR-INTERP-FMT の残** ★ — (a) user 型に spec を渡す API
  (`Display` の `to_str(&self)` は引数を取らない規約なので、
  `fn to_str(&self, spec: str)` にするかは未決)、(b) fill 文字 / `+` /
  `#` / `$`-parameterised width、(c) interpreter JIT の
  `jit_format_<ty>` helper。いずれも踏んでから。
- **PATTERN-EXTEND の残** ★ — (a) sub-pattern 位置の or
  (`Shape::Circle(1i64 | 2i64)`)、(b) `@` を enum variant に
  (`x @ Color::Red` — guard では表現できないので `Pattern` 拡張が要る)、
  (c) 範囲の被覆判定 (`0i64..5i64` + `5i64..10i64` + ... で `_` 不要に)。
  いずれも踏んでから。
- **STRUCT-UPDATE: struct update 構文 (`P { x: 5i64, ..a }`)** ★ — parse エラー。
  「1 フィールドだけ差し替えた copy」が全フィールド列挙になる。

### インクリメンタルコンパイル

- **INCREMENTAL-COMPILATION の残** — Phase 1〜5 は完了 (設計と実測は [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md))。統合パスの削減
  (placeholder 2 パス + HashMap → 1 パス + オフセット演算、シンボル翻訳キャッシュ)
  は **2026-08-18 に landing** (integrate 本体 ~43% 削減)。残るのは
  (a) **preparse の deserialize ~1.5ms** (16 ファイルの並列 read + bincode。
  bundle 化 = 1 ファイルにすると invalidation が全モジュール単位になるので
  見送り)、(b) per-module IR compilation + IR linker (warm 19ms のうち ~4ms
  しか狙えないので保留 — 着手するなら、大きめの実プログラムで lowering が
  支配的になることを**再測定してから**)。

### テスト・ドキュメント

- **BUILD-PERF** — **ビルドはテスト実行より桁で高い**。クリーンな target で、`interpreter/src/lib.rs` を 1 行触ってからの再ビルドが **2.35s**、テストファイル 1 個なら **1.08s**、クリーンからのフルビルド (テストターゲット全部) が **23.7s** — 対して全 1999 テストの実行が 7.5s (2026-08-19 実測、20 コア)。ここは 2026-08-19 に一度片付けたので、**残っているのは運用の話**:
  - **target を肥大させないこと — 実測 49x で、他のどの施策より大きい** ★★★ — 同じ「lib を触って再ビルド」が、**95GB / 1,248,912 ファイル**まで育った target の上では **1m55s**、`cargo clean` 直後の 1.7GB / 7,729 ファイルでは **2.35s**。消えた 125 万ファイルの 99% は過去のビルドの残骸。理由は cargo が rustc に `-L dependency=target/debug/deps` を渡すことで、**リンカが毎回 125 万エントリのディレクトリを走査する**。「user 45s に対し sys 6分」という異常な比率の正体がこれで、リンカが遅いのではなくディレクトリが大きすぎた。**古い成果物を定期的に GC すること** — **運用セットアップ済み (2026-08-20)**: `cargo-sweep` を導入し、`cargo sweep --time 30` (世代 GC) を CLAUDE.md に明記。この状態に戻ると下の施策は全部誤差に埋もれる。
  - **テストバイナリは 1 クレート 1 本** (2026-08-19 landing、73 → 12) — cargo は `tests/*.rs` を **1 ファイル 1 バイナリ**でリンクするので、64 ファイルは ~27MB の実行ファイルを 64 回リンクすることを意味していた (各々が frontend / interpreter / cranelift を静的に抱える)。`autotests = false` + `[[test]]` 1 個 + `#[path]` でモジュール取り込み。ファイルは 1 つも移動していない。**クリーン比較で フルビルド 37.5s → 23.7s / CPU 8m45s → 3m13s、lib 変更ループ 4.33s → 2.35s**。代償はテストファイル 1 個の編集が 0.88s → 1.08s (クレートの suite 全体が再コンパイルされる) と、テスト名にファイル名が前置されること。
  - **third-party の opt-level は 0、ただし cranelift だけ 2** (2026-08-19) — 全 deps を 3 で焼くのはビルド時間の払い損だった。**テスト実行が速さを感じる dep は cranelift だけ** (各テストが小さなプログラムを JIT / AOT する) なので、そこだけ残した。**テスト実行は劣化していない** (7.5s)。綴りに 2 つ罠があり、**どちらも間違えても cargo はエラーを出さない**: キーは `overrides` ではなく **`package`** (`overrides` は 1.41 以前の名前で、`unused manifest key` として黙って無視される)、そして **`cranelift` 単体は umbrella crate にしか当たらない** (実体は `cranelift-codegen` 以下 12 crate なので個別に列挙する。一致しない package spec は警告なしで「オーバーライド無し」になる)。
  - **測って外れた仮説を 2 つ記録しておく**: (1) **デバッグ情報の削減は効かない** — `[profile.dev]` / `[profile.test]` に `debug = "line-tables-only"` を入れて 2m06s (対照 1m55s)、改善ゼロ。27MB の中身はデバッグ情報ではなく cranelift のコード。(2) **リンカ差し替え (lld) と Spotlight 除外は、上を片付けた後では測る意味がない** — 絶対値が 1〜2 秒台まで落ちているので削り代が残っていない。target が肥大していた頃の「リンクが遅い」という観察は、リンカの速度ではなくディレクトリ規模の問題だった。

- **TEST-PERF** — ワークスペース全体で **~6.5s** (2026-08-19 実測、20 コア、warm、`cargo nextest run`、1999 テスト。AOT demand-driven lowering で 7.8s → ~6.5s)。**この suite は wall ではなく CPU 律速**: テスト時間の総和 (149.4s CPU は demand-driven lowering 前の値) / 20 コア が下限で、実測 wall はそこに張り付いている。**ビルド時間は別問題で、そちらの方が大きい — BUILD-PERF を見ること**。最長の単一テストも 2.11s (`example_consistency` shard_8) なので critical path 律速でもない。したがって**並列度を上げる策は効かず、効くのは CPU そのものを減らす策だけ。換算レートは 20:1** (CPU を 20s 削って wall 1s)。

  クレート別 CPU:

  | | CPU | テスト数 | 平均 |
  |---|---|---|---|
  | compiler | 78.3s (52%) | 408 | 192ms |
  | interpreter | 55.5s (37%) | 984 | 56ms |
  | frontend | 15.6s (10%) | 583 | 27ms |

  - **`compiler::consistency` + `example_consistency` が suite CPU の 45%** ★★★ — 46.5s (317 テスト) + 22.0s (14 shard) = 68.5s。`consistency` の分布は二峰性で、**lite パスで完結するテストと full core にフォールスルーするテストで 1 桁違う**。**2026-08-20 のフロントエンドパス共有で 46.5s → 34s / 22.0s → 16s 相当 (下記)** まで下がったが、残りは AOT の codegen + link + spawn (cache warm でも ~50ms/テスト) と JIT の native compile が本質的なので、ここから先はバックエンド実行そのものの削減になる。
    **「lite → full 二重パス」は 2026-08-18 に潰したが、それ自体はコストではなかった**と分かったので記録しておく: `assert_consistent` の let-chain は**最も安いレーン (no-core の tree-walker) で短絡する**ので、stdlib を使うソースが捨てられる AOT codegen / link / spawn まで到達することは元から無かった。捨てていたのは parse + no-core 型検査 ~2ms だけ。実際に効いたのは同時に入れた**フロントエンドパスの共有**の方 (下記)。
  - **with-core のフロントエンドパスがレーンごとに独立** ★★ — 1 テストが core を使うと、tree-walker / AOT / JIT / IR VM がそれぞれ `core/std/*.t` を integrate + 型検査する。**tree-walker と IR VM の 2 レーンは 2026-08-18 に 1 パスに統合済み** (`ast_lanes`、AST に interior mutability が無いので型検査済み `File` を両方に渡せる。ついでに lite 試行の失敗も memo 化 — 以前は失敗を記録せず毎回引き直していた)。**実測 `consistency` 52.7s → 46.5s CPU (-12%)、suite 156.7s → 149.7s CPU / wall 7.8s → 7.3s**。残る 2 レーン (AOT は `compile_file` がパス受け取り、JIT は `run_source` がソース受け取り) を畳むには**ライブラリ側に「型検査済み program を受け取る」入口**が要る。~~残る 2 レーン~~ **解消 (2026-08-20)**: `compile_checked_program` + `CheckedProgram` で 4 レーンが 1 フロントエンドパスを共有 (consistency -27% / example -29% CPU。詳細は完了済み節)。
  - ~~**AOT レーンが stdlib 全体を毎回 codegen している** ★★★~~ —
    **解消 (2026-08-19)**: `lower_program` の本体 lowering を需要駆動にし
    (reachable closure のみ)、codegen / JIT は `Module::reachable_from` で
    reachable-from-main だけを compile。lowering ~3x、フル AOT ~2x
    (debug, opt=none)。詳細は 完了済み節 2026-08-19。
  - **core module のロードが 1 プロセスあたり 27ms** ★★★ — trivial プログラムを空 core dir と比べた実測 (2026-08-18、debug ビルド): **33.5ms → 6.1ms**。nextest は 1 テスト 1 プロセスなので、interpreter の 984 テストはそれぞれこれを払う = ~26s CPU ≈ wall 1.3s。内訳は 2026-08-15 時点の計測 (integrate ~43% 削減が landing する前) で `integrate_modules` 11.3ms / `execute_entry` の context 構築 5.2ms / stdlib 40 impl block の型検査 2.5ms / その他の型検査 1.1ms。

    **測って分かった否定的な結果を 3 つ記録しておく**: (1) **free function の body は既に user 分しか検査していない** (`take(user_func_count)`) ので「stdlib 本体を型検査しない」で削れるのは impl block の 2.5ms だけ。しかも**型検査器は body を書き換える** (`?` の desugar、`Display` の `to_str` 挿入) ので、stdlib の body を検査しないと**書き換え前の AST がバックエンドに流れる** — 今の stdlib は `?` も補間も使っていないので通ってしまい、使った日に壊れる罠になる。(2) `remap_symbol` の memo 化 (module symbol → main symbol を Vec でキャッシュ) は**効果ゼロ**だった。integrate の時間は文字列ハッシュではなく AST を pool に複製する作業そのもの。(3) **「型検査済み core をプロセス内で使い回す」は unit テストには効かない** — nextest は 1 テスト 1 プロセスなので、そもそもプロセス内に 2 回目の呼び出しが無い。
    したがって残る手は (a) stdlib を使わないテストを `test_program_no_core` に寄せる (実測: `test_program` を no-core にすると interpreter の 879 テスト中 **797 が通り**、その binary は 2.3s → 1.3s。ただし stdlib 同居時の回帰を見なくなる = coverage を実際に落とす)、(b) **プロセスを跨いで**型検査済み core を再利用する (INCREMENTAL-COMPILATION 側の仕事。`File` が `Rc` を持つので素朴な in-memory memo 化はできない — 別スレッドから clone すると refcount が壊れる)、(c) 1 プロセスで core を複数回ロードしている `consistency` を直す — **解消 (2026-08-20)**: 4 レーンが 1 フロントエンドパスを共有するようになり、AOT / JIT レーンが毎回 core をロードし直す重複が無くなった (consistency -27% CPU)。
  - **プロセス起動が ~5ms × 1999 ≈ 10s CPU (約 7%)** ★ — 起動フロアの実測は空 core dir の trivial 実行 6.1ms。nextest は 1 テスト 1 プロセス。テストを機能別に束ねれば減るが、失敗の切り分けと引き換え。
  - ~~`serial_test` (`oop_tests.rs`) の並列化~~ — **効果ゼロと分かったので却下 (2026-08-18)**。`#[serial]` が付いているのは 8 テストで合計 **0.193s CPU (suite の 0.12%)**、1 本 18〜34ms と既に起動フロア。しかも `serial_test` のロックはプロセスローカルなので、**nextest では各テストが別プロセスに散る = 元から直列化していない**。
- **65. frontend リファクタリング** — (a)〜(g) は完了。残: doc コメント拡充、プロパティベーステスト追加。
- **property test の generator が仕様と drift しないか** — `valid_identifier()` は lexer に問い合わせる形にした (2026-08-10)。他の generator (リテラル / 演算子) はまだ手書きなので、同種の drift が起きうる。
- **26. ドキュメント整備** — 残: API リファレンス、advanced topics。

## 検討中の機能

* FFI — P1 (静的 FFI、`from`/`as`) 完了 (2026-08-16、[`FFI_PLAN.md`](FFI_PLAN.md))。
  P2 (動的ロード / dlopen builtin) は未着手
* AOT ランタイムの Rust 化 — R0+R1 完了、R2 (extern 一般化 = FFI_PLAN P1)
  も完了 (2026-08-16、[`RUNTIME_PORT.md`](RUNTIME_PORT.md))。R3 (str 系の
  toylang 化) と R4 (f64 整形等) は計測で中止条件に該当 (interpreter
  ~20 倍〜~1000 倍遅延) し、Layer 1 に残すのが確定。R4 の byte 一致
  テスト固定のみ実施済み。
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
- 合計 **1999 テスト** (100% 成功、2026-08-18 時点)。
- 内訳: interpreter unit + integration、frontend unit、compiler e2e + consistency。後者は interpreter / JIT / AOT の 3 経路一致を保証する。
- テスト実行はワークスペース全体で **~6.5s** (warm、20 コア。2026-08-19、
  AOT demand-driven lowering で 7.8s → 6.5s。内訳と削り代は TEST-PERF、
  ビルド時間は BUILD-PERF)。`compiler/build.rs` が `toylang_rt` を rustc で
  staticlib pre-build し、リンク結果は `TOY_LINK_CACHE_DIR` で
  content-addressed にキャッシュされる (キャッシュが効くにはコード生成が
  決定的である必要がある — `compiler/tests/reproducible_build.rs` が pin)。

### 既知の不具合

- **f64 の print / 補間が 3 バックエンドで食い違う** — ~~AOT は C ランタイムの
  `%g` / `%.1f` (`emit_f64`)、interpreter / JIT は Rust の `Display`。~~
  **解消 (2026-08-16、RUNTIME-PORT R1)**: ランタイムが `toylang_rt`
  1 本になり、整形も `Display` (整数値は `.0`) に統一された。AOT の出力は
  `0.3` → `0.30000000000000004`、`1.23457e+06` → `1234567.75` に変わった
  (精度が上がる方向の仕様変更、`docs/language.md` の Output 節に明記)。
  `f64_display_agrees_across_backends` が 3 者一致を pin する。
- **`if` / `elif` の条件が型検査されない** — ~~`if 42u64 { ... }` が通る。~~
  **解消 (2026-08-16)**: `visit_if_elif_else` が条件を
  `check_expr_located` し bool を要求するように。この変更で 2 つの
  隠れバグが露呈し、両方修正:
  (1) `resolve_numeric_types` / `visit_compare_binary` に
  Generic↔Identifier 同一シンボル (と Generic↔Generic 同一パラメータ)
  の arm が無く、dict.t の `existing == key` が E0001/E0002 で弾かれた
  (条件未検査が隠していた) — 専用 arm を追加。
  (2) **generic 関数の 2 回目以降のインスタンス化が最初の実体に解決
  される既存バグ** — `id(1u64)` の後に `id("hello")` を呼ぶと u64 版を
  str handle で呼んで全バックエンドでゴミを返す。インスタンスが
  function_index に bare name で登録されていたのが原因で、
  `declare_function_anon` 化 + `resolve_call_target` の generic 優先に
  修正 (`toy_same__str` が生成されず fn#158 が呼ばれていた)。
  `val x = <generic call>` の型推論 (value_scalar) も template の
  return 型を置換する形に。付随して `struct_null_test.t` (u64 に
  `.is_null()` を呼ぶ壊れた example) を `ptr` +
  `__builtin_ptr_is_null` に修正し AOT_UNSUPPORTED から外し、
  `trait_basic.t` も generic 修正で AOT が通るようになったので同様に
  リストから除去。テスト: `if_conditions_must_be_bool` /
  `generic_equality_is_instantiated_per_type` /
  `generic_functions_instantiate_per_type_argument`。

以下 4 件は `docs/language.md` を実装と突き合わせた監査 (2026-08-18) で
発見。ドキュメント側は同日のコミットで実態に合わせたので、残るのは
実装をどう直すかの判断。

### パーサーの既知制限事項
- bare `self` 非対応 — `self: Self` / `&self` / `&mut self` のいずれかを書く。
- `else if` 非対応 — `elif` を使う。
- `val` はキーワードなのでパラメータ名に使えない。
- 関数のネスト定義 (`fn` の中の `fn`) は不可 — closure (`fn(x: T) -> R { ... }`) を使う。
- デフォルト引数 / 名前付き引数は不可 (`f(a: u64, b: u64 = 1u64)` / `f(a: 1u64)`)。導入予定も無い。
- `extern fn` の generic params は parser では受理されるが、JIT / AOT が per-instance シンボル名を持たないため interpreter でのみ動く (`#195b`)。
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省いているのはこのため)。
- 3-part qualified call (`std::math::abs(x)`) は parser が **last 名だけを採る**。名前が一意なら結果的に解決するが、意図した経路ではない (`#185残`)。
