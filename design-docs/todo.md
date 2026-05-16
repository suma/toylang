# TODO - Interpreter Improvements

## 完了済み ✅

> 詳細は git log / commit message を参照。本セクションは直近マイルストーンの 1 行サマリのみ保持する。

### 2026-05-17
- **`loop {}` + comparison chain** — 小規模 syntactic-sugar 2 件を parser-level desugar で実装。**`loop { BODY }`** → `while true { BODY }` に desugar。`@label: loop { ... }` / `break @label` / `break` も `while true` 経由でそのまま動作。**comparison chain** (`a < b < c`) → `{ val __cmp_0 = b; a < __cmp_0 && __cmp_0 < c }` に desugar。各中間オペランドを synthetic temporary に入れて side-effect を 1 回に抑制。`<` / `<=` / `>` / `>=` の任意連鎖をサポート (例: `0u64 < x <= 10u64 < 100u64`)。`==` / `!=` は含めない (precedence 曖昧)。interpreter unit 8 件 + compiler consistency 4 件追加。1454 → **1466 tests pass** (+12)。
- **GENERIC-ENUM-MATCH-HOF (96残-後半 解決)** — `Option::map<U>` / `Result::map` / `Result::map_err` を stdlib に追加し、generic enum match arm 内での HOF (higher-order function) 呼び出しを type checker で動作させた。根本原因は `module_integration.rs` の `remap_type_decl` が `TypeDecl::Function` を remap していないため、`core/std/option.t` / `core/std/result.t` 内の `fn (T) -> U` closure 型の generic parameter symbol (`U`) が `main_string_interner` と `module_string_interner` でずれ、method call の generic substitution が構築できない問題だった。`remap_type_decl` に `TypeDecl::Function(params, ret)` 処理を追加して解決。`collect_substitution` の symbol 比較は元の `string_interner.resolve()` ベースに戻した (u32 value 比較は `string_interner` の同一文字列 = 同一 symbol 値を保証しないので fallback では不足)。`type_decl.rs` の `substitute_generics` に `Enum` 処理も追加（戻り値の generic 置換用）。テスト: `generics_tests.rs` に `test_option_map_hof` / `test_option_map_none` / `test_result_map_hof` / `test_result_map_err_hof` 4 件追加。1466 → **1470 tests pass** (+4)。

### 2026-05-10
- **DEBUG-BUILTINS Phase A+B+C** — テスト性 / デバッグ向け parser-level macro 群を追加。**Phase A**: `__builtin_source_file()` / `__builtin_source_line()` / `__builtin_source_column()` を parser-level で literal に substitute (call-site の line/col は `current_source_location()`、file path は `Parser::set_source_file` で entry point から threading、default `"<source>"`)。**Phase B**: `__builtin_dbg(EXPR)` を `{ val __dbg_n = EXPR; println("[file:line] text = ".concat(to_string(__dbg_n))); __dbg_n }` に desugar。EXPR の原文は `Parser::source_substring(byte_range)` で input buffer から直接切り出し、AST 再レンダリング不要。**Phase C**: `assert_eq(a, b)` / `assert_ne(a, b)` を `{ val l=a; val r=b; assert(l OP r, "header".concat(to_string(l)).concat(...).concat(to_string(r))) }` に desugar、panic message に source line + 左右の値を含む。すべて parser-level rewrite なので type checker / 3 backend は変更不要 (interpreter / AOT は普通の AST として動作、JIT は `assert` の dynamic message で fallback)。`SourceLocation` を `Copy` 化 (内部 32-bit 3 個なので tiny)、`compiler_core::CompilerSession::parse_program_with_source` を新設して filename を threading、`interpreter::run_source` から呼ぶ。tests: `interpreter/tests/debug_builtins_tests.rs` 16 件追加 (source_*, dbg, assert_eq/ne の happy + failure path、nested dbg counter、negative path)。1438 → **1454 tests pass** (+16)。

### 2026-05-09
- **IF-VAL (`if val` / `while val`)** — pattern-binding 条件分岐 + ループ。Rust の `if let` 相当だが、toylang は `let` ではなく `val` を不変束縛キーワードに使うので construct 側も `val` で揃えた。**pure parser desugar** — `if val PAT = EXPR { THEN } else { ELSE }` → `match EXPR { PAT => THEN, _ => ELSE }`、`while val PAT = EXPR { BODY }` → `while true { match EXPR { PAT => { BODY; continue }, _ => break } }`。AST / type checker / interpreter / AOT / JIT すべて既存の match / while を再利用、追加 backend 作業ゼロ。**no-else 形** (`if val PAT = EXPR { THEN }`) は THEN を synthetic `val __ifval_dummy_<n> = ...` で wrap して Unit 化、else arm は empty block (match arm body の empty block を Unit 扱いするように pattern_match.rs も小修正)。labelled `@outer: while val PAT = ...` も label 伝播。**AOT 制約**: function-call enum scrutinee (`while val Some(x) = func(i)`) は match_lowering の MVP 制約で未対応 — method-call 形 (`c.next()`) で書く必要あり (interpreter は OK)。3-way consistency 3 件 + interpreter unit 9 件追加。1432 → **1444 tests pass** (+12)。

