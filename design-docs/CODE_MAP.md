# CODE_MAP.md — 関心事から実装サイトを引く表

「name resolution はどこか」に**読み取り 1 回**で答えるための表
(COMPILER_DEV_LOOP D5)。

> **なぜこれが要るか**
> `f` バグ (bare-name 呼び出しのレキシカルスコープ) の修正には
> **3 バックエンド 4 箇所**の変更が必要だった。型チェッカだけ直した時点で
> 「型は通るが答えが間違う」状態になったが、**何も「他のバックエンドも見ろ」と
> 教えなかった**。調査は grep の往復で進んだ。この表があれば 1 回の読み取りで
> 済んだ。
>
> 履歴 (どのフェーズがいつ landing したか) は
> [`todo.md`](todo.md)、言語仕様は [`docs/language.md`](../docs/language.md)。
> **本ファイルは「場所」だけを答える。**

## 読み方

toylang には**同じ意味論を独立に実装した実行系が 4 つ**ある。

| 実行系 | 実体 | 備考 |
|---|---|---|
| **tree-walker** | `interpreter/src/evaluation/` | 参照実装。診断が最も詳しい |
| **IR VM** | `compiler_vm/` (interpreter の `ir_vm` は re-export + `host.rs` / `lift.rs` 境界層) | 既定エンジン。`compiler_lower` の IR を解釈 |
| **AOT** | `compiler/src/codegen/` | 同じ IR を cranelift に落とす |
| **interpreter JIT** | `interpreter/src/jit/` | **`compiler_lower` を通らない独自経路** |

`compiler_lower` に置いた変更は **IR VM / AOT / compiler 側 JIT の 3 つ**に
効く。tree-walker と interpreter 側 JIT は別途対応が要る。
意味論を変えたら `compiler/tests/consistency/` の `assert_consistent` を使った
テストを追加すること。

---

## 名前解決・呼び出し

| 関心事 | 型検査 | tree-walker | lowering (IR VM / AOT / compiler JIT) | interpreter JIT |
|---|---|---|---|---|
| bare-name 関数呼び出しの解決 | `type_checker/expression.rs::visit_call` | `evaluation/call.rs::evaluate_function_call` | `compiler_lower/src/call.rs::resolve_call_target` | `jit/codegen/mod.rs` |
| メソッド dispatch | `type_checker/method_call.rs` | `evaluation/call.rs::call_struct_method` | `compiler_lower/src/method_call.rs` | 同上 |
| モジュール統合 / 修飾名 | `type_checker/module_access.rs` | `interpreter/src/module_integration.rs` | `compiler_lower/src/program.rs` | — |
| クロージャ | `type_checker/expression.rs::visit_closure_impl` | `evaluation/call.rs::evaluate_indirect_call` | `compiler_lower/src/lib.rs::lift_closure_binding` | fallback |

> **注意**: この行が `f` バグの現場。ローカル束縛とグローバル関数の
> **優先順位**が 3 箇所で独立に決まっている。

## 式・演算

