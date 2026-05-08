# TODO - Interpreter Improvements

## 完了済み ✅

> 詳細は git log / commit message を参照。本セクションは直近マイルストーンの 1 行サマリのみ保持する。

### 2026-05-08
- **STR-INTERP-COMPOUND-EXTEND** (tuple + nested compound) (`d36a063`) — `lower_struct_to_string` を refactor し binding tree ベースの `emit_struct_format` / `emit_tuple_format` + dispatch helper `emit_field_to_string` / `emit_tuple_element_to_string` を新設、nested compound (struct field が struct/tuple、tuple element が struct/tuple) を recursive 処理。tuple は `(a, b)` / `(a,)` (Rust 風) format。新規 helper `lower_tuple_to_string` を Binding::Tuple identifier 直接 lookup で resolve (value_scalar が tuple_id を取れない問題を回避)。新規テスト 3 件 (tuple basic / single-element / nested struct)、3-way consistency 2 件。1391 -> **1396 tests pass** (+5)。残: enum 値の補間 (`{Option::Some(v)}`)、cranelift block 機構を直接扱う必要があり別 commit へ → `STR-INTERP-COMPOUND-EXTEND-ENUM` (下記未実装参照)。
- **STR-INTERP-COMPOUND** (`5c1038c`) — `"value: {struct_val}"` 形式の string interpolation を AOT で動かした (interpreter / JIT は既に動作)。新規 IR `InstKind::ConstStrBytes { bytes: Vec<u8> }` を導入し、frontend interner を経由せず `.rodata` に raw bytes を直接配置 (interner mutability 拡張不要)。codegen に `const_str_bytes: HashMap<Vec<u8>, DataId>` 追加 (content-keyed dedup で `", "` / `" }"` 等の separator は 1 entry 共有)。AOT lower の `BuiltinFunction::ToString` arm に struct 専用 path を追加: `lower_struct_to_string` が field を alphabetical sort して header / `name: ` prefix / value to_string / `, ` separator / ` }` footer を `ConstStrBytes` + `ToString(scalar)` + `StrConcat` chain で組み立て。新規 C runtime helper 不要 (既存 `toy_str_concat` / `toy_to_string_<ty>` で完結)。MVP 制約: all-scalar field の struct のみ (nested compound / tuple / enum は future phase)。1387 -> **1391 tests pass** (+4: 3 unit + 1 consistency)。
- **STRING-NOMINAL** (`682331c`) — `type String = Vec<u8>` alias を削除し、`struct String { data, len, cap, elem_size }` の **flat-layout 独立 struct** に置き換え。Vec<u8> と同じ memory shape だが nominal identity は別。byte-specific helper を全部 `impl String` に移植 (new / from_str / push / pop / get / set / size / len / as_ptr / capacity / is_empty / clear / extend_bytes / push_str / push_char / eq / to_string)。trait 拡張 (Substring / Trim / CaseConvert / Concat<String> / Contains<String> / Split<String, Vec<String>>) も String target に re-impl。`ToString` trait は frontend canonicalisation 制約 (Identifier(String) vs Struct(String, []) の trait conformance mismatch) を回避するため inherent method 化、`str.to_string()` は user-side で `String::from_str(s)` に書き換え。test migration: `Vec::from_str(...)` → `String::from_str(...)` (~70 件)、`val s: String = Vec::new()` → `String::new()` 等。1387 tests pass (regression なし)。
- **AOT-COMPOUND-PTR-RW + STRING-API Phase 5 (Split)** (`07a895d`) — AOT lower で `__builtin_ptr_write/read` の compound (struct/tuple) 値を per-leaf 展開 + `lower_param_or_return_type` の type args 再帰化。これにより `Vec<T>` で T が compound でも全 backend で動作。`STRING-API-SPLIT` も同 commit で着地: `Split<Vec<u8>, Vec<Vec<u8>>>` trait + impl 追加 (`core/std/str_ops.t` / `core/std/string.t`)、`s.split(sep) -> Vec<Vec<u8>>` が 3 backend で動作 (Rust `str::split` 風: trailing sep で empty 末尾、empty sep で panic)。新 helper `compute_leaf_layout` (compute_byte_size の leaf-walking 版)。+7 tests (1380 → **1387**)。
- **OP-OVERLOAD-EQ** (`286a613`) — `==` / `!=` operator が同型 struct ペアで `eq(&self, other: &Self) -> bool` method に dispatch (Phase B)。型 checker `struct_eq_compatible`、interpreter `evaluate_binary` early dispatch、AOT `lower_binary::try_lower_struct_eq` (Call to eq FuncId + leaf flatten + `!=` は `bool_v == false` で negate) で 3 backend 対応。`s == t` で String 比較が動く。+6 tests (1374 → **1380**)。Phase A (String の nominal newtype 化) は spike で frontend symbol canonical 化の深い問題に当たり deferred (下記 STRING-NOMINAL 参照)。
- **TYPE-ALIAS-QUALIFIER** (`ec83086`) — `frontend::alias_resolution` の expression loop に `Expr::AssociatedFunctionCall` qualifier rewrite を追加。`val s: String = String::from_str("hello")` / `String::new()` のような alias-qualified call が 3 backend で動作 (qualifier symbol を alias target の base name に substitute、type args は annotation hint から recover)。+3 tests (1371 → **1374**)。
- **AOT-SIZEOF-COMPOUND** (`a57d9fe`) — `__builtin_sizeof(struct/tuple/enum)` を AOT で対応 (`compute_byte_size` 新設、`value_scalar` で struct/enum identifier を認識)。+1 test (1370 → **1371**)。
- **STRING-API Phase 0〜4** (`a0cae0d` / `e9b2ee7` / `7335421` / `6a0fa3c`) — String API の本格拡充: (Phase 0) `type char = u32` 化 + `Vec<u8>::push_char` の UTF-8 encoding + module_integration の narrow int remap fix。(Phase 1) `Length` / `AsPtr` extension trait を `Vec<u8>` にも impl。(Phase 2) `Substring` / `Trim` / `CaseConvert` 追加 + 末尾式 chain workaround。(Phase 4) `Concat` / `Contains` / `ToString` 追加 + AOT lower の identifier-flatten path / primitive-receiver compound-return bind path 拡張。trait は `core/std/str_ops.t`、impl は `core/std/string.t`。詳細 git log。+44 tests (1326 → **1370**)。

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
96残-後半. **Enum/match — Option/Result の追加 stdlib メソッド** — `Option::map<U>` / `and_then` / `Result::map_err` 等を追加するには **closures** が必要 (要設計議論)。toylang は closure 型をまだ持たないので、その前段の検討が必要。前半 (深い網羅性解析) は `273580c` で完了。