### 2026-05-08
- **LABEL (labelled break / continue)** — `@label: while/for ...` + `break @label` / `continue @label` を 3 backend (interpreter / AOT / JIT) で実装。`@` token 新規追加 (Rust 風 `'label:` は char literal `'a'` との lexer 曖昧性で見送り)、AST: `Stmt::Break(Option<Symbol>)` / `Continue(Option<Symbol>)` / `While(Option<Symbol>, ...)` / `For(Option<Symbol>, ...)`、pool に `loop_label` 列追加。Type checker は `loop_label_stack` で undefined label / break-outside-loop を新規バリデーション (今までの 2 つは silent pass だったので副次的な型安全強化)。AOT は `loop_stack: Vec<(Option<Symbol>, ...)>`、JIT は `LoopFrame.label` で rev-find resolve、複数ループ跨ぎ break で with/drop scope cleanup を target depth まで遡って発行。**副次 fix**: AOT for-range loop に dedicated `step` block を追加 — 既存実装は increment が body 末尾に inline されていて、`continue` が header に直接 jump すると increment を skip して infinite loop になっていた (今まで integer-range for + continue を使う test が存在しなかったため未発覚)。iterator-form `@label: for x in iter { ... }` は desugar 後の synthetic `while true` にラベル伝播。3-way consistency 2 件 + interpreter unit 7 件追加。1423 → **1432 tests pass** (+9)。
- **OP-OVERLOAD 完全コレクション** (`286a613` + `04eeb25` + `ae4d358` + `bda35dc`) — 同型 struct ペアの全 binary + unary operator を user method dispatch 化: `eq` (`==` / `!=`) / `lt` `le` `gt` `ge` (順序比較) / `add` `sub` `mul` `div` `rem` (`+ - * / %` + 複合代入 `+=` 等) / `bitand` `bitor` `bitxor` `shl` `shr` (ビット演算) / `neg` `bitnot` `not` (単項 `- ~ !`)。3 backend、let-rhs context only。`&& ||` は意図的 scope 外 (short-circuit)。**MVP 制約**: chained position (`a + b + c`)、binary struct literal operand (`a & Bits { v: 1 }`) は follow-up。1326 → **1423 tests pass** (+97 cumulative)。
- **STRING-NOMINAL + API 全 Phase + STR-INTERP-COMPOUND** (`682331c` + `a0cae0d` / `e9b2ee7` / `7335421` / `6a0fa3c` + `5c1038c` + `d36a063`) — `String` を flat-layout 独立 struct (`{ data, len, cap, elem_size }`) に nominal 化、`type String = Vec<u8>` alias 削除。byte-specific method + 拡張 trait (Substring / Trim / CaseConvert / Concat / Contains / Split / ToString inherent) を `impl String` に集約。`type char = u32` + `push_char` の UTF-8 encoding (RFC 3629)。`"{struct_val}"` / `"{tuple}"` / nested compound 補間を AOT で動作 (新 IR `ConstStrBytes` で frontend interner mutability 不要、binding tree ベースの recursive `emit_struct_format` / `emit_tuple_format`)。
- **AOT lower 系の汎用拡張** (`a57d9fe` + `07a895d` + `ec83086`) — `__builtin_sizeof` の compound (struct/tuple/enum) 対応 (`compute_byte_size` 新設)、`__builtin_ptr_write/read` の compound 値 per-leaf 展開 (`compute_leaf_layout` 新設) → `Vec<T>` で T compound 全般に対応 + `Vec<Vec<u8>>` / `Vec<String>` の AOT round-trip 動作。`alias_resolution` の expression loop に `Expr::AssociatedFunctionCall` qualifier rewrite (`String::from_str(...)` 等)。

### 2026-05-07
- **ITER-PROTOCOL-TRAIT 完了** — generic trait declarations (`trait Foo<T, U, ...>`) と impl (`impl Foo<i64> for Counter`) をサポート。AST: `TraitDecl.generic_params: Vec<DefaultSymbol>` + `ImplBlock.trait_type_args: Vec<TypeDecl>` 追加。Parser: trait 名の後の `<T, U>` を消費 (既存 `parse_generic_params` 流用)、`impl Trait<...> for Type` の `<...>` を `trait_type_args` として保存、trait method body 内の `T` 参照は `parse_type_declaration_with_generic_context` 経由で Generic として認識。Type checker: `context.trait_generic_params` に各 trait の generic params を登録、`check_trait_conformance_with_args(trait_type_args)` で trait 側 method signature を `T -> trait_type_args[i]` で substitute してから impl 側と比較 (param/return type 両方)。impl 側の trait_type_args 数が trait の generic_params 数と一致しない場合は arity error。新規 visit メソッド: `visit_impl_block_with_trait_args` (default は `visit_impl_block` に forward)、`visit_trait_decl_with_generics` (default は `visit_trait_decl`)。`context.pending_trait_type_args` で `visit_impl_block_with_trait_args` → `visit_impl_block_impl` 間に trait_type_args を引き渡し。stdlib `core/std/iter.t` を documentation-only から `pub trait Iterator<T> { fn next(&mut self) -> Option<T> }` の実宣言に更新 (user は `impl Iterator<i64> for Counter` を直接書ける)。test: `interpreter/tests/trait_tests.rs::generic_traits` モジュールに 6 件追加。1320 -> **1326 tests pass**。残: trait-bounded generic API (`fn first<I: Iterator<i64>>(iter: I)`) の bound check は別タスク。
- **STR-INTERP-INTERP-JIT 完了** — interpreter 旧 JIT (`interpreter/src/jit/`) でも string interpolation を完全動作 (silent fallback ではなく実 JIT compile)。新規 `ScalarTy::Str` (i64 ポインタとして storage、cranelift I64 へ map)、新規 14 個のランタイムヘルパ (`jit_string_literal` / `jit_str_concat` / `jit_to_string_{i64,u64,f64,bool,str,i8,u8,i16,u16,i32,u32}` / `jit_print_str` / `jit_println_str`) を `interpreter/src/jit/runtime.rs` に実装 — heap layout は AOT / compiler JIT と pointer-uniform (`[bytes][NUL][u64 len LE]`、戻り値ポインタは len field を指す)。eligibility (`interpreter/src/jit/eligibility.rs`) に `Expr::String → ScalarTy::Str` 推論、`__builtin_to_string` の primitive arg 受理、`s.concat(t)` の str/str 検査、`Print/Println` での Str 受理を追加。codegen (`interpreter/src/jit/codegen.rs`) で各々を runtime helper への direct call に lower。**str は function 境界禁止** (`resolve_param_ty` で `TypeDecl::String` を `None` 返却、param/return として使えない) — interpreter ↔ JIT の Object lifecycle 整合性確保のため。method-name 比較は `concat_sym()` thread-local キャッシュで interner reference 不要。新規 example `interpreter/example/string_interpolation_jit.t` (str.len() 等まだ未対応の builtin を避けた版)、jit_integration test 2 件追加 (`string_interpolation_jit_matches_interpreter` / `string_interpolation_jit_logs_compiled_main`)。1318 -> **1320 tests pass**。
- **ITER-PROTOCOL-NON-IDENT-EXPR 動作確認完了** — `for x in Counter::new(5) { ... }` (AssociatedFunctionCall 直接) は実は既に 3 backend 全部で動作。`for x in make_counter(5) { ... }` は AOT で失敗するが、原因は make_counter 自体の compound-returning function body の expression-position lowering (一般的 AOT 制約) で、iterator-protocol とは無関係。
- **ITER-PROTOCOL-AOT 完了** — `for x in EXPR { body }` を AOT compiler でも動作させた。3 つの実装変更: (1) parser desugar (`frontend/src/parser/stmt.rs::desugar_for_in_iterator`) で EXPR が bare identifier のとき synthetic `var __iter_for_<n>` temporary を skip して直接 `iter.next()` を呼ぶ (AOT の let-rhs path は struct binding のエイリアスコピーをサポートしないため、`var __iter = iter` の形は失敗する)、`&mut self` writeback で user の original binding が正しく mutate される。(2) `compiler/src/lower/let_lowering.rs::lower_let` の MethodCall arm に `&mut self` writeback サポート追加 — receiver leaves と compound `&mut T` arg dests を struct/tuple/enum 各 return path の dests に append、`self_writeback_types` 経由で plumbing。(3) `compiler/src/lower/match_lowering.rs::classify_match_scrutinee` に enum-returning method call の scrutinee サポート追加 — fresh enum storage を allocate し CallEnum (writeback 込み) で populate、`MatchScrutinee::Enum(storage)` を返す。3-way consistency 5 件追加 (`iter_protocol_basic/break/continue/zero_iterations/nested_round_trip`)。1313 -> **1318 tests pass**。
- **STR-INTERP Phase 2 (AOT + cranelift JIT)** — string interpolation を AOT + cranelift JIT (compiler-side) でも完全動作。新規ランタイムヘルパ: `toy_str_concat(a, b) -> str` と `toy_to_string_{i64,u64,f64,bool,str,i8,u8,i16,u16,i32,u32}` (all in `compiler/runtime/toylang_rt.c`)。新規 IR: `InstKind::StrConcat { a, b }` と `InstKind::ToString { value, value_ty }`。lower: `BuiltinFunction::ToString` を `InstKind::ToString` に、`MethodCall(str, "concat", _)` を `InstKind::StrConcat` に変換。`value_scalar` を拡張して `s.concat(t)` chain (literal receiver / nested method call) と `__builtin_to_string(...)` の戻り値を Type::Str として推論できるように。codegen (`lower_inst.rs`) で各 InstKind を対応する `toy_*` runtime helper への direct call に lower。`compiler/src/jit.rs` には JIT-side mirror として同形のヘルパ (`toy_str_alloc` / `toy_str_concat` / `toy_to_string_*`) を Rust で実装し JITBuilder.symbol で登録 — 同じヒープ layout (`[bytes][NUL][u64 len LE]`、戻り値ポインタは len field を指す) なので AOT と pointer-uniform で `.rodata` strs と相互運用可能。3-way consistency 6 件追加 (`string_interp_*_round_trip`)。1307 -> **1313 tests pass**。残: interpreter JIT (`interpreter/src/jit/`) の対応 (silent fallback で動作するので blocker ではない)。