| 関心事 | 型検査 | tree-walker | lowering | IR VM 実行 |
|---|---|---|---|---|
| 二項演算 | `type_checker/expression.rs::visit_binary` | `evaluation/operators.rs::evaluate_binary` | `compiler_lower/src/expr_ops.rs::lower_binary` | `compiler_vm/src/dispatch.rs::eval_binop` |
| 実行時トラップ (RUNTIME-TRAP: u64 underflow / 0 除算 / `MIN / -1`) | — | `evaluation/operators.rs::evaluate_arithmetic_op_v` | `expr_ops.rs` の `emit_u64_underflow_guard` / `emit_div_by_zero_guard` / `emit_div_overflow_guard` (どれも `emit_trap_unless` 経由) | (guard は IR に含まれる) |
| 添字境界の実行時トラップ | — | `evaluation/slice.rs::resolve_array_index` | `array_access.rs::emit_index_guard` (定数 index は `resolve_const_index` がコンパイル時に弾く) | (guard は IR に含まれる) |
| `const fn` / CTFE (COMPILE-TIME-EVAL) | 適格性検査: `type_checker/const_fn_check.rs` (`type_checker/effects.rs` のマスク) | — | fold の評価器は **`compiler_vm` (IR VM)** — `interpreter/src/const_eval.rs` が driver 層で fold (`fold_const_evaluations`) + 配列長の解決 (`resolve_array_lengths`)、`compiler_lower/src/consts.rs::eval_const_expr` が literal リーダー (fold.rs の共通テーブル)。配列長の型は `type_decl.rs::ArraySize` (Literal / Deferred) | — |
| 契約による guard の除去 (CONTRACT-ELISION) | — | — | `contract_facts.rs::ContractFacts::from_requires` (事実の抽出) + `expr_ops.rs` の `contract_rules_out_zero` / `contract_rules_out_underflow` (guard 発行の抑止)、失効は `let_lowering.rs` の `facts.shadowed` | — |
| 代入 | `type_checker/expression.rs::visit_assign` | `evaluation/operators.rs` | `compiler_lower/src/assign.rs::lower_assign` | — |
| 演算子オーバーロード | `type_checker/expression.rs::visit_arith_binary` | `evaluation/operators.rs` | `compiler_lower/src/expr_ops.rs` | — |
| キャスト (`as`) | `type_checker/collections.rs` | `evaluation/expression.rs` | `compiler_lower/src/expr.rs` | `compiler_vm/src/dispatch.rs` |
| SIMD の lane-wise 演算・intrinsic (SIMD) | `type_checker/simd.rs` (型 は `type_decl.rs::VectorType`、intrinsic は `ast/expr.rs::SimdOp`) | `evaluation/simd.rs` (値は `object.rs::SimdValue`) | `compiler_lower/src/simd.rs` (IR は `compiler_ir::VecTy` + `InstKind::Simd*`)、cranelift は `compiler/src/codegen/simd.rs` | `compiler_vm/src/simd.rs` |
| `__builtin_sizeof` (値形式 + 型引数形式 `::<T>`、POINTER P1) | turbofish の parse は `parser/expr/primary.rs` (`BuiltinFunction::SizeOfType(TypeDecl)`、generic context 無しで parse するのでパラメータは `Identifier(T)`)、妥当性検査は `type_checker/expression.rs::validate_sizeof_type`。**generic fn body が自パラメータを generic scope に積む**のは `type_checker/visitor.rs::type_check_body` | `evaluation/builtin.rs::builtin_reflection` (型引数形式は `evaluation/call.rs` の呼び出し境界が push する generic scope で解決 — receiver の runtime type_args / 引数値 / `val` 注釈 / 呼び出し元 scope、ヘルパは `evaluation/mod.rs`) | lower 時に畳む — `compiler_lower/src/expr.rs::lower_builtin_reflection` の `SizeOfType` arm (active monomorph subst + `compute_byte_size`)。subst は `PendingGenericInstance` (free fn) / `PendingMethodInstance` (method) が運ぶ。**module 統合の remap が payload を 通る**のは `interpreter/src/module_integration.rs::map_expr` の `SizeOfType` arm | silent fallback (`jit/eligibility/checker.rs`) |

## 束縛・スコープ

| 関心事 | 場所 |
|---|---|
| 所有権の移動 / use-after-move (E0014) | `type_checker/move_check.rs::check_moves` (結果は `File::transferred_bindings`)。**所有の判定** (推移的 contains_drop) は `type_checker/contains_drop.rs::DropAnalysis`。**別名** (`val b = a` / `match` の腕の payload 名 / `val x = match a {..}`) は `Owned::root` で根を持ち、渡すと根が移る (`use_binding` / `declare_owning` / `match_alias_source`)。腕の中で scrutinee の payload を渡してよいかは `arm_consumes`。**値渡し引数の貸し出し** (読むだけの受け手なら呼び出し側が drop を持つ) は `compute_lend` / `LendAnalysis` が不動点で決め、呼び出し側は `walk_arg_list` で `Use::Lend` になる |
| 再帰 drop (DROP-GLUE) | glue 関数の合成: `compiler_lower/src/drop_glue.rs` (`ensure_drop_glue` / `lower_drop_glue`)。**drop を抑制 / 登録する側** — tree-walker: `interpreter/src/evaluation/mod.rs::register_drop_if_needed` + `glue_drop` (値駆動の iterative walk)、AOT/IR VM: `compiler_lower/src/lib.rs::register_drop_for_struct_binding` / `register_drop_for_enum_binding` / `register_drop_for_tuple_binding` + match arm 束縛 (`match_lowering.rs` の `arm_drop_targets`)。冪等 free + never-reuse bump ヒープ: `compiler/runtime/toylang_rt/` (AOT と compiler JIT が共有、RUNTIME_PORT R1) |
| 再帰型の検出 (E0013) | `type_checker/recursive_type.rs::check_recursive_types` (呼び出しは `interpreter/src/lib.rs::check_typing_diagnostics`)、lowering 側の保険は `compiler_lower/src/templates.rs` の `Guard` |
| `val` の型注釈チェック | `type_checker/statement.rs::visit_val_impl` |
| `var` の型注釈チェック | `type_checker/visitor.rs::process_val_type_with_mut` |
| ブロックスコープ (lowering) | `compiler_lower/src/expr.rs::lower_expr_block` |
| スコープスタック (型検査) | `type_checker/scope.rs`, `type_checker/context.rs` |
| struct / enum 宣言の**事前登録** | `type_checker/visitor.rs` のコンストラクタ (statement pool を 1 周して `register_struct` / `enum_definitions` に入れる)。これがあるのでフィールド型は後方の宣言を名指しできる。enum は `enums_awaiting_decl` で「事前登録した分」と「2 つ目の宣言」を区別する。フィールド型の妥当性検査は `type_checker/struct_literal.rs::visit_struct_decl_impl` (`named_type_is_defined` が **両方の表**を見る) |
| タプル struct の desugar (NEWTYPE) | 宣言: `parser/stmt.rs::parse_tuple_struct_fields` (フィールドを `"0"` / `"1"` ... と名付ける、判別は `ast/program.rs::StructField::is_positional`)。**使う側の 2 形は型検査器が書き換える** — `Meters(v)` は `type_checker/expression.rs::check_tuple_struct_construction` (`visit_call` の関数未発見経路から)、`m.0` は `type_checker/collections.rs::visit_tuple_access_impl` + `tuple_struct_field_symbol`。ただし**この 2 つは節点自身の `ExprRef` を持たない**ので (`accept_expr` 経由の呼び出し元が多数)、決定は `TupleStructRewrites` に記録し、`expression.rs::apply_tuple_struct_rewrites` が pool を 1 周して `StructLiteral` / `FieldAccess` に置換する (呼び出しは `interpreter/src/lib.rs::check_typing_diagnostics` と `type_checker/module_access.rs::check_program_multiple_errors`)。パターン `Meters(v)` は parser が直接 `Pattern::Struct` を作る (`parser/expr/match_.rs::parse_pattern_tuple_struct`)。**バックエンドは砂糖を見ない**。表示だけは書いた形に戻す — `interpreter/src/object.rs::to_display_string` / `compiler_lower/src/print.rs::emit_print_struct` / `frontend/src/api.rs` |
| 実行時環境 | `interpreter/src/environment.rs` |

