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
| **IR VM** | `interpreter/src/ir_vm/` | 既定エンジン。`compiler_lower` の IR を解釈 |
| **AOT** | `compiler/src/codegen/` | 同じ IR を cranelift に落とす |
| **interpreter JIT** | `interpreter/src/jit/` | **`compiler_lower` を通らない独自経路** |

`compiler_lower` に置いた変更は **IR VM / AOT / compiler 側 JIT の 3 つ**に
効く。tree-walker と interpreter 側 JIT は別途対応が要る。
意味論を変えたら `compiler/tests/consistency.rs::assert_consistent` を使った
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
| 二項演算 | `type_checker/expression.rs::visit_binary` | `evaluation/operators.rs::evaluate_binary` | `compiler_lower/src/expr_ops.rs::lower_binary` | `ir_vm/dispatch.rs::eval_binop` |
| 算術 guard (u64 underflow) | — | `evaluation/operators.rs::evaluate_arithmetic_op_v` | `expr_ops.rs::emit_u64_underflow_guard` | (guard は IR に含まれる) |
| 代入 | `type_checker/expression.rs::visit_assign` | `evaluation/operators.rs` | `compiler_lower/src/assign.rs::lower_assign` | — |
| 演算子オーバーロード | `type_checker/expression.rs::visit_arith_binary` | `evaluation/operators.rs` | `compiler_lower/src/expr_ops.rs` | — |
| キャスト (`as`) | `type_checker/collections.rs` | `evaluation/expression.rs` | `compiler_lower/src/expr.rs` | `ir_vm/dispatch.rs` |

## 束縛・スコープ

| 関心事 | 場所 |
|---|---|
| 所有権の移動 / use-after-move (E0014) | `type_checker/move_check.rs::check_moves` (結果は `File::transferred_bindings`)。**所有の判定** (推移的 contains_drop) は `type_checker/contains_drop.rs::DropAnalysis` |
| 再帰 drop (DROP-GLUE) | glue 関数の合成: `compiler_lower/src/drop_glue.rs` (`ensure_drop_glue` / `lower_drop_glue`)。**drop を抑制 / 登録する側** — tree-walker: `interpreter/src/evaluation/mod.rs::register_drop_if_needed` + `glue_drop` (値駆動の iterative walk)、AOT/IR VM: `compiler_lower/src/lib.rs::register_drop_for_struct_binding` / `register_drop_for_enum_binding` / `register_drop_for_tuple_binding` + match arm 束縛 (`match_lowering.rs` の `arm_drop_targets`)。冪等 free + never-reuse bump ヒープ: `compiler/runtime/toylang_rt/` (AOT と compiler JIT が共有、RUNTIME_PORT R1) |
| 再帰型の検出 (E0013) | `type_checker/recursive_type.rs::check_recursive_types` (呼び出しは `interpreter/src/lib.rs::check_typing_diagnostics`)、lowering 側の保険は `compiler_lower/src/templates.rs` の `Guard` |
| `val` の型注釈チェック | `type_checker/statement.rs::visit_val_impl` |
| `var` の型注釈チェック | `type_checker/visitor.rs::process_val_type_with_mut` |
| ブロックスコープ (lowering) | `compiler_lower/src/expr.rs::lower_expr_block` |
| スコープスタック (型検査) | `type_checker/scope.rs`, `type_checker/context.rs` |
| 実行時環境 | `interpreter/src/environment.rs` |

> `val` と `var` は**別経路**。片方だけ直すと非対称になる (実際に起きた)。

## match・パターン・enum

| 関心事 | 場所 |
|---|---|
| パターン型検査 / 網羅性 / 到達性 | `type_checker/pattern_match.rs` |
| match 実行 | `evaluation/expression.rs` |
| match lowering + scrutinee 制約 | `compiler_lower/src/match_lowering.rs` |

## trait / dyn

| 関心事 | 場所 |
|---|---|
| trait 宣言・conformance | `type_checker/trait_decl.rs` |
| default method 展開 | `type_checker::expand_trait_defaults_in_pool` |
| impl block | `type_checker/impl_block.rs`, `type_checker/method.rs` |
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
| panic の位置・backtrace (P6-1) | `interpreter/src/error.rs` (`CallFrame`), `evaluation/call.rs` |
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

## テスト・検証機構

| 関心事 | 場所 |
|---|---|
| `test` ブロック (P4) | `parser/program_parser.rs` (contextual), `interpreter/src/lib.rs::run_tests` |
| 契約プロパティテスト (P5) | `interpreter/src/property.rs` |
| バックエンド一致 (単発) | `compiler/tests/consistency.rs::assert_consistent` |
| バックエンド一致 (全 example 掃引) | `compiler/tests/example_consistency.rs` |
| バックエンド一致 (CLI・D6) | `compiler/src/all_backends.rs` (`compiler f.t --all-backends`) |
| コード生成の再現性 | `compiler/tests/reproducible_build.rs` (**lowering / codegen で HashMap を反復すると link cache が全ミスになる**) |
| JIT の eligibility / fallback | `interpreter/src/jit/eligibility/` |

## メモリ・allocator

| 関心事 | 場所 |
|---|---|
| heap builtin (`__builtin_heap_*` / `ptr_*`) | `evaluation/builtin.rs` |
| `__builtin_ptr_read` の**型注釈の解釈** | **2 箇所ある**: 型検査は `type_checker/visitor_impl.rs::visit_builtin_call` の hint 許容リスト、lowering は `compiler_lower/src/let_lowering.rs::lower_let_builtin_ptr_read` (名前解決は `lower_type_arg`)。片方だけ足すと「型は通るが lower できない」/「lower はできるが型で落ちる」になる |
| allocator スタック (`with allocator =`) | `evaluation/builtin.rs`, `interpreter/src/runtime_state.rs` |
| stdlib 側の policy | `core/std/allocator.t` |

## パーサ

| 関心事 | 場所 |
|---|---|
| トークン定義 / lexer 生成元 | `frontend/src/token.rs`, `frontend/src/lexer.l` |
| トップレベル宣言 | `parser/program_parser.rs` |
| 文 | `parser/stmt.rs` |
| 式 | `parser/expr/` (`mod.rs` / `primary.rs` / `control.rs` / `match_.rs` / `macros.rs`) |
| `assert_eq` / `dbg` 等のマクロ desugar | `parser/expr/macros.rs` |
| AST プール / 位置プール | `frontend/src/ast/pool.rs`, `frontend/src/ast/builder.rs` |
| AST キャッシュ (schema version) | `frontend/src/cache.rs` |

> `File` にフィールドを足したら **`FULL_AST_CACHE_SCHEMA_VERSION` を bump**
> すること。忘れると古い `.toycache` が新レイアウトとして deserialize され、
> **プログラムが黙って壊れる** (全 stdlib trait が "is not defined" になった実績あり)。

---

## メンテナンス

この表は**間違っていると無いより有害**。パスや関数名を変えたら同時に直すこと。
全エントリは追加時に実在を確認してある。