### 2026-05-06
- **STR-INTERP Phase 1 (interpreter)** — `"hello {name}, sum={a + b}"` 形式の string interpolation。lexer で `{...}` を検出して `Kind::InterpolatedString(parts: Vec<StringPart>)` を発行、parser-level で `.concat() + __builtin_to_string()` chain に desugar (`Token::insert_token` で synthetic token を current position に push、parse_postfix で chain を parse)。`{{` / `}}` を literal `{` / `}` にエスケープ (Rust 規約)。任意型 (primitives + structs/enums) を補間可能。新規 `BuiltinFunction::ToString` builtin (`Object::to_display_string` 経由)。**副次 fix**: 既存の `visit_builtin_method_call` が `StrConcat` / `StrSubstring` / `StrTrim` / `StrToUpper` / `StrToLower` / `StrContains` / `StrSplit` で全て Unit を返していたバグ (catch-all `_ => Ok(Unit)` がカバーしていた) を修正、各 method の宣言通りの戻り型を返すように。JIT は `__builtin_to_string` を eligibility で skip (silent fallback)、AOT は lower で reject (cleanly with message)。詳細は `STR-INTERP-AOT` 参照。
- **ITER-PROTOCOL Phase 1 (interpreter + JIT)** — `for x in EXPR { body }` を parser-level で `while + match Option::Some(x)/None` に desugar。EXPR の型が `fn next(&mut self) -> Option<T>` を持てば動作 (structural / duck-typed)。`trait Iterator<T>` 宣言は generic-trait 未対応のため providing 不可、`core/std/iter.t` は documentation-only。`evaluate()` に `Expr::Block` を追加 (match arm body が block の場合の runtime error も同時 fix)。Range-based for-loop (`0..N` / `0 to N`) は既存の `Stmt::For` 整数 fast path を維持。AOT 対応は follow-up (下記 ITER-PROTOCOL-AOT 参照)。

### 2026-05-05
- **NUM-LIT-SEPARATORS** — 数値リテラルに `_` 区切り (`1_000_000u64` / `0xDEAD_BEEFu64` / `3_141.592_653f64`)。lexer-only。
- **CLOSURES Phase 1〜8** — frontend + 型 checker (P1/P2)、interpreter (P3)、AOT direct/indirect/capturing/narrow int/return/struct field 格納 (P5a〜P8)、`fn (T) -> R` 関数型構文。
- **CLOSURES Phase 7 (partial)** — stdlib `Option::map` 等を着地予定、generic-enum match arm unification ブロックで部分着地のみ (続きは未実装側)。
- **DOCS-2026-05-05** — `docs/language.md` + `compiler/README.md` の最新化。
- **NUM-W-JIT** — 狭い数値型の JIT cranelift codegen 4 phase 全完了。
- **ZERO-MEMCOPY-FIX** — `HeapManager::copy/move/set_memory` で size==0 が false を返す bug の libc parity fix。
- **TYPE-ALIAS 周辺整備** — forward-ref、cross-module resolution、`String = Vec<u8>` 統合、`Vec<u8>` API port、`type char = u8` 整備。