> `val` と `var` は**別経路**。片方だけ直すと非対称になる (実際に起きた)。

## match・パターン・enum

| 関心事 | 場所 |
|---|---|
| パターン型検査 / 網羅性 / 到達性 | `type_checker/pattern_match.rs` |
| or パターン (PATTERN-EXTEND) | `parser/expr/match_.rs::parse_match_pattern` (全 sub-pattern 位置がここを通る) + `expand_slots` の直積。alternative ごとに arm を複製する (body / guard は共有) ので型検査・バックエンドに専用の分岐は無い。alternative 間で束縛名が食い違うと parse エラー (`check_alternatives_bind_alike`)、展開数の上限は `MAX_PATTERN_ALTERNATIVES` |
| `@` / 範囲 パターン (`Pattern::Binding` / `Pattern::Range`) | どちらも実 pattern 形。parse は `parse_match_pattern` (任意の深さ)、`@` を剥がすのは `type_checker/pattern_match.rs::peel_bindings` (網羅性・到達性は内側の pattern のもの)、範囲の被覆判定は同ファイルの `IntCoverage` (literal と range を 1 つの区間集合で持ち、隣接区間を merge するので型を分割する arm 群は `_` 不要)。interpreter は `evaluation/expression.rs::try_match_pattern`、AOT/JIT は `match_lowering.rs::dispatch_arm_pattern` / `emit_range_branch` / `bind_payload_sub_pattern` / `dispatch_field_shape_pattern`。interpreter 側 JIT は eligibility で reject |
| const パターン (MATCH-CONST-PATTERN) | const の登録: `interpreter/src/lib.rs` の const 登録ループが `register_const_for_patterns` を呼ぶ。書き換え: `type_checker/pattern_match.rs::rewrite_patterns` (`visit_match_impl` の冒頭で腕を literal pattern に置き換え、以降の検査はその形を見る) → `apply_pattern_rewrites` が scrutinee を鍵に pool へ書き戻す。値は宣言型で型を固定した literal の**複製** (`typed_const_literal`)。バックエンドは名前を見ない |
| enum の struct variant (ENUM-STRUCT-VARIANT) | 宣言: `parser/program_parser.rs` の enum 本体 (`EnumVariantDef::field_names`)。使う側の 2 形はパーサが `E::A` を**結合した 1 つの名前**で既存の形に載せる (`Parser::enum_variant_path_symbol`): 構築は `parser/expr/primary.rs` の qualified path 分岐 (`brace_opens_field_list` で `match E::A {` と区別) → `Expr::StructLiteral`、pattern は `parser/expr/match_.rs::parse_pattern_enum_variant_tail` → `Pattern::Struct`。型検査器が位置の形に戻す: `type_checker/enum_struct_variant.rs` (構築は `visit_enum_struct_literal` → `apply_enum_struct_literal_rewrites`、pattern は `pattern_match.rs::rewrite_pattern` から `struct_variant_pattern`)。バックエンドは tuple variant しか見ない。`--api` の描画は `frontend/src/api.rs` |
| enum の discriminant と `as` (ENUM-DISCRIMINANT) | 値の parse: `parser/program_parser.rs::parse_discriminant_literal` (`EnumVariantDef::discriminant`)。実効値・重複検査・`as` の検査と書き換え: `type_checker/enum_cast.rs` (`discriminant_values` / `check_enum_cast` / `apply_enum_cast_rewrites`、入口は `collections.rs::visit_cast_impl`)。`as` は `match` に (素のパスは literal に) 書き換わるのでバックエンドは知らない |
| `String` のリテラル腕 (MATCH-STRING-LITERAL) | `type_checker/pattern_match.rs::rewrite_string_literal_arms` — `visit_match_impl` で const パターンの書き換えの直後に、`"a" =>` を `_ if <scrutinee>.eq_str("a")` に置き換える (scrutinee は `is_readable_twice` な名前 / フィールドパスのみ)。pool への書き戻しは `apply_pattern_rewrites`。compiled レーンでフィールドを scrutinee にできるのは `compiler_lower/src/match_lowering.rs::classify_match_scrutinee` の FieldAccess 分岐 |
| struct の省略形と分割束縛 (STRUCT-SUGAR-GAP) | 省略形: `parser/expr/primary.rs::parse_struct_literal_fields_impl` (enum の struct variant 側は `Parser::brace_opens_field_list`)。分割束縛: `parser/stmt.rs` の `parse_var_def` → `parse_tuple_destructuring` (`DestructPat::Struct` / `parse_destruct_struct`)。検査用の 1 腕 `match` は `destruct_check_pattern` |
| ループの値 (BREAK-WITH-VALUE) | パーサ: `parser/stmt.rs::parse_loop_value` (文の `loop`) / `parse_loop_expr` (式の位置、`parser/expr/primary.rs::parse_primary_keyword_form` から。`@label: loop` もそこ)。`break` の行き先は `Parser::loop_stack` (`LoopFrame`、各ループの本体は `parse_loop_body` を通る)、値つき `break` の展開は `parse_break_value`。型検査器: `type_checker/expression.rs` — 隠し var の登録は `visit_block_stmt`、型の確定は `visit_assign` → `settle_loop_value`、決まらなかったときのエラーは `unsettled_loop_value` |
| match 実行 | `evaluation/expression.rs` |
| compound (struct / tuple / enum) が call 引数を渡るとき | **1 値では渡らない — leaf ごとの local に materialize してその値を並べる**。binding 引数は `expr.rs::lower_call_args_with_target` / `lower_arg_values` / `method_call.rs::build_method_call_values` の identifier 展開、literal 引数は `expr.rs::lower_compound_literal_arg` (CALL-ARG-COMPOUND-LITERAL)、**enum-typed field を引数位置で直接読む** (`has_next(node.next)`) は `lower_call_arg_items` の `FieldChainResult::Enum` arm (POINTER P5)。generic の monomorph は callee の**引数スロットの型** (`Function::params`、宣言 1 個 = entry 1 個) から採り、literal と食い違うスロットは信用せず名前ベースに落ちる |
| match lowering + scrutinee 制約 | `compiler_lower/src/match_lowering.rs`。compound scrutinee (struct / tuple) は `classify_match_scrutinee` が `MatchScrutinee::Struct` / `Tuple` を返し、照合は `dispatch_struct_pattern` / `dispatch_tuple_pattern` / `dispatch_field_shape_pattern`。**arm body の型推論にも束縛が要る** (`bind_*_for_inference`) — 無いと result local が作られない |