CONCRETE-IMPL-Phase-2c. **annotation hint threading + 型 checker registry refactor** — Phase 1 / 2-interp / 2b は完了 (パーサ + AST + interpreter dispatch + AOT compiler dispatch、3-way `assert_consistent` で `Container<u8>` / `Container<i64>` 並存 dispatch を pin)。**残 (Phase 2c)**: (1) annotation hint (`var v: Vec<u8> = Vec::from_str(...)`) を `call_associated_function` (interpreter) / `lower_let::AssociatedFunctionCall` (compiler) の lookup まで thread し、現状の lone-spec fallback を狭める。複数 concrete-args 入りの associated function (`impl FromStr for Vec<u8>` + `impl FromStr for Vec<i64>` で両方が `from_str(s) -> Self` を持つケース) を annotation で disambiguate 可能に。(2) 型 checker `struct_methods` registry を Vec<MethodSpec> 形に refactor (現状 same `(struct, method)` で異 args の impl が来ると last-wins、signature が同一なら type-check は通るが、異なる signature を持つ複数 concrete impl を厳密化するために必要)、(3) generic methods queue (`pending_method_work`) で対象の `target_type_args` を carry し、Vec から first spec ではなく対応する spec を選ぶように。priority 中。

REF-Stage-2 (residual). **`&T` reference の残perf作業** — Stage 1 (`&mut self`) と Stage 2 (a/d/e/f/b/c/g/i/iv: `&T`/`&mut T` 引数 + writeback + escape reject + ref chain など) は完了済み (詳細は git log)。残: (ii-true-pointer) compound `&mut T` を真の pointer-passing で渡して copy 削減、(iv-compound) `&T` (immutable) compound 型の RefScalar 経路活用で copy 削減。優先度: 低 (perf 改善のみ、機能的には差分なし)。

65. **frontend の改善課題** — docコメント拡充、プロパティベーステスト追加、コード重複削減
26. **ドキュメント整備** — 言語仕様 / API ドキュメント (`docs/language.md` は最新化済み、`compiler/README.md` / `interpreter/README.md` も追従済み。残: API リファレンス、advanced topics)
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
- 基本言語機能: if/else/elif、for、while、break/continue、return
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
- 合計 1396 テスト, 31 skipped（100% 成功率、2026-05-08 時点 — STRING-API-CHAR-U32 / -PHASE-1 / -2 / -4 / -5 + AOT-SIZEOF-COMPOUND + AOT-COMPOUND-PTR-RW + TYPE-ALIAS-QUALIFIER + OP-OVERLOAD-EQ + STRING-NOMINAL + STR-INTERP-COMPOUND + STR-INTERP-COMPOUND-EXTEND で計 70 件追加。compiler/e2e は 197、consistency は 50+）
- 内訳: interpreter unit + integration、frontend unit、compiler e2e (191) + consistency (50+) — 後者は interpreter / JIT / AOT 3 経路一致を保証
- パフォーマンス: `compiler/build.rs` で `toylang_rt.c` を pre-build、AOT 1 テストあたりの compile 時間は ~50ms。並列 wall-clock の dominate factor は macOS の Mach-O コード署名検証 (~150-300ms/binary、`compiler/README.md` 参照)

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
- `extern fn` の generic params は parser で受理されるが JIT/AOT は per-instance シンボル名を持たないため interpreter のみで動作 (`#195` / `#195b`)
- `package` 宣言 / `import` path のセグメントに primitive type キーワード (`i64` / `f64` / ...) は使えない (`core/std/i64.t` が `package` 宣言を省略しているのはこのため)
- 3-part qualified call (`std::math::abs(x)`) は parser が last 名のみ採用 — `import std.math` または auto-load 経由で `math::abs(x)` の形にする必要あり (`#185残`)