### 2026-05-04
- **エスケープシーケンス** — `\u{HEX}` Unicode、`\xHH` hex、char literal `'a'` (u32)、string literal escape の対称化。
- **TYPE-ALIAS / GENERIC-TYPE-ALIAS** — `type Name = Target` parse-time 即時展開、`Pair<T> = Box<T>` 形 generic alias、`Struct(name, [])` 等価性緩和。
- **GENERIC-RAII (interpreter + AOT)** — user struct の `impl Drop` を scope-bound auto-call。
- **ALLOCATOR Phase 5** — `trait Drop` + `AllocatorBinding`、`with allocator = Arena::new() {...}` / `FixedBuffer::new(cap) {...}` の temporary-form auto-cleanup。
- **REF-Stage-2** — `&T` / `&mut T` の field / tuple / nested chain / array index borrow + writeback、escape rule の構文 reject、`val` からの `&mut` borrow reject。
- **121-Phase-B-rest** — arena / fixed_buffer 系 native runtime + `__builtin_arena_drop`、`with` body の早期 exit cleanup、heap_alloc dispatch via active stack。
- **TEST-PERF-lazy-core** — dependencies opt-level=3 + lazy core auto-load でテストスイート高速化。
- **STRING stdlib** — `core/std/string.t::String` heap-managed byte buffer、`eq` / `clear` / `push_char` 拡張、read-only method を `&self` 化。
- **CONCRETE-IMPL Phase 1 / 2-interp / 2b** — `impl Trait for Generic<T>` の concrete type args capture と 3 backend dispatch。
- **NUM-W (Phase 1〜6 + AOT + AOT-pack + signed-hash)** — 狭い数値型 (u8/u16/u32/i8/i16/i32) のインタプリタ + AOT 完全対応、stdlib Hash / Display 配線、homogeneous scalar element 配列 packing と専用 print helper 12 個。
- **DICT 系まとめ** — DICT-AOT-NEW Phase B/C (`Dict::new()` AssociatedFunctionCall + per-monomorph generic substitution + `__builtin_sizeof` AOT)、cross-module Option contamination 修正、`return inside while` 伝播 fix、typed_slots realloc migration test。

### 2026-05-03
- **VEC-collection** — `core/std/collections/vec.t` user-space `Vec<T>` (push / from_str / geometric grow)。
- **STR-LEN-O1 / STR-PTR-LEN** — AOT で `__builtin_str_len` を O(1) 化、`.rodata` per-literal layout を `[bytes][NUL][u64 len LE]` に拡張、`as_ptr` / `len` trait 化。
- **121-Phase-B-min / Phase-A** — Allocator builtin 群、heap / pointer builtins (default global allocator)。
- **MUT-SELF-Stage-1** — `&mut self` impl-block method receiver (frontend + AOT + stdlib migration)。
- **96残-前半** — match の deep exhaustiveness check (nested enum patterns)。