## trait / dyn

| 関心事 | 場所 |
|---|---|
| trait 宣言・conformance | `type_checker/trait_decl.rs` |
| default method 展開 | `type_checker::expand_trait_defaults_in_pool` |
| impl block | `type_checker/impl_block.rs`, `type_checker/method.rs` |
| 型パラメータ bound の強制 (`<T: Ord>`) | `type_checker/utility.rs::check_generic_bounds` (判定は `satisfies_trait_bound`)。呼び出しは free function が `generics.rs::visit_generic_call`、method が `method_call.rs` の generic struct / enum 経路 |
| `dyn Trait` (AOT vtable / thunk) | `compiler_lower/src/program.rs`, `compiler/src/codegen/` |

## 診断 (LLM_FEEDBACK_LOOP)

| 関心事 | 場所 |
|---|---|
| 型エラーの型・位置 | `type_checker/error.rs` (`TypeCheckError` / `SourceLocation`) |
| 位置の付与ヘルパ | `type_checker/error_helpers.rs` (`check_expr_located` / `error_with_location`) |
| 文単位のエラー回復 (P1) | `type_checker/visitor.rs::type_check`, `expression.rs::visit_block` |
| 構造化診断・エラーコード・修正提案 (P3) | `frontend/src/diagnostic.rs` |
| テキスト描画 (caret / snippet) | `interpreter/src/error_formatter.rs` |
| パースエラー収集・1 宣言 1 件 | `parser/core.rs::report_error`, `parser/program_parser.rs::parse_program` |
| 型検査ドライバ (全件報告) | `interpreter/src/lib.rs::check_typing_diagnostics` |
| どのファイルの位置か (DEBUG-OBS D2) | `frontend/src/source_map.rs` (`FileId` / `SourceMap`)。map は `File.source_map`、モジュールの位置の付け替えは `module_integration.rs::integrate` |
| panic の位置・backtrace (P6-1, D1) | `interpreter/src/error.rs` (`CallFrame`)、frame を積むのは `evaluation/call.rs` の `call_method` / `call_associated_method` / 閉包 2 箇所と `evaluate_function_call`、描画は `lib.rs::render_backtrace` (折り畳み + 深さ上限) |
| バックエンド間の診断比較 (DEBUG-OBS D0) | `compiler/tests/consistency/diagnostics.rs` + `harness.rs::diagnostic_lanes` |
| 再帰上限 / stdlib の境界 (DEBUG-OBS D6) | 上限は `compiler_ir::RECURSION_LIMIT` (文言は `recursion_limit_message`)。検査は VM `call_function` / codegen のプロローグ / interpreter JIT の `emit_recursion_check` / tree-walker の `max_call_depth`。`Vec` / `String` の境界は `core/std/` の toylang 側 |
| ユーザ向け診断 API (DEBUG-OBS D5) | `__builtin_function_name` はパーサ置換 (`parser/expr/macros.rs`)、`__builtin_backtrace` は `InstKind::Backtrace` → tree-walker `evaluation/builtin.rs` / VM `dispatch.rs` / AOT `toylang_rt::toy_backtrace_str` |
| 実行時失敗の JSON (DEBUG-OBS D5) | `interpreter/src/lib.rs::runtime_diagnostic` + `Diagnostic::backtrace`、コードは `E0019` / `E0020` (`frontend/src/explain.rs`) |
| backtrace のフレーム (DEBUG-OBS D4) | `compiler_ir` の `Frame` / `FrameId` / `Instruction::frame` / `render_backtrace`、stamp は `compiler_lower/src/lib.rs::emit`、shadow stack は `toylang_rt` の `toy_shadow_ctx` (スレッドごと、`ToyShadowCtx`) / `write_backtrace`、codegen は `codegen/mod.rs::ShadowPrologue` + `lower_inst.rs::emit_frame_push` |
| 実行時診断の位置 (DEBUG-OBS D3) | `compiler_ir` の `Site` / `SiteId` / `Module::intern_site` / `render_stderr_text`、書式は `format_diagnostic_frame` (interpreter の `ErrorFormatter` も呼ぶ)。site を付けるのは `compiler_lower` の `current_site` / `site_of`、AOT の blob は `codegen/mod.rs::declare_panic_string`、書き出しは `toylang_rt::toy_panic_at` |
| 契約違反時の値 (P6-2) | `evaluation/call.rs::capture_contract_bindings` |
| 型ホール `val x: _` (P7) | `parser/stmt.rs::parse_var_def` (受理), `parser/types.rs` (他位置で拒否), `type_checker/error_helpers.rs::report_type_hole` |
| 型のソース表記 | `frontend/src/type_decl.rs::TypeDecl::source_name` (**散文用の `type_name_for_error` とは別物** — 貼り戻せる表記を返す) |
| エラーコード解説 (P7 `--explain`) | `frontend/src/explain.rs` (`codes::ALL` と対応、テストで強制) |
| シグネチャ一覧 (P7 `--api`) | `frontend/src/api.rs::render` |
| 契約節の span | `parser/declarations.rs::parse_clause_with_span` (節全体を根の式に記録) |
| 式の span を消費範囲まで広げる | `parser/core.rs::Parser::span_to_cursor` |
| postfix チェーンの span 付け直し | `parser/expr/mod.rs::parse_postfix_impl` (field / tuple access / method call / index / cast / `?`) |
| キーワード構文の anchor (`if` / `match` / `with`) | `parser/expr/primary.rs::keyword_form` |
| `as` キャスト提案の可否 | `type_checker/error_helpers.rs::cast_suggestion_form` (**span が式全体を覆う形にだけ提案する**) |
| JIT が実際にビルドに入っているか | `interpreter::jit_available()` (cross-backend suite が assert) |

## 契約 (Design by Contract)

| 関心事 | 場所 |
|---|---|
| `requires` / `ensures` の型検査 | `type_checker/visitor.rs::check_contract_clause`, `type_checker/impl_block.rs` |
| 実行時評価 | `evaluation/call.rs::evaluate_requires_clauses` / `evaluate_ensures_clauses` |
| 実行モード (`INTERPRETER_CONTRACTS`) | `evaluation/mod.rs::ContractMode` |
| lowering (panic 化) | `compiler_lower/src/program.rs` + `ContractMessages` |
| `old(expr)` の desugar (ALLOC-CONTRACT) | `parser/expr/primary.rs::parse_old_snapshot` — 式を `Function.old_exprs` に移し、跡地に `__old_N` を残す。**バックエンドは普通の識別子しか見ない** |
| `old` の入口評価 | 型検査は `type_checker/visitor.rs::check_old_snapshots`、tree-walker は `evaluation/call.rs::evaluate_old_snapshots`、lowering は `compiler_lower/src/program.rs::emit_old_snapshots` (どれも requires の後・body の前) |
| アロケーション契約の糖衣 (ALLOC-CONTRACT-SUGAR) | `parser/expr/primary.rs::parse_alloc_budget` (`allocates` / `retains` / `allocations` → `counter() <= old(counter()) + N`)。語の対応は同ファイルの `alloc_budget_stat` |
| 契約節の種別 | `ast/program.rs::EnsuresKind` (`ensures` と並列の `ensures_kinds`)。budget 判定は `parser/declarations.rs` の `last_alloc_budget` 比較 |
| budget 違反の診断 | 文言は `compiler_ir::format_alloc_budget_violation` (**no_std 複製が `toylang_rt::toy_panic_alloc_budget`**)、tree-walker は `evaluation/call.rs::alloc_budget_detail`、lowering は `program.rs::emit_alloc_budget_check` → `Terminator::PanicAllocBudget` |
| 静的な確保検査 (NEVER-ALLOCATES) | `type_checker/alloc_check.rs::check_never_allocates` (`type_checker/effects.rs` の `Alloc` マスク)。修飾子の parse は `parser/program_parser.rs` の `pub` 直後、診断は `E0016` |
| 生メモリ検査 (POINTER P6、`unsafe fn`) | `type_checker/unsafe_check.rs::check_unsafe_declarations` (`type_checker/effects.rs` の `RawRead | RawWrite` マスク)。**呼び先を辿らない** `EffectTable::new_direct_only` / `of_body_direct` を使うので `unsafe fn` を呼んでも呼び出し側は safe。修飾子の parse は free fn が `parser/program_parser.rs`、method / trait signature が `parser/stmt.rs::parse_method_modifiers` (contextual、`never_allocates` / `const` と順不同)、AST は `Function::is_unsafe` / `MethodFunction::is_unsafe` / `TraitMethodSignature::is_unsafe` (default body の継承は `type_checker/trait_decl.rs::synthesize_default_method`)。診断は `E0024` |
| RUNTIME-TRAP guard の消去 (GUARD-ELISION) | `compiler_lower/src/contract_facts.rs` — `requires` と **`if` の条件 / `for` の範囲**の両方から事実を取る。消費は `expr_ops.rs` (0 除算 / underflow / `MIN / -1`) と `array_access.rs` (境界)。設計は [`GUARD_ELISION.md`](GUARD_ELISION.md) |
| 並列ループの本文検査 (CONCURRENCY A1、E0029) | `type_checker/parallel_check.rs::check_parallel_loops` — `parallel for` の本文が出力 (`Io`) や `with allocator` に届く形を拒否。どのループが `parallel` かは `File::parallel_loops` (parser が記録)。設計は [`CONCURRENCY.md`](CONCURRENCY.md) |
| モジュールパスの検査 (MODULE-SYSTEM P3、E0030) | `type_checker/module_path_check.rs::check_module_paths` — 書いた `a::b::f` のパスが実在するか。全セグメントは `File::call_paths` (parser が記録)、**解決自体は最寄り 1 セグメントのまま** (P2 の規則)。設計は [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) |
| 要素の借用 (ELEMENT-BORROW、E0027 / E0028) | `type_checker/move_check.rs` の `check_copy_out_of_borrow` / `check_owning_element_copy`、参照の返却は `type_checker/method.rs::check_reborrow_returns`、窓としての扱いは `region_check.rs::names_window`。設計は [`ELEMENT_BORROW.md`](ELEMENT_BORROW.md) |
| リージョン脱出検査 (REGION) | `type_checker/region_check.rs::check_regions` — `with allocator = <local>` から確保した値が allocator より長生きする形を `E0022` で拒否。「確保か」は effects の `Alloc`、「ポインタを持ちうる型か」は `expr_types`。設計は [`REGIONS.md`](REGIONS.md) |
| エフェクト推論 (EFFECT-SYSTEM) | `type_checker/effects.rs` — builtin → エフェクトの表 (`builtin_effect`) と呼び出しグラフの歩行 (`EffectTable`)。4 検査 (`alloc_check` / `const_fn_check` / `contract_purity` / `unsafe_check`) はここへのマスク (最後の 1 つだけ `direct_only` で呼び先を辿らない)。一覧は `--effects` (`interpreter/src/main.rs::run_effects` → `lib.rs::effects_from_source`)。設計は [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) |
| trait 契約の impl への継承 (DBC-TRAIT-INHERIT) | `type_checker/trait_decl.rs::inherit_trait_contracts` (引数名の一致は `check_trait_conformance_with_args` が強制) |
| 事前条件の強化拒否 (DBC-LISKOV) | `type_checker/trait_decl.rs::check_trait_conformance_with_args` の `strengthened` — impl が trait に無い `requires` を持てば `E0023`。impl 登録の**後**に返すので `&dyn` の型エラーが被さらない |