### 2026-05-02 以前 (大きめのマイルストーン)
- **#183 コンパイラ MVP** — IR / cranelift-object backend で実行ファイル生成。struct / tuple / enum / generic / trait / DbC / allocator builtin / 配列 / cast / f64 / panic-assert / print まで網羅。`compiler/src/lower/` を 20 ファイルに分割完了 (Phase Z2〜Z20)。Phase A〜Y 系列 (enum + match A1/A2、payload 拡張 G〜O、generic struct/関数 monomorphize K/L、method dispatch R/R3、配列 S/Y/Y2/Y3、tuple Q1/Q2、文字列 T、compound-returning method W、stdout 比較 V) 含む。
- **per-module function namespacing (#193 / #202)** — IR + 型 checker + interpreter 実行時の関数テーブル分離。
- **コア・モジュール auto-load (#193)** — `<repo>/core/` を起動時に再帰的にロード。
- **Extension trait 全 backend 対応 (#191、Step A〜F)** — primitive 型への trait impl、prelude → `core/std/{i64,f64}.t` 化、`BuiltinMethod` 統合。
- **Math externalisation (#190、Phase 1〜4)** — `extern fn` 経由の f64 math intrinsic。3 backend dispatch + libm import。
- **Option / Result stdlib (#203)** — `core/std/option.t` / `core/std/result.t` (`Option<T>` / `Result<T, E>`) と AOT enum receiver dispatch (#201)、JIT skip 詳細化 (#204)、AOT で str enum payload 許可 (#205)、ドキュメント整備 (#206)。
- **Value/Reference 分離 Phase 1〜5** — `Value` enum 導入、Environment / EvaluationResult / operators の Value 化。fibonacci -8% / for_loop -12% 高速化。
- **panic / assert / DbC (#166〜#175)** — `requires` / `ensures` 全 backend 対応、`assert` JIT、`Object::Struct` field key を symbol 化、`PropagateFlow` 除去。
- **言語仕様拡充 (#161〜#165)** — f64 サポート、`%` mod、複合代入、タプル JIT (flat scalar)、ネスト val/var タプル分解、match arm guard。
- **#184 Trait + impl** / **#170 top-level const** / **#169 言語リファレンス `docs/language.md` 新設**。

## 未実装 📋


STR-INTERP-COMPOUND-EXTEND-ENUM. **enum 値の `{Option::Some(v)}` 補間 (AOT)** — `d36a063` で struct + tuple + nested compound は landed。残: enum (Option / Result / user-defined) の to_string。tag local を読んで variant 別の format を出すために cranelift block の if-elif chain (lower_short_circuit / lower_if_chain 系) を借用する必要、payload は variant ごとに分岐。format 例: `Option<i64>::Some(99)` / `Option<i64>::None` / `Result<i64, str>::Ok(42)`。interpreter は `Object::to_display_string` で既に動作。優先度: 中。


NUM-W-AOT-pack-Phase3. **狭い数値型 AOT の compound element packing 続き**: NUM-W-AOT-pack Phase 1 (`747146c`) で homogeneous scalar 配列の packing は完了。残: compound element 配列 (`[PackedRgba; N]` で `struct PackedRgba { r:u8, g:u8, b:u8, a:u8 }` が現状 32 バイト消費、本来 4 バイト) の tighter layout。実装には `ArraySlotInfo` を per-leaf strides ベースに refactor、もしくは `ArrayLoad/Store` に element_index + leaf_offset_const の 2 値を取らせる IR 拡張が必要。優先度: 低 (機能は動くがメモリ効率の改善のみ)。

195b. **`extern fn` 生 monomorph 化** (`#195` の続き): 現状ジェネリック extern は interpreter の type-erased registry でだけ動く。JIT/AOT で動かすには call site ごとに mangled extern symbol (`__extern_id__u64`, `__extern_id__i64` など) を emit し、対応する monomorph 実装を Rust 側に登録する仕組みが必要。優先度: 低 (現状必要なケースなし)

185残. **3+ part qualified call** (`std::math::abs(x)` 形式) — 現状は `import std.math` (または auto-load) してエイリアス `math` 経由でしか呼べない (parser が 3-part path で last 名のみを採用するため)。優先度: 低 (auto-load のおかげで実用上の不便は限定的)

JIT-enum-1 (residual). **JIT enum サポートの残作業** — JE-2〜JE-6 まで完了済み (Option/Result + ユーザ enum の constructor / match / 関数境界 / receiver method dispatch すべて JIT 対応; 詳細は git log)。残: ネストした generic enum payload (`Option<Option<T>>`)、enum-typed struct fields の JIT layout、payload に struct/tuple を持つ enum。優先度: 低 (Option/Result の stdlib メソッドは既に JIT で動く)。

160. **タプルの追加 JIT 対応** — フラットなスカラーtupleの param / return / TupleAccess / destructure / tuple-returning call は完了 (`#163`)。残: ネストタプル (`((a,b),c)`) と tuple-of-struct を JIT codegen で扱う (`ParamTy::Tuple(Vec<ScalarTy>)` を tree 構造に拡張する 100+ 箇所の refactor)、inline tuple literal を call argument として渡せるようにする
159. **JIT Phase 2 拡張** — Phase 1 / 2a-2h / 2c-2 / 2d-2/3/4 / 2e (allocator stack) / `__builtin_fixed_buffer_allocator` / `with` 内の早期 exit はすべて完了。残: generic 構造体 / メソッド (`struct_layouts` を type-args 別に持つ refactor)。サポート範囲のまとめは `JIT.md`。**diagnostic 状態 (`98e2249`)**: generic struct を踏むと `JIT: skipped (... struct literal references a generic struct (JIT does not yet model generic struct values; see #159))` が出るので、diagnostic から直接 todo entry に飛べる。`jit_skip_reason_for_generic_struct` test で wording を pin。
121-Phase-B-rest-leftover. **AllocatorBinding wiring + default_allocator API 整理** — Phase B-rest Item 1+3 + Item 2 cleanup + arena_drop builtin、それに **inline raw builtin allocator auto-drop (Item 1)** = `121-PHASE-B-LEFTOVER-1` (`with allocator = __builtin_arena_allocator() { ... }` の自動 drop) も完了。**残**: (2) `AllocatorBinding::Generic/Local/Ambient` の lower 配線 (perf 最適化のみで観察可能な振る舞い変化なし、IR 上の placeholder)、(3) `__builtin_default_allocator()` の戻り型を `Allocator` から `u64` に変えて生比較 (`!= 0u64`) を許可するか — 現状は `!= __builtin_default_allocator()` の方法しかない。priority 低 (cosmetic / perf / API 整理)。
~~96残-後半. **Enum/match — Option/Result の追加 stdlib メソッド** — `Option::map<U>` / `and_then` / `Result::map_err` 等を追加するには **closures** が必要 (要設計議論)。toylang は closure 型をまだ持たないので、その前段の検討が必要。前半 (深い網羅性解析) は `273580c` で完了。~~ → **2026-05-17 に解決** (`TypeDecl::Function` の `remap_type_decl` 対応 + `substitute_generics` の `Enum` 処理)。`Option::map` / `Result::map` / `Result::map_err` を stdlib に追加済み。

CONCRETE-IMPL-Phase-2c. **annotation hint threading + 型 checker registry refactor** — Phase 1 / 2-interp / 2b は完了 (パーサ + AST + interpreter dispatch + AOT compiler dispatch、3-way `assert_consistent` で `Container<u8>` / `Container<i64>` 並存 dispatch を pin)。**残 (Phase 2c)**: (1) annotation hint (`var v: Vec<u8> = Vec::from_str(...)`) を `call_associated_function` (interpreter) / `lower_let::AssociatedFunctionCall` (compiler) の lookup まで thread し、現状の lone-spec fallback を狭める。複数 concrete-args 入りの associated function (`impl FromStr for Vec<u8>` + `impl FromStr for Vec<i64>` で両方が `from_str(s) -> Self` を持つケース) を annotation で disambiguate 可能に。(2) 型 checker `struct_methods` registry を Vec<MethodSpec> 形に refactor (現状 same `(struct, method)` で異 args の impl が来ると last-wins、signature が同一なら type-check は通るが、異なる signature を持つ複数 concrete impl を厳密化するために必要)、(3) generic methods queue (`pending_method_work`) で対象の `target_type_args` を carry し、Vec から first spec ではなく対応する spec を選ぶように。priority 中。

REF-Stage-2 (residual). **`&T` reference の残perf作業** — Stage 1 (`&mut self`) と Stage 2 (a/d/e/f/b/c/g/i/iv: `&T`/`&mut T` 引数 + writeback + escape reject + ref chain など) は完了済み (詳細は git log)。残: (ii-true-pointer) compound `&mut T` を真の pointer-passing で渡して copy 削減、(iv-compound) `&T` (immutable) compound 型の RefScalar 経路活用で copy 削減。優先度: 低 (perf 改善のみ、機能的には差分なし)。

65. **frontend リファクタリング候補** (2026-05-09 棚卸し):
   - ✅ **(a) `ast/pool.rs::add` / `update` の dedupe** — `populate_expr_slot` / `populate_stmt_slot` / `clear_expr_slot` / `clear_stmt_slot` ヘルパに集約済み。新 variant 追加時の漏れ防止にも有効。
   - **(b) `type_checker/expression.rs::visit_binary` (267 行) の op category 分割** — arith / compare / bitwise / shift / logical の 5 系統が 1 関数に同居。`visit_arith_binary` / `visit_compare_binary` / `visit_bitwise_binary` / `visit_logical_binary` / `visit_shift_binary` に分割し、`visit_binary` 自体は dispatch table に。OP-OVERLOAD で各 arm が肥大化している経緯。優先度 ★★
   - ✅ **(c) `parser/expr.rs` の分解** — `expr.rs` (1885 行) を `expr/mod.rs` (580 行) + `expr/primary.rs` (506 行) + `expr/control.rs` (157 行) + `expr/match_.rs` (195 行) + `expr/macros.rs` (193 行) に分割。`parse_primary_impl` / `parse_primary_atom_or_form` / `parse_primary_keyword_form` / `parse_interpolated_string` / `parse_tuple_or_grouped_expr` / `parse_bracket_access` / `parse_array_elements` / `parse_struct_literal_fields` / `parse_closure_expr` を primary.rs に集約。`parse_if` / `parse_if_val` / `parse_with` / `parse_dict_literal` を control.rs に集約。`parse_match` / パターン関数を match_.rs に集約。parser macro 関数を macros.rs に集約。
   - **(d) `AstVisitor` trait の per-category 分割** — `visitor.rs` (717 行) + `visitor_impl.rs` (743 行) を `ExprVisitor` / `StmtVisitor` / `DeclVisitor` のサブ trait に分離。`Acceptable` も対称に分割。trait API 破壊変更で外部 implementor 影響あり (workspace 内のみ前提)。優先度 ★、大規模
   - **(e) `ast/builder.rs` の 63 関数を macro で簡潔化** — `*_with_label` / `*_stmt` / `*_expr` の boilerplate が多い。`builder_method!` macro で signature + body を 1 行に。優先度 ★
   - **(f) 細かい関数分解** — `visit_call` (153 行), `visit_unary` (152 行 — 主に struct overload 周り), `visit_closure_impl` (92 行), `parse_match_pattern` (156 行) を内部 helper 切り出しで 50 行以下に。優先度 ★
   - **(g) `type_checker/utility.rs` (439 行) と `context.rs` (403 行) の責務再整理** — 雑多な utility 関数を topic 別 module へ移動 (variable-scope / type-resolve / error-helpers 等)。優先度 ★
   - **既存の課題**: docコメント拡充、プロパティベーステスト追加 (進行中)

AOT-MATCH-SCRUTINEE-EXPAND. **AOT match scrutinee に function-call enum を許可** — 現状 `compiler/src/lower/match_lowering.rs::classify_match_scrutinee` は (1) enum-bound identifier、(2) enum-returning method-call (ITER-PROTOCOL-AOT で導入)、(3) scalar primitive のみ受理。`while val Some(x) = func(i)` のような **enum-returning function-call scrutinee** は未対応で、IF-VAL の MVP 制約として `docs/language.md` に明記済み。method-call path の構造をそのまま `Expr::Call` 用にも書けば対応可能 (CallEnum + writeback 不要 — function は Self を持たない)。優先度 中。

NEW-FEATURES. **新規 syntactic-sugar 候補** (2026-05-09 棚卸し、未着手):
   - **`?` 演算子** — `Result::?` / `Option::?` で early-return。`expr?` を `match expr { Ok(v) => v, Err(e) => return Err(e) }` に desugar (closures landed 済で blocker なし)。優先度 ★★★
    - **OP-OVERLOAD-CHAIN** — `a + b + c` chained position 対応。今は let-rhs のみ MVP (`bda35dc`)。chained / binary struct literal operand (`a & Bits { v: 1 }`) を解消。優先度 ★★
    - **`??` (null-coalesce)** — `opt ?? default` で `unwrap_or` の糖衣。優先度 ★
    - **raw / multi-line string literal** — `r"\path"` / `"""..."""`。lexer 拡張のみ。優先度 ★

NEW-TYPE-SYSTEM. **型システム拡張候補** (2026-05-09 棚卸し、未着手):
   - **Trait 拡張** — default method body / 多重 bound (`<T: A + B>`) / trait inheritance / `dyn Trait` / associated types。stdlib HOF (`Option::map`) の前提。優先度 ★★★、大規模
   - **Trait-bounded generic API** — `fn first<I: Iterator<i64>>(iter: I)` の bound check (現状 `<T: Trait>` は struct で動くが Iterator 等 generic trait の bound は未強制)。優先度 ★★
   - **`Display` trait** — user-defined `to_string` で STR-INTERP の default 動作を拡張可能に。優先度 ★★
   - **`From` / `Into` 自動変換** — `let s: String = "hi".into()` の自然変換。優先度 ★★
   - **slice 型 `&[T]`** — 配列 borrow を first-class に。優先度 ★、中〜大
   - **const generics** — `struct Array<T, const N: usize>`。優先度 ★、大規模
26. **ドキュメント整備** — 言語仕様 / API ドキュメント (`docs/language.md` は最新化済み、`compiler/README.md` / `interpreter/README.md` も追従済み。残: API リファレンス、advanced topics)

TEST-PERF. **テスト実行時間改善** (2026-05-16 プロファイル):
- 現状: `cargo test` = ~51.5s wall-clock / `cargo nextest run` = ~41.7s (20% 高速)。`PROPTEST_CASES=32` を `.cargo/config.toml` の `[env]` にデフォルト化済み。
- 遅いテストトップ3: `compiler::consistency` (7.4s / 82 tests) → AOT compile + spawn、`interpreter::jit_integration` (重複 `run_source` 呼び出し)、`interpreter::generics_tests` (3.4s)。
- 実施済み: `compiler/tests/consistency.rs` の `interpreter_value_with_core` / `jit_exit_code` に `LazyLock<Mutex<HashMap>>` キャッシュ、`interpreter/tests/jit_integration.rs` の `run` 関数に同様のキャッシュ（ヒット率は限定的だが同ソース複数呼び出し時に有効）。
- 残タスク:
  - `interpreter/tests/` の25テストバイナリを機能別に統合（バイナリ起動オーバーヘッド削減）。優先度 ★
  - core モジュールのパース結果を `thread_local!` でテストバイナリ内共有。優先度 ★★
  - `serial_test` (`oop_tests.rs`) を並列化可能にする（破壊ログのスレッドローカル化）。優先度 ★
183. **コンパイラ MVP** — Phase A〜D + Phase E〜Z 系列まで全て完了 (詳細は git log `compiler/` 関連コミット)。残: lower 周辺の compound-returning method の expression position 制約、generic struct の JIT (`159`)、tuple JIT のネスト対応 (`160`)、CONCRETE-IMPL Phase 2c (annotation hint threading)、3+ part qualified call (`185残`)、extern fn の JIT/AOT monomorph 化 (`195b`)、NUM-W-AOT-pack Phase 3 (compound element packing) など個別エントリで継続管理。AOT live state の現在の制約は `compiler/README.md` を参照。


## 検討中の機能

* FFI/拡張ライブラリ
* 文字列操作
* ラムダ式・クロージャ
* モジュール拡張（バージョニング、リモートパッケージ）
* 言語組み込みテスト機能
* 言語内からのAST取得・操作
* LSP (Language Server Protocol) 対応 — エディタ統合 (補完、go-to-definition、hover、診断、フォーマット)。frontend の AST/型チェッカ・SourceLocation を再利用して `tower-lsp` などで実装

## 実装済み機能サマリー

### コア言語機能
- 基本言語機能: if/else/elif、for、while、break/continue (`@label:` でラベル付きループ + `break @label` / `continue @label`、3 backend)、return、`if val PAT = EXPR { ... }` / `while val PAT = EXPR { ... }` (parser desugar、Rust の `if let` / `while let` 相当)
- 変数: val（不変）/var（可変）、コンテキストベース型推論
- 数値型: u64 / i64 / f64（f64 リテラルは `1.5f64` / `42f64` のように `f64` サフィックス必須、タプルアクセスとの曖昧性回避）。`as` による i64/u64 ↔ f64 変換、剰余 `%` と複合代入 `+= -= *= /= %=` 対応
- 固定配列: 型推論対応、インデックス型推論、境界チェック、要素に struct / tuple / 別配列も可
- 配列スライス: `arr[start..end]`、`arr[..]`、負インデックス`arr[-1]`対応
- 辞書（Dict）型: `dict{key: value}`リテラル、Object型キーサポート
- 構造体: 宣言、implブロック、フィールドアクセス（read/write 両対応）、メソッド (`self: Self` / `&self` / **`&mut self`** — Stage 1 reference receiver、AOT で Self-out-parameter writeback)、非ジェネリック struct でも `Struct::new()` の associated function、`__getitem__`/`__setitem__`、ネストフィールド (`a.b.c`) chain access
- タプル: 局所バインディング + 関数引数 / 戻り値、`val (a, b) = expr` 分解 (ネスト対応)、`t.0` access、ネストタプル + tuple-of-struct + struct-of-tuple
- Trait: `trait Name { fn m(self: Self) -> T }` 宣言、`impl <Trait> for <Struct> { ... }` 実装、`<T: SomeTrait>` bound、conformance チェック（型不一致・欠落メソッド検出）。**プリミティブ型に対する extension trait** (`impl <Trait> for i64/f64/...`) も interpreter / JIT / AOT 全 backend で動作。stdlib の `i64.abs()` / `f64.abs()` / `f64.sqrt()` も `core/std/{i64,f64}.t` の extension trait impl 経由。chained primitive method call (`x.abs().abs()`) も 3 backend 全対応 (`#194`)
- `extern fn` 宣言: `extern fn name(params) -> ret` で signature だけ宣言、body は backend (interpreter registry / JIT helper or native / AOT libm import) が提供。math intrinsic はすべてこの仕組み経由。generic params (`extern fn name<T>(x: T) -> T`) は parser で受理、interpreter で動作 (`#195`)
- 文字列: ConstString/String二重システム、`str.len()`、`.concat()`、`.trim()`、`.to_upper()`、`.to_lower()`、`.split()`、`.substring()`、`.contains()`。STR-PTR-LEN (`93892b2`): AOT の `.rodata` per-literal layout は `[bytes][NUL][u64 len LE]`、`__builtin_str_to_ptr(s)` / `__builtin_str_len(s)` で byte ポインタとバイト長を取れる。`core/std/str.t` の `AsPtr` / `Length` / `ToString` trait 経由で `s.as_ptr()` / `s.len()` / `s.to_string()` も可
- `String` (= `Vec<u8>` alias、`core/std/string.t`): heap-managed byte buffer。`.len()` / `.as_ptr()` / `.substring(start, end)` / `.trim()` / `.to_upper()` / `.to_lower()` / `.concat(other)` / `.contains(needle)` / `.to_string()` を `core/std/str_ops.t` の `Substring` / `Trim` / `CaseConvert` / `Concat` / `Contains` 経由で extension trait impl。alias-qualified call (`String::from_str("...")` / `String::new()`) も 3 backend 対応。`Vec<u8>::push_char(c: char)` は UTF-8 encoding (RFC 3629、1〜4 bytes、surrogate / U+110000+ は panic)、`type char = u32` (Unicode codepoint)
- `__builtin_sizeof(value)`: primitive / struct / enum (1-byte tag + payload) / tuple / array をサポート (3 backend、AOT は `a57d9fe` で compound 対応)
- コメント: `#`（行）、`/* */`（ブロック）
- Allocator システム: `with allocator = expr { ... }`、`ambient` キーワード、`<A: Allocator>` bound、自動 ambient 挿入、Global / Arena / **FixedBuffer** allocator (interpreter / JIT で 3 種すべて実装済み)。AOT は Phase A (heap / pointer builtins via libc) + Phase B-min (`__builtin_default_allocator` / `__builtin_current_allocator` + `with allocator = ...` scope) を対応 — arena / fixed_buffer の native backend は #121 Phase B-rest で未対応
- Enum + match: unit + tuple variant、`Enum::Variant` / `Enum::Variant(args)`、ジェネリック enum (`Option<T>`)、ネスト enum payload (`Option<Option<i64>>`)、payload に `f64` / struct / tuple / **str** (`#205` AOT も対応)、リテラル / ネスト / タプルパターン、guard (`if cond`)、網羅性チェック、到達性チェック。enum receiver method dispatch も interpreter / AOT で動作 (`#96` `#203`)。JIT は enum 値モデル未対応で silent fallback (`#204` で precise diagnostic)
- DbC: `requires` / `ensures` 節の実行時チェック、`ensures` 内の `result` バインド、`INTERPRETER_CONTRACTS` (interpreter) / `--release` (compiler) で gating
- `panic("msg")` / `assert(cond, msg)` ビルトイン (3 backend 全対応、release でも常時 active 設計)

### 型システム
- 自動型変換・型推論（数値リテラルのサフィックス省略可）
- ジェネリック関数: `fn identity<T>(x: T) -> T`（パース→型推論→実行→3 backend モノモル化）
- ジェネリック構造体: `struct Container<T>`、constraint-based型推論
- ネストジェネリック: `Container<Container<T>>`（C++11スタイル`>>`分割）
- Self キーワード: implブロック内での構造体参照、プリミティブ impl では対応する `TypeDecl` (i64 / f64 / ...) に解決
- Trait bound: `<A: Allocator>` および `<T: UserTrait>` を関数・struct・impl に付与、呼び出し側で検証、bound 連鎖
- method-only generic params: `impl Box { fn pick<U>(self, a: U, b: U) -> U }`
- method receiver: `self: Self` (by-value) / `&self` / `&mut self` の 3 形式 (Stage 1 reference type)。trait conformance で receiver kind 完全一致を要求。AOT は `&mut self` を Self-out-parameter convention で writeback
- references (REF-Stage-2 (a)+(d)+(e)+(f)): `&T` / `&mut T` 引数型、explicit `&value` / `&mut value` borrow 式、`is_arg_compatible` で `T → &T` auto-borrow + `&mut T → &T` downgrade (auto-borrow `T → &mut T` は意図的に不許可)、`&mut <name>` は `var`-declared bare identifier のみ受理、syntactic escape rule (戻り型 / val-var binding / struct field に Ref 不可)。runtime / IR は erasure (true mutation 伝播は (b)/(c)/(g) で deferred)
- pattern match の deep exhaustiveness: `Option<Option<T>>` 等の nested EnumVariant も payload position 単位で coverage 検証 (96残 前半)

### モジュール・その他
- Go-styleモジュールシステム: package/import/qualified name resolution、3-segment 以上は alias 経由
- **コア・モジュール auto-load**: `<repo>/core/` を起動時に再帰的に integrate (`<exe>/core/` / `<exe>/../share/toylang/core/` / `<exe>/../../core/` の順に探索、CLI flag `--core-modules <DIR>` と env var `TOYLANG_CORE_MODULES` で override)。`math::sin(x)` 等が import 行なしで呼べる
- **per-module function namespacing**: IR / 型チェッカー / interpreter 実行時の関数テーブル全 3 層を `(Option<DefaultSymbol> qualifier, DefaultSymbol name)` キー化 (`#193` `#193b`)。同名 `pub fn` を複数モジュールが持っても安全に共存。bare 呼び出しは user-authored を優先、qualified 呼び出しは module qualifier 直接 lookup
- stdlib: `core/std/math.t` (math 関数) / `core/std/i64.t` (Abs trait) / `core/std/f64.t` (Abs/Sqrt impl) / `core/std/option.t` (`enum Option<T>` + is_some/is_none/unwrap_or/expect) / `core/std/result.t` (`enum Result<T, E>` + is_ok/is_err/unwrap_or/expect) / `core/std/hash.t` (`Hash` trait + 全 primitive 型 impl) / `core/std/dict.t` (`Dict<K, V>` user-space hash table、`new`/`insert(&mut self)`/`get`/`get_or`/`contains_key`/`remove(&mut self)`/`size`、3 backend 一致動作 — DICT-AOT-NEW Phase D 完全対応) / `core/std/str.t` (`AsPtr` + `Length` + `ToString` extension traits — `s.as_ptr()` / `s.len()` (O(1)) / `s.to_string()`) / `core/std/str_ops.t` (`Substring` / `Trim` / `CaseConvert` / `Concat` / `Contains` extension traits — `Vec<u8>` で impl) / `core/std/string.t` (`type String = Vec<u8>` alias + 上記 trait の Vec<u8> 向け impl 集約) / `core/std/collections/vec.t` (`Vec<T>` user-space dynamic array、`new`/`push(&mut self)`/`pop(&mut self)`/`get`/`set(&mut self)`/`size`/`capacity`/`is_empty` + `FromStr::from_str(s) -> Vec<u8>` の memcpy ベース str → Vec<u8> 変換 + `push_char(c: char)` UTF-8 encoding) / `core/std/char.t` (`type char = u32`、Unicode codepoint alias)
- 統合インデックスシステム: 配列・辞書・構造体で統一`x[key]`構文

### テスト状況
- 合計 1444 テスト, 31 skipped（100% 成功率、2026-05-09 時点 — STRING-API 全 Phase + AOT-SIZEOF-COMPOUND + AOT-COMPOUND-PTR-RW + TYPE-ALIAS-QUALIFIER + OP-OVERLOAD (Eq + Arith + Phase 1-4 含む全 binary/unary) + STRING-NOMINAL + STR-INTERP-COMPOUND (struct/tuple/nested) + LABEL (labelled break/continue) + IF-VAL (`if val` / `while val`) で計 118 件追加。compiler/e2e は 202、consistency は 65+）
- 内訳: interpreter unit + integration、frontend unit、compiler e2e (191) + consistency (50+) — 後者は interpreter / JIT / AOT 3 経路一致を保証
- パフォーマンス: `compiler/build.rs` で `toylang_rt.c` を pre-build、AOT 1 テストあたりの compile 時間は ~50ms。並列 wall-clock の dominate factor は macOS の Mach-O コード署名検証 (~150-300ms/binary、`compiler/README.md` 参照)

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
- `extern fn` の generic params は parser で受理されるが JIT/AOT は per-instance シンボル名を持たないため interpreter のみで動作 (`#195` / `#195b`)
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省略しているのはこのため)
- 3-part qualified call (`std::math::abs(x)`) は parser が last 名のみ採用 — `import std.math` または auto-load 経由で `math::abs(x)` の形にする必要あり (`#185残`)