## テスト・検証機構

| 関心事 | 場所 |
|---|---|
| `test` ブロック (P4) | `parser/program_parser.rs` (contextual), `interpreter/src/lib.rs::run_tests` |
| 契約プロパティテスト (P5) | `interpreter/src/property.rs` |
| バックエンド一致 (単発) | `compiler/tests/consistency/` の `assert_consistent` |
| バックエンド一致 (全 example 掃引) | `compiler/tests/example_consistency.rs` |
| バックエンド一致 (CLI・D6) | `compiler/src/all_backends.rs` (`compiler f.t --all-backends`) |
| コンパイル時間のプロファイル (COMPILE-PROFILE) | 記録器: `frontend/src/compile_profile.rs` (`phase` / `count` / `file_parsed` / `hot`)。表示: `compiler/src/compile_profile.rs`。計測点: `compiler/src/lib.rs::compile_file` (read / parse / link)、`interpreter/src/lib.rs` の `integrate_modules` と `check_typing_collecting` (modules / typecheck の各段)、`interpreter/src/module_integration.rs` (ファイルごと)、`compiler_lower/src/program.rs::lower_program` (+ `record_lowered`)、`compiler/src/codegen/mod.rs` の `emit_object` / `build_object_module` |
| コード生成の再現性 | `compiler/tests/reproducible_build.rs` (**lowering / codegen で HashMap を反復すると link cache が全ミスになる**) |
| JIT の eligibility / fallback | `interpreter/src/jit/eligibility/` |

## メモリ・allocator

| 関心事 | 場所 |
|---|---|
| heap builtin (`__builtin_heap_*` / `ptr_*`) | `evaluation/builtin.rs` |
| `__builtin_ptr_read` の**型注釈の解釈** | **2 箇所ある**: 型検査は `type_checker/visitor_impl.rs::visit_builtin_call` の hint 許容リスト、lowering は `compiler_lower/src/let_lowering.rs::lower_let_builtin_ptr_read` (名前解決は `lower_type_arg`)。片方だけ足すと「型は通るが lower できない」/「lower はできるが型で落ちる」になる |
| `Ptr<T>` / `Span<T>` (POINTER P3/P4) | **stdlib のみ** (生 builtin を触る method は `unsafe fn`、P6) — `core/std/ptr.t` / `core/std/span.t` (`Box<T>` と同じ手口、backend 特殊扱いゼロ)。bracket sugar (`p[i]` / `s[i] = v`) の compiled レーン dispatch だけは `array_access.rs::lower_slice_access` / `lower_slice_assign` (struct binding を `__getitem__` / `__setitem__` 呼び出しに委譲、checker 側の arity / generic 戻り型は `type_checker/struct_literal.rs::check_struct_getitem_access` / `check_struct_setitem_access`)。tree-walker の struct 経由 bracket は `evaluation/slice.rs` |
| allocator スタック (`with allocator =`) | `evaluation/builtin.rs`, `interpreter/src/runtime_state.rs` |
| stdlib 側の policy | `core/std/allocator.t` |

## 入出力 (print / `io::`)

| 関心事 | 場所 |
|---|---|
| `print` / `println` / `eprint` / `eprintln` | builtin の解決は `frontend/src/ast/expr.rs::BuiltinFunctionSymbols`、`Display` への書き換えは `type_checker/expression.rs::apply_display_dispatch`。tree-walker は `evaluation/builtin.rs::builtin_diagnostics` → `interpreter/src/output.rs`、lowering は `compiler_lower/src/print.rs` (compound は `PrintRaw` + `Print` の列に展開) |
| **どちらの流れに出るか** (RUNTIME-LIB P0-A) | IR の `Print` / `PrintStr` / `PrintRaw` が持つ `stderr` フラグ。lowering は `FunctionLower::print_stderr` (`expr.rs` の `EPrint` / `EPrintln` arm が 1 呼び出しの間だけ立てる)、IR VM は `compiler_vm/src/dispatch.rs::emit_text`、AOT / compiler JIT は `codegen/lower_inst.rs::lower_printing` が `toy_print_stream(1/0)` で挟む (`toy_print_*` ヘルパは stdout 専用のまま 1 セット)。runtime のシンクは 2 本 — `toylang_rt` の `sink` / `err_sink` |
| str → 数値 (RUNTIME-LIB P0-B) | `core/std/parse.t` — 整数 / bool は純 toylang、`to_f64` だけ `toy_parse_f64` + ペアの status extern (interpreter は `evaluation/extern_io.rs::parse_f64`)。**文法の判定は toylang 側の `is_decimal`** — host 任せにすると `inf` / hex float / 先頭空白で受理集合がバックエンド間で割れる |
| `io::` の extern 実装 | 宣言は `core/std/io.t` (`from "toylang_rt" as "toy_io_*"` / `from "c"`)、interpreter は `evaluation/extern_io.rs::build_io_registry`、AOT / JIT は `toylang_rt` の `toy_io_*` (JIT のシンボル登録は `compiler/src/jit.rs`)。失敗は payload extern + ペアの `__extern_io_*_status` で運ぶ (RUNTIME-IO) |

## パーサ

| 関心事 | 場所 |
|---|---|
| トークン定義 / lexer 生成元 | `frontend/src/token.rs`, `frontend/src/lexer.l` |
| どの body が型検査されるか | `interpreter/src/lib.rs` の `functions` (integrate 後の**全関数** — stdlib の free function を含む。以前は `take(user_func_count)` で user 分のみ)。**型検査器は書き換えもする**ので、検査しない body は `?` / `Display` / char リテラルの書き換えが効かないまま backend に流れる |
| char リテラル (CHAR-LITERAL-NUM) | lex は `frontend/src/lexer.l` の 4 規則 (`char_literal_token` → `Kind::CharLiteral`)、AST は `Expr::CharLiteral` (pool の discriminant は `ExprType::CharLiteral`)、型は `visitor_impl.rs` が `u32` として返す。**位置の型を取る**書き換えは `type_checker/type_conversion.rs::coerce_char_literal` — `coerce_number_expr` (val / 引数 / 戻り) と `expression.rs::visit_binary` (比較・算術) の 2 経路から呼ぶ。pattern 位置は `parser/expr/match_.rs::parse_pattern_literal` |
| トップレベル宣言 | `parser/program_parser.rs` |
| 文 | `parser/stmt.rs` |
| 式 | `parser/expr/` (`mod.rs` / `primary.rs` / `control.rs` / `match_.rs` / `macros.rs`) |
| `assert_eq` / `dbg` 等のマクロ desugar | `parser/expr/macros.rs` |
| 文字列補間の desugar (`.concat` chain) | lexer 側の `{...}` 切り出しは `frontend/src/lexer.l` (`split_format_spec`)、token 合成は `parser/expr/primary.rs::parse_interpolated_string`。合成 token の位置は `StringPart::Expr.offset` + `Parser::insert_token_at` (INTERP-DIAG-SPAN) |
| format spec (`"{x:.2}"`) の文法 / pack / 描画 | `frontend/src/format_spec.rs` (**同じ bit layout の no_std 版が `compiler/runtime/toylang_rt` の `Spec`**)、実行は interpreter `evaluation/builtin.rs::format_object` / IR VM `compiler_vm/src/host.rs::format_value` (default method) / AOT・JIT `toy_format_*` |
| AST プール / 位置プール | `frontend/src/ast/pool.rs`, `frontend/src/ast/builder.rs` |
| AST キャッシュ (schema version) | `frontend/src/cache.rs` |

> `File` にフィールドを足したら **`FULL_AST_CACHE_SCHEMA_VERSION` を bump**
> すること。忘れると古い `.toycache` が新レイアウトとして deserialize され、
> **プログラムが黙って壊れる** (全 stdlib trait が "is not defined" になった実績あり)。

---

## メンテナンス

この表は**間違っていると無いより有害**。パスや関数名を変えたら同時に直すこと。
全エントリは追加時に実在を確認してある。
