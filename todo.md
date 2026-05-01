# TODO - Interpreter Improvements

## 完了済み ✅

184. **`trait` 宣言と `impl <Trait> for <Type>`**: 共通インターフェースの仕組みを追加。`trait Name { fn m(self: Self, ...) -> T; ... }` でシグネチャだけを宣言、`impl <Trait> for <Struct> { ... }` で body を提供。型チェッカーが trait のシグネチャと比較して欠落メソッド・型不一致を検出（`missing method` / `parameter type mismatch` / `return type mismatch`）。型パラメータ bound `<T: SomeTrait>` を関数・struct・impl に書け、呼出時に「実型がその trait を実装しているか」を `struct_trait_impls` で検証。`Self` は impl の対象 struct に解決される。impl-trait の method は同 struct の inherent method としても登録されるため、interpreter 側のメソッドディスパッチは無変更で動作。トークン `Kind::Trait`、AST `Stmt::TraitDecl` + `Stmt::ImplBlock.trait_name: Option<DefaultSymbol>`、新規 `frontend/src/type_checker/trait_decl.rs` で conformance check。tests: `interpreter/tests/trait_tests.rs` に 10 件追加（基本宣言・impl-method dispatch・bounded-generic dispatch・複数 struct 実装・missing method / signature mismatch / 未実装 struct の bound 違反 / 重複 trait / 重複 method）。example: `interpreter/example/trait_basic.t`。`docs/language.md` に新章 *Traits*、CLAUDE.md にもキーワード追加。out of scope（後続）: trait ジェネリクス・デフォルトメソッド・複数 bound・trait 継承・`dyn Trait`・associated types (2026-04-30)
182. **Value/Reference 分離 Phase 5 後半 — variable assignment / 演算子 operand の Value 化**: `handle_variable_assignment` を Value-native に書き直し（`val.borrow()` 経由の Object クローンを Value::clone で置換、不要な `rhs_borrow` を排除）、`evaluate_binary` / `evaluate_unary` / 短絡論理演算子の operand 評価を `try_value!` (Rc allocate) → `try_value_v!` (Value 直) に置換、Phase 2 で残っていた `Value::from_rc(&lhs_val)` の中間変換を削除。`handle_val_declaration` / `handle_var_declaration` も同様。**Bench 結果 (Apple Silicon release)**:

```
                          Pre-Phase1   Post-Phase4   Post-Phase5(全)   vs Pre-P1
fibonacci_recursive       130 µs       150 µs        120 µs            -8% 高速
for_loop_sum              314 µs       349 µs        275 µs            -12% 高速
complex_expressions       35 µs        36 µs         34 µs             -3% 高速
type_inference_heavy      27 µs        27 µs         27 µs             parity
variable_scopes           65 µs        69 µs         63 µs             -3% 高速
parsing_only              34 µs        34 µs         36 µs             +6% (noise)
```

最も hot な fibonacci/for_loop で **pre-Phase1 を 8〜12% 上回る** 性能を達成。当初の目論み「primitive 値の Rc 排除で 10〜30% 改善」がようやく現実化。残るは literal eval / array slice / dict 等の cold path だが、計測可能な hot path はほぼカバー済み。tests: 492 件すべて pass (2026-04-30)
181. **Value/Reference 分離 Phase 5 — hot path consumer の Value-native 化（前半）**: `evaluate_function_with_values` のシグネチャを `args: &[RcObject] → &[Value]`、戻り値 `RcObject → Value` に変更。`evaluate_function_call` の引数評価を `try_value_v!` ベースに、`Vec<Value>` で受け取る。`evaluate_if_elif_else` の cond 評価を Value 直 match、`handle_while_loop` / `handle_for_loop` の cond / start / end も Value 経由。`execute_for_loop` 内のイテレータ束縛も Value 直接。`call_struct_method` / `call_associated_function` は legacy `RcObject` 引数を境界で `.into()`。Bench 改善 (Apple Silicon release): fibonacci_recursive 150→136µs (-9%)、for_loop_sum 349→337µs (-3%)、complex_expressions 36→34µs (-5%)、variable_scopes 69→66µs (-4%)。Pre-Phase1 (dd9ff33) 比では fibonacci +5% / for_loop +7% / complex_expressions -3% / type_inference -3% / parsing -3% — 一部 hot path で改善、残るは Phase 5 後半 (literal eval / member access / arithmetic operand 等) で更に migration が必要。tests: 492 件すべて pass (2026-04-30)
180. **Value/Reference 分離 Phase 4 — Environment を Value に**: `VariableValue.value: RcObject` → `Value`、`Environment::set_val` / `set_var` / `get_val` のシグネチャを Value 化。これにより val/var 宣言時の `Rc::new(RefCell::new(...))` 構築が primitive で消える（inline 値そのまま格納）、Identifier 参照時の `Rc::clone` も `Value::clone()` （primitive は cheap copy、Heap は Rc::clone）に置換。call.rs / statement.rs / expression.rs / lib.rs の全 set_val/set_var/get_val callsite に `.into()` 変換を挿入し、From<Object> / From<RcObject> for Value 経由で自動変換。`handle_identifier_expression` から `.borrow().is_null()` を `Value::is_null()` に直接置換、`handle_assignment` の type-check ブランチも Value 直アクセスに。tests: 492 件すべて pass (2026-04-30)
179. **Value/Reference 分離 Phase 3 — `EvaluationResult` を Value に**: `EvaluationResult::Value(Rc<RefCell<Object>>)` → `Value(Value)`、同様に `Return(Option<RcObject>)` → `Return(Option<Value>)`。`try_value!` macro はバックエンド互換性のため `v.into_rc()` で内部変換、つまり既存 consumer サイト（`val.borrow()` パターン）は無変更で動作。新規 hot path 用に `try_value_v!` macro を提供（`Value` を直接返す）。全 ~30 箇所の `EvaluationResult::Value(rc)` 構築サイトに `.into()` 追加（`From<Object>` / `From<RcObject>` for Value 経由）、`Rc::new(RefCell::new(obj))` パターンは `obj.into()` に書き換え。`evaluate_function_with_values` / `evaluate_method` の Variant ↔ RcObject 変換も整理。`Object` enum 自体はまだ primitive variants を持っており、Phase 4 以降で内部表現の最適化を進める基盤。tests: 492 件すべて pass (2026-04-30)
178. **Value/Reference 分離 Phase 2 — operators の内部 Value 化**: `operators.rs` の comparison / arithmetic / bitwise / shift / unary / short-circuit 全dispatch経路を `&Value` ベースに書き換え。`evaluate_arithmetic_op_v` / `evaluate_comparison_op_v` / `evaluate_bitwise_*_v` / `evaluate_*_shift_v` を新設、内部の primitive matching は `Object` ではなく `Value` で行う。`evaluate_binary` / `evaluate_unary` は entry で `Value::from_rc`、exit で `into_rc` する境界変換に統一。短絡論理演算子も同様に Value 経由。legacy `&Object` 受け取り版 (`evaluate_add` 等) は `object_ref_to_value` / `value_to_object` shim で Value path に flatten、API 互換は維持。tests: 492 件すべて pass、Phase 1 と合わせて Phase 3 (evaluate 戻り値を Value 化) のための基盤完成 (2026-04-30)
177. **Value/Reference 分離 Phase 1**: `src/value.rs` に新規 `Value` enum を追加。primitive variants (Bool / Int64 / UInt64 / Float64 / ConstString / Pointer / Null / Unit) は inline で持ち、composite は `Heap(RcObject)` で既存の `Rc<RefCell<Object>>` を再利用。`Value::from_rc` / `Value::into_rc` / `Value::clone_to_rc` の境界 shim を提供し、Phase 2 以降で hot path の関数シグネチャを `RcObject` → `Value` に置換しても interpreter の他部分は無変更で動く。primitive 構築・型取得・accessor (`try_unwrap_int64` 等) を実装、`debug_assert!` で `Value::heap` に primitive variant を渡すミスを検知。tests: `value.rs` に 4 件追加 (round-trip / const_string / heap-sharing / type_lookup_matches_legacy)。既存 488 件すべて green、合計 492 件 (2026-04-30)
176. **`Object::Struct` のフィールド key を `String` から `DefaultSymbol` に変更**: `fields: Box<HashMap<String, RcObject>>` → `Box<HashMap<DefaultSymbol, RcObject>>`。フィールドアクセス毎に発生していた `string_interner.resolve(*sym).to_string()` の allocation を排除し、HashMap lookup を u32 比較に。`evaluate_field_access` / `evaluate_struct_literal` / `handle_field_assignment` から resolve の呼び出しを削除（エラーパスでのみ resolve）。`Object::set` の field clone も `(*k, v.clone())` で軽量化。`to_display_string` は表示時に各 symbol を resolve する形に（フィールド数分の lookup だが一回限りなので OK）、ハッシュは symbol 数値 id でソート（テキスト ASCII 順ではなくなったが Hash 用途なので問題なし）。dead code だった `Object::get_field`/`set_field` と `ObjectError::FieldNotFound` を削除。oop_tests のテスト fixture を `DefaultSymbol::try_from_usize(N)` ベースに移行。tests: 488 件すべて pass、struct 経由の `jit_struct.t` も interpreter / JIT 両モードで exit=20 を継続 (2026-04-30)
175. **`ScalarTy::Never` で expression position の panic を JIT 化**: `BuiltinFunction::Panic` の eligibility 戻り型を `Unit` から新設 `ScalarTy::Never` に変更。`Never` は bottom type として `ScalarTy::unify_branch(a, b)` で他のどの型とも互換となり、`if cond { panic("...") } else { 5i64 }` のような expression position でも if 式全体が i64 として通る。`Expr::IfElifElse` eligibility は等値比較ではなく `unify_branch` で枝の型を統合し、`gen_if` codegen も全枝の型を unify して `cont` block param を決定する。Never 枝は codegen で `trap UserCode(1)` を出して `terminated=true` になるため cont へ jump せず、verifier は predecessor 1 のみで満たされる。`val` / `var` の RHS が Never の場合は eligibility で reject（`val x = panic("...")` は意味のないコードだが silent fallback で interpreter が正常 panic させる）。`ir_type(Never) = None`、main の Never 戻りは jit_panic helper の `process::exit` 経由で reach 不可。example: `jit_panic_expr.t` / `jit_panic_expr_fail.t`、tests: `jit_integration` に 1 件追加（成功・失敗両パスで `divide` が JIT 化されることを確認）。JIT.md 更新、Known limitations から「expression-position panic 不可」を削除 (2026-04-30)
174. **`panic` / `assert` を release でも常時 active と決定（運用方針をドキュメント化）**: `INTERPRETER_PANIC` / `INTERPRETER_ASSERTIONS` 系 env-var の追加は見送り。理由: D の `-release` で assert が消え本番でバグが顕在化する事例の再発を避けるため、type-safe に切れる assert も含めて release で残す方針。`docs/language.md` の Known limitations から「No release-mode gate for panic / assert」を削除し、代わりに「intentionally always-on by design」セクションに置換。`docs/language.md` / `interpreter/README.md` / `README.md` の `INTERPRETER_CONTRACTS` 説明箇所に「`all` を本番でも維持するのが推奨。`pre`/`post`/`off` は計測可能なホットパスのみで使うこと」「panic/assert は build profile 間で挙動を一致させるため意図的に gate しない」運用警告を追記 (2026-04-30)
173. **`assert(cond, msg)` ビルトイン + JIT 対応**: `panic` の隣人として追加。`BuiltinFunction::Assert`、シンボル `assert`（user-facing 名）、type_checker は `(bool, str) -> ()`。interpreter は cond を bool として評価し、true なら Unit、false なら msg を to_display_string で文字列化して `InterpreterError::Panic { message }`。message は false 時にのみ評価される（lazy）。JIT は `brif cond, cont_blk, fail_blk; fail_blk: call jit_panic(msg_sym); trap UserCode(1); cont_blk: …` で lower、success path は1分岐コスト、failure path は `panic("literal")` と同じ `jit_panic` helper に集約。message は `Expr::String(_)` のみ accept（panic と同じ制約）。example: `jit_assert.t`、tests: `language_core_tests` に 3 件 / `jit_integration` に 3 件追加。docs/language.md / CLAUDE.md / JIT.md 更新、Known Limitations の「assert 無し」削除（代わりに「panic/assert の release-mode gate 無し」を残置） (2026-04-30)
172. **`panic("literal")` の JIT 対応（Option B: symbol id 渡し）**: cranelift JIT で `panic("literal")` をサポート。`HelperKind::Panic` を追加し、`extern "C" fn jit_panic(sym_id: u64)` が thread-local `JIT_STRING_INTERNER` 経由で program の `&DefaultStringInterner` を `*const` で borrow して symbol → &str を resolve、`Runtime error occurred:\npanic: <msg>` を stderr に出して `process::exit(1)`。`execute_cached` 内で raw pointer を install/clear（HeapGuard と同じパターン）。eligibility は `Expr::String(sym)` のときのみ accept、それ以外は既存の "unsupported builtin" 経路。codegen は `iconst u64 sym.to_usize()` + `call_helper(Panic)` + `trap UserCode(1)` を emit、`state.terminated = true`。trap は helper が exit するため dead code、CFG terminator として cranelift verifier を満たす目的のみ。expression position（`if cond { panic("...") } else { value }`）は branch 型不一致で silent fallback（将来的には `ScalarTy::Never` で対応予定）。example: `jit_panic.t`、tests: `jit_integration` に 2 件追加（JIT compiled + helper 経由 panic / dynamic 引数の fallback）。JIT.md 更新 (2026-04-29)
171. **`panic("msg")` ビルトイン**: 実行を中断する終了用 builtin。`BuiltinFunction::Panic` を AST に追加、シンボルは `panic`（`__builtin_` prefix 無しの user-facing 名）。type_checker のシグネチャテーブルで 1引数 `str` → `Unknown` として登録、Unknown を「発散する式の型」として if-elif-else の枝統一と関数 body の戻り型一致判定でワイルドカード扱い（`if cond { panic("...") } else { 5i64 }` や `fn foo() -> i64 { panic("not impl") }` がそのまま通る）。interpreter は `InterpreterError::Panic { message }` を返して停止、表示は `panic: <message>`。JIT は既存の catch-all `unsupported builtin` 経路で silent fallback。example: `panic.t`、tests: `language_core_tests` に 3 件追加（基本 / if-then 位置で型ユニファイ / const をメッセージに使う）。docs/language.md / CLAUDE.md 更新、Known Limitations から「panic 無し」を削除し「assert 無し」だけ残置 (2026-04-29)
170. **トップレベル `const` 宣言**: `const NAME: Type = expression` を関数の外側に書けるようにした。`Kind::Const` トークン、`Program.consts: Vec<ConstDecl>`、parser で `pub? const NAME: Type = expr` を読む経路、type_checker でグローバルスコープに `set_var` で登録（前方参照不可）、interpreter で main 呼出前に各 const を順番に評価して `environment.set_val`。型ミスマッチは「Const `X` declared as ... but initializer has type ...」の専用エラー。JIT は const を参照する関数を silent fallback（const 値は eligibility walker からは未知の identifier に見える）。example: `const_decls.t`、tests: `language_core_tests` に 5 件追加（基本利用 / 関数からの参照 / f64 const / 先行 const 参照 / 型エラー）。docs/language.md / CLAUDE.md / JIT.md を更新 (2026-04-29)
169. **言語リファレンス `docs/language.md` を新設**: 構文・型・式・文・関数・struct / impl・enum / match・generics / bounds・modules・allocators・builtins・Design by Contract・runtime model・known limitations を 1 ファイルに集約。CLAUDE.md / README.md からはトップで `docs/language.md` を「正本」として案内し、言語仕様の重複を許容しつつ最新は language.md を参照する形に。隣接ドキュメント（JIT.md, ALLOCATOR_PLAN.md, BUILTIN_ARCHITECTURE.md, interpreter/README.md）は実装者向けとして残しリンク (2026-04-29)
168. **`InterpreterError::PropagateFlow` の除去とフロー伝搬バグ修正**: `extract_value` が制御フロー (Return/Break/Continue) を `Err(PropagateFlow(_))` に詰めて伝搬していたが、誰もキャッチせず関数 / ループ境界をすり抜けて user に「Propagate flow: …」を表示する潜在バグを発見（`val y = if cond { return X } else { Y }` で再現）。`extract_value` を `try_value!` macro と `unwrap_value` に分離。前者は flow を `return Ok(flow)` で関数の caller に正しく伝搬、後者は flow を許さない位置（contract 述語、pattern literal）で flow を InternalError 化。`InterpreterError` から `PropagateFlow` variant 削除（API leak 解消）。`handle_val_declaration` / `handle_var_declaration` の戻り型を `Result<Option<EvaluationResult>, _>` → `Result<EvaluationResult, _>` に統一し、`EvaluationResult::None` で「値を生まない statement」を表現。tests: `language_core_tests` に regression 2 件追加（return が if-then / else 位置から正しく function return 値になる） (2026-04-29)
167. **DbC release mode（`INTERPRETER_CONTRACTS` env var）**: `requires` / `ensures` を独立に切替可能な runtime gate。`ContractMode { check_pre, check_post }` を `evaluation::mod` に追加し、`EvaluationContext::new` で env を 1 回読む。値は `all|pre|post|off`（unset = `all`、case-insensitive、`on/1/true` / `0/false` も受け付け、未知値は stderr 警告 + `all` フォールバック）。`evaluate_function_with_values` の requires/ensures ブロックと、method 経路の `evaluate_method_requires` / `evaluate_method_ensures` ヘルパー先頭で gate。テストは `tests/contract_mode_tests.rs` に 7 件（process spawn ベース、各モード × pre/post 違反プログラム）。CLAUDE.md / README.md にも記載 (2026-04-29)
166. **Design by Contract（`requires` / `ensures`）**: 関数とメソッドの `-> ReturnType` の後、body `{` の前に `requires <bool_expr>` / `ensures <bool_expr>` 節を複数並べられる（案 1 多節形式）。`Function` / `MethodFunction` AST に `requires: Vec<ExprRef>` / `ensures: Vec<ExprRef>` を追加。parser は contract clause 列をパース（`Condition` context で struct literal 抑止）。type_checker は各節を bool で検証し、`ensures` では `result` 識別子を戻り値型で binding。interpreter `evaluate_function_with_values` / `call_method` / `call_associated_method` で entry 時に `requires` を、exit 時に `result` を bind して `ensures` を評価し、違反時に `InterpreterError::ContractViolation { kind, function, clause_index }` を返す。JIT は contract を持つ関数を eligibility で reject（silent fallback）。`old(...)` と名前付き return（案 4）は今回スコープ外。example: `contracts.t`、tests: `language_core_tests` に 6 件追加（pass / requires違反 / ensures違反 / 多節での clause index / メソッド contract / 非 bool 節は型エラー） (2026-04-29)
165. **f64 (浮動小数点数) サポート**: `TypeDecl::Float64`、`Kind::F64` / `Kind::Float64(f64)`、lexer に `1.5f64` / `42f64` パターン（タプルアクセス `t.0.1` との曖昧性回避のため `f64` サフィックス必須）、`Expr::Float64(f64)`、`Object::Float64(f64)`。算術 (`+ - * / %`)、比較 (IEEE 754 ordered)、unary minus、`as` による i64/u64 ↔ f64 変換、`__builtin_sizeof = 8` を実装。Hash/Eq/Ord は `to_bits()` でビット等価ベース（NaN を Dict キーに使えるように total order）、表示は `1.0` のように常に小数点付き。JIT も対応：`ScalarTy::F64` を追加し、`fadd/fsub/fmul/fdiv` と `fcmp` (Ordered)、`fneg`、`fcvt_from_sint/uint` および `fcvt_to_sint/uint_sat`（Rust の `as` と一致）、`jit_print_f64` / `jit_println_f64` ヘルパー、`main` の f64 戻り値を `Object::Float64` に詰め直し。f64 mod は cranelift にネイティブ命令が無いため eligibility で reject（silent fallback）。example: `float64.t` / `jit_float64.t`、tests: `language_core_tests` に 7 件、`jit_integration` に 2 件追加 (2026-04-28)
164. `%` 剰余演算子と複合代入 (`+= -= *= /= %=`): lexer/token/AST に `IMod` および `PlusEqual` 系トークンを追加。parser の `parse_mul` で `%` を *,/ と同じ優先度で扱い、`parse_assign` 入口に複合代入 dispatch を追加。複合代入は `lhs op= rhs` を `lhs = lhs op rhs` に desugar (LHS は identifier / `FieldAccess` 対応、SliceAccess も既存 SliceAssign 経路で動く)。type_checker は既存の `IAdd | ISub | IDiv | IMul` ケースに `IMod` を merge。interpreter は `ArithmeticOp::Mod` を Rust の `%` で実装 (truncated remainder)。JIT は cranelift の `srem`/`urem` で実装。example: `modulo_compound.t`、tests: `language_core_tests` に 5 件追加 (2026-04-28)
163. JIT タプル対応 (flat scalar tuples): `ParamTy::Tuple(Vec<ScalarTy>)` を導入、tuple 型の関数 param / return / val / var / TupleAccess / TupleLiteral RHS / tuple-returning call / tuple alias を JIT eligibility と codegen に追加。tuple param は要素ごとに cranelift param に分解、tuple return は multi-return、TupleAccess は要素 SSA Variable から `use_var`。`val (a, b) = expr` 分解は parser desugar (`val tmp = expr; val a = tmp.0; val b = tmp.1`) 経由で自動的に動く。tuple 引数は名前付き local 必須 (inline literal は不可)。Out of scope: ネストタプル、tuple-of-struct、main の tuple return。example: `jit_tuple.t`、tests: `jit_integration` に 2 件追加 (2026-04-27)
162. ネストした val/var タプル分解: `parse_tuple_destructuring` を `DestructPat { Name | Tuple }` 木で再帰化、`emit_destructure` が深さに応じて `__tuple_tmp_N` を連鎖させる。outer `is_val/is_var` は leaf binding にのみ伝播し、内部 tmp は常に `val`。`val ((a, b), c) = ...` / `val ((a, b), (c, d)) = make()` / `val (((a, b), c), d) = ...` / `var ((a, b), c) = ...` + 再代入が動作。example: `tuple_destructure_nested.t`、tests: `collections_tuple_struct_tests` に 4 件追加 (2026-04-27)
161. match arm guard: `match x { v if v < 0 => …, _ => … }` のように pattern と `=>` の間に `if <bool>` を置ける。AST は `MatchArm { pattern, guard: Option<ExprRef>, body }` 構造体に統一、parser は guard 式を `Condition` context で読む（struct literal 禁止）、type_checker は guard を `Bool` 型でチェックし pattern bindings を可視に保つ。guarded arm は exhaustiveness で wildcard 扱いせず、literal/enum-variant の "fully covered" マークも付けないので網羅性が緩まない。interpreter は pattern 一致後に guard を評価し false なら次の arm にフォールスルー（bindings はスコープごと破棄）。example: `match_guard.t`、tests: `collections_tuple_struct_tests` に 5 件追加。JIT は match を従来どおり silent fallback (2026-04-27)
160. match のタプルパターン: `Pattern::Tuple(Vec<Pattern>)` を AST に追加。parser で `( p, q, ... )` を 2 要素以上のタプルパターンとして認識、type_checker は `ScrutineeKind::Tuple(Vec<TypeDecl>)` を導入し各要素を `check_sub_pattern` で再帰検証、interpreter は `Object::Tuple` の対応要素を順に sub-pattern に渡す。irrefutable な (`_` / 名前束縛のみの) タプルパターンは exhaustiveness で wildcard 扱い、リテラル混在の場合は wildcard 必須。ネストしたタプルパターン (`((a, b), c)`) も動作。example: `match_tuple.t`、tests: `collections_tuple_struct_tests` に 3 件追加 (2026-04-26)
159. タプル `val (a, b) = expr` / `var (a, b) = expr` 分解: パーサ desugar で隠し temporary + 各名へ `tmp.0`, `tmp.1`, … で bind。`Parser.pending_prelude_stmts` を `parse_block_impl` が drain して source 順に展開。3 要素以上、関数戻り値の分解、`var` 形式と再代入の組み合わせも動作。example: `tuple_destructure.t`、tests: `collections_tuple_struct_tests` に 4 件追加 (2026-04-26)
158. JIT Phase 2e (allocator stack): JIT runtime に allocator registry + active stack を追加。`__builtin_default_allocator()` / `__builtin_arena_allocator()` / `__builtin_current_allocator()` は registry index (u64) を返し、`with allocator = expr { … }` は push + body + pop でディスパッチ。heap_alloc 系 callback は active 先頭の allocator を経由。`with` body は linear 限定 (return/break/continue 不可)。`ScalarTy::Allocator` を追加。example: `jit_allocator.t` (2026-04-26)
157. JIT Phase 2d-4 (struct method dispatch): `MonoTarget::Method(struct, method)` を導入、`MonomorphSource` enum で Function/Method を統一。method 本体を `self: Self` 入りの普通の関数として codegen、`p.method()` 呼出は receiver を struct arg に展開して通常の Call と同じ経路。`Self` は monomorph 時点で受領 struct に解決。Out of scope: 動的 dispatch、generic method。example: `jit_method.t` (2026-04-26)
156. JIT Phase 2d-3 (struct return / multi-return): `FuncSignature.ret` を `ParamTy` 化、struct return は cranelift signature の returns に layout 順展開。codegen は struct-returning 関数の body 末尾 (Identifier or StructLiteral) を gather して return_、Call site は val/var RHS で multi-result から struct local を再構築。main return は scalar 限定。example: `jit_struct_return.t` (2026-04-26)
155. JIT Phase 2d-2 (struct as func parameter): `ParamTy::Struct(name)` を導入、関数 param が struct のとき各 scalar field を別 cranelift param に分解。codegen は entry block で param 値群を struct_locals の Variable に振り分け、Call site は `Identifier(struct_local)` 経由で field values に展開。Out of scope: struct return (multi-return が必要)。example: `jit_struct_param.t` (2026-04-26)
154. interpreter unused-variable warnings 整理: `destruction_log!` macro が release build (debug-logging feature 無し) で no-op になり、引数で参照される binding が未使用になる問題を `#[allow(unused_variables)]` で抑止。debug/release/--no-default-features/--all-targets 全 4 profile で 0 warning に (2026-04-26)
153. JIT Phase 2h (関数コンパイルキャッシュ): thread_local で `&Program` ポインタ identity を key に `JITModule` + `main_ptr` + return ScalarTy を保持。連続呼出で eligibility/codegen/finalize をスキップ。bench (Apple Silicon release): fib_recursive 107µs→31µs、loop_sum_100k 134µs→31µs、fib_iter_50k 106µs→31µs。speedup vs interpreter は 451×〜1741× に向上 (2026-04-26)
152. JIT Phase 2d (struct field アクセス): scalar フィールドのみの struct を JIT 対応。各フィールドを別 SSA Variable として decompose し、StructLiteral RHS / FieldAccess 読み出し / `p.field = value` 書き込みを許可。out-of-scope: struct copy, struct as func param/return, methods, nested struct, generic struct。example: `jit_struct.t` (2026-04-26)
151. JIT Phase 2f (generic monomorphize): EligibleSet を `MonoKey = (Symbol, Vec<ScalarTy>)` keyed に refactor。`Call(generic_fn, args)` を見たとき arg 型から substitutions を推論、各 monomorph を別 cranelift 関数 (`id__I64`/`id__U64` 等) として compile。call_targets で各 Call → MonoKey を解決。generic 関数内で PtrRead は ineligible (typed-slot hint が ExprRef-keyed なため)。example: `jit_generic.t` (2026-04-26)
150. JIT 機能ドキュメント (`JIT.md`): サポート範囲、env var、性能数値、skip 理由、example 一覧、未実装項目を 1 ファイルにまとめ。CLAUDE.md からも参照 (2026-04-26)
149. JIT Phase 2g (`__builtin_sizeof` 対応): scalar 型 (i64/u64/ptr=8、bool=1) でコンパイル時定数を返す。eligibility は引数 1、JIT-対応 scalar、戻り値 u64。codegen は arg を gen_expr して値を捨て (副作用保存) iconst を返す (2026-04-26)
148. JIT パフォーマンス計測: `interpreter/benches/jit_bench.rs` で interpreter / JIT を比較。実測 (Apple Silicon, release): fib_recursive(20) 13.65ms→107µs (127×)、loop_sum(100k) 51.6ms→134µs (383×)、fib_iter(50k) 39.2ms→106µs (371×)。JIT 側は cranelift コンパイル込み。`--no-default-features` ビルド成立 (2026-04-26)
147. JIT skip 理由の詳細化: `analyze` が `Result<EligibleSet, String>` を返すように変更、各 reject 点で `note(reason, ...)` で具体的な理由 (関数名 + 構文要素 / unsupported builtin / ptr_read の type-hint 欠落 等) を記録。`-v` で `JIT: skipped (function `main`: uses unsupported expression array literal)` 形式で出力 (2026-04-26)
146. JIT Phase 2c-2 (ptr_read/ptr_write 対応): 8 helper を追加 (read/write × i64/u64/bool/ptr)。eligibility が val/var/assign の左辺型から `__builtin_ptr_read` の期待型を pre-pass で収集し `ptr_read_hints: HashMap<ExprRef, ScalarTy>` に格納。codegen は hint で helper を選択。callback は `HeapManager::typed_read/typed_write` を経由し interpreter と互換 (2026-04-26)
145. JIT 統合テスト追加: `interpreter/tests/jit_integration.rs` で `INTERPRETER_JIT=1` ON/OFF のバイナリ実行を比較。fib/jit_cast/jit_print/jit_heap で exit code + stdout 往復一致、fallback プログラム (配列使用) の挙動、verbose ログ (`JIT compiled:` / `JIT: skipped`) の存在を検証。8 テスト追加 (--no-default-features では 5 テスト) (2026-04-26)
144. JIT Phase 2c (heap builtins): `heap_alloc`/`heap_free`/`heap_realloc`/`ptr_is_null`/`mem_copy`/`mem_move`/`mem_set` を JIT で扱う。`ScalarTy::Ptr` を追加 (cranelift I64 マップ)、callback は thread_local の `JIT_HEAP` で `HeapManager` を共有、`PtrIsNull` は `icmp_imm` でインライン展開。`ptr_read`/`ptr_write` は typed-slot 仕様の都合で次回 (2026-04-26)
143. JIT Phase 2b (print/println callback): `BuiltinCall(Print/Println, scalar_arg)` を JIT で扱う。`extern "C"` Rust callback (jit_print_i64/u64/bool + println 各種) を `JITBuilder.symbol()` で登録、`Linkage::Import` で declare、codegen は引数型から helper を選んで call。eligibility は arg=1, type∈{i64,u64,bool} を許可、return type は Unit (2026-04-26)
142. JIT Phase 2a (Cast 対応): `Expr::Cast` を eligibility/codegen に追加。i64 ↔ u64 (identity 含む) のみ対応。両者ともクランリフトの I64 にマップされるため codegen は no-op (2026-04-26)
141. main の数値戻り値を process exit code に: `Object::Int64`/`UInt64` のときに `process::exit` で値を返す。fib なら `cargo run example/fib.t` の終了コードが 8 になる (2026-04-26)
140. cranelift-based JIT (Phase 1): `INTERPRETER_JIT=1` env var で opt-in、cargo feature `jit` (default on)。i64/u64/bool/Unit のみ使う関数 (`main` から transitively reachable) を一括コンパイル。リテラル/算術/比較/論理 (短絡)/ビット/シフト/単項/val/var/代入/if-elif-else/while/for-range/break/continue/return/関数呼び出しに対応。サポート外は silent fallback (`-v` で skip 理由表示)。設計は `~/.claude/plans/mutable-wobbling-kettle.md` (2026-04-26)
139. `__builtin_sizeof` の struct / enum / tuple / array 対応: struct はフィールド合計、enum は 1-byte タグ + payload 合計（variant 依存）、tuple / array は要素合計。`List<Option<i64>>` のようなケースで stride 計算に利用可能 (2026-04-22)
138. 任意型 T に対応した `ptr_write` / `ptr_read`: HeapManager に typed-slot map を追加、write は任意型の RcObject を保存、read は型ヒント（`val v: T = ...`）に従って返す。`List<i64>` / `List<bool>` / `List<T>` の実用的な動作 (2026-04-22)
137. Allocator を型パラメータに取る struct: `struct List<T, A: Allocator>` 形式。struct 生成時に型注釈をヒントとしてフィールドに現れない T を推論、メソッド内の `Self` 再構築に return type ヒントを伝播、struct-level bound を impl body へマージ、block レベルの型ヒント上書きを修正 (2026-04-22)
136. `__builtin_sizeof(value)` builtin: 引数の型のバイトサイズを u64 で返す。generic `T` の実体サイズを取得するジェネリックコレクションの土台。現状 primitive（u64/i64/bool/ptr/unit）のみ対応、struct/enum/str は未対応 (2026-04-22)
135. match の文字列リテラルパターン: `"hello" => ...` で分岐可能。scrutinee 型に `str` を追加、重複リテラルは unreachable エラー、wildcard 必須 (2026-04-22)
134. match のネストパターン: タプル variant のサブパターンに再帰的なパターンを書ける（`Option::Some(Option::Some(v))`、`Box::Put(Color::Red)`、`Some(42i64)`）。`Pattern` を再帰構造に統合し `PatternBinding` を削除、型ヒントをネスト構築に伝播、irrefutable 判定で不要な unreachable を避ける (2026-04-22)
133. match のリテラルパターン: primitive scrutinee（`bool`/`i64`/`u64`）に対して `0i64 =>`、`true =>` のようなリテラルで分岐可能。bool は両値網羅、整数は wildcard 必須、重複リテラルは unreachable エラー (2026-04-22)
132. Enum + match（Phase 3）: ジェネリック `enum Option<T> { None, Some(T) }`。タプル variant 引数からの型パラメータ推論、ユニット variant の型注釈ヒント補完、match パターンバインディングでの型パラメータ置換 (2026-04-21)
131. match の到達性チェック: `_` 以降の arm / 同一 variant の重複 arm を型チェックエラーとして検出 (2026-04-20)
130. Range literal を式として利用可能に: `0u64..10u64` を式位置で使える。`for i in 0..n` と `val r = 0..n` の両方が動作、`to` 形式も互換維持。`Object::Range`、`TypeDecl::Range(Box<T>)` 追加 (2026-04-19)
129. Enum + match（Phase 2c）: 網羅性チェック。wildcard なしで variant 欠落の場合に型チェックエラー。欠けている variant 名をエラーに明示 (2026-04-19)
128. Enum + match（Phase 2）: タプル variant `Shape::Circle(i64)`, `Rect(i64, i64)` のコンストラクタ、バインディングパターン `Circle(r)` と `_` discard、型チェックの payload 型検証 (2026-04-19)
127. Enum + match（Phase 1）: `enum Name { A, B, C }` unit variant、`Color::Red` バリアント参照、`match scrutinee { pat => body, _ => body }` による分岐。型チェックは全 arm の型一致と variant 存在を検証 (2026-04-19)
126. 非ジェネリック struct の associated function 対応: `List::new()` 形式が generic struct なしで動作、メソッドチェーンの return type 正規化 (2026-04-19)
125. struct field 代入 `obj.field = x` サポート: interpreter の handle_assignment に FieldAccess LHS 追加、Counter.inc() 等の imperative スタイルが書けるように (2026-04-19)
124. Allocator システム実装（Phase 1a/1b/1c/2a/2b + Phase 3 部分）: `with allocator = expr { ... }` 構文、`TypeDecl::Allocator`、`Object::Allocator(Rc<dyn Allocator>)`、`Allocator` trait + Global/Arena/FixedBuffer、`<A: Allocator>` bound（関数・struct・impl）、bound 連鎖、`ambient` 糖衣、自動 ambient 挿入、ユーザ空間 List<u64> 対応。設計・進捗は `ALLOCATOR_PLAN.md`、使用例は `interpreter/example/allocator_*.t` (2026-04-19)
123. ヒープメモリ管理の完全実装: heap_alloc/free/realloc、ptr_read/write/is_null、mem_copy/move/set を allocator stack 経由でルーティング (2026-04-19)
122. 動的配列（List 型）ユーザ空間対応: `struct List { data: ptr, len: u64, cap: u64 }` + impl + heap builtin で push/get/imperative な growth を記述可能 (2026-04-19)
120. interpreter/evaluation.rs (2632行) を evaluation/ モジュール7ファイルに分割: operators/expression/statement/call/slice/builtin/mod に責務分離 (2026-04-19)
119. parser/core.rs (1038行) を core/types/declarations/program_parser に4分割: パース責務ごとに独立 (2026-04-17)
118. type_checker.rs (1000行) を visitor/visitor_impl/module_access に3分割: Acceptable/ProgramVisitor/AstVisitor実装を切り離し (2026-04-17)
117. ast.rs (1647行) を ast/{expr,pool,program,builder}.rs に分割: 責務別5ファイル構成、re-exportで後方互換維持 (2026-04-17)
116. type_checkerコード重複削減リファクタ: シンボル解決/エラー位置付加ヘルパーの統一、__getitem__アクセスロジック統合で正味52行削減 (2026-04-17)
115. CLAUDE.mdにlexer定義のキーワード・演算子を追記 (2026-02-28)
114. テストスイート大規模改善・統合: frontend 26→16, interpreter 41→11ファイル、99テスト追加、合計787テスト (2026-02-28)
113. ジェネリック構造体高度テスト7件の失敗修正: bare `self`、`val`キーワード競合、`else if`バグ回避 (2026-02-27)
112. ネスト配列型推論と改行対応パース修正: `[[u64;2];3]`の型推論正常動作 (2026-02-27)
111. C++11スタイル`>>`トークン分割: `Container<Container<T>>`のネストジェネリック型パース対応 (2025-12-10)
110. パーサーでのジェネリック型引数サポート: `Container<T>`パースと関連関数戻り値型の完全な型置換 (2025-12-10)
109. ジェネリック構造体フィールドアクセス型パラメータ置換: `Container<u64>.value`が正しく`u64`を返す (2025-12-09)
108. 単一型パラメータGenericsの基本実装: 関数・構造体でのジェネリクス構文パース (2025-09-07)
107. 負数インデックス推論修正: `a[-1]`が自動的にi64として推論 (2025-09-06)

## 未実装 📋

160. **タプルの追加 JIT 対応** — フラットなスカラーtupleの param / return / TupleAccess / destructure / tuple-returning call は完了 (`#163`)。残: ネストタプル (`((a,b),c)`) と tuple-of-struct を JIT codegen で扱う (現状 silent fallback)、inline tuple literal を call argument として渡せるようにする
159. **JIT Phase 2 拡張** — Phase 1 / 2a-2h / 2c-2 / 2d-2/3/4 / 2e (allocator stack) は完了。残: `__builtin_fixed_buffer_allocator`、`with` 内の早期 exit (return/break/continue) サポート、generic 構造体 / メソッド。サポート範囲のまとめは `JIT.md`
96. **Enum/match 拡張** — Phase 1/2/2c/3 + リテラル + ネスト + 文字列リテラルパターン完了。標準 Option/Result ライブラリ、深い網羅性解析は未実装
29. **Option<T> を標準的に提供** — ジェネリック enum は動作中。ユーザ空間で書ける（`enum Option<T> { None, Some(T) }`）。標準ライブラリとして組み込むかは別議論
30. **組み込み関数システム** — 型変換（u64 ↔ i64 は既に `as` で可能）、数学関数（`abs`, `min`, `max`, `pow`, `sqrt`）
65. **frontendの改善課題** — docコメント拡充、プロパティベーステスト追加、コード重複削減
26. **ドキュメント整備** — 言語仕様 / API ドキュメント
121. **Allocator システム残作業** — `__builtin_sizeof`（primitive/struct/enum/tuple/array）、`struct List<T, A: Allocator>`、任意型 T 対応の `ptr_write`/`ptr_read` 実装済み。残り: IR レベルの `AllocatorBinding`、Phase 4 以降の native codegen（詳細は `ALLOCATOR_PLAN.md`）
183. **コンパイラの作成（MVP + IR + panic/assert + print/println + struct + cast/f64 + struct boundary + tuple + const + DbC + --release + nested fields + tuple boundary + 3 経路一致テスト 対応・段階的進行中）** — toylang のソースを実行可能バイナリにコンパイルする独立コンポーネントを新設する。

   **2026-05-01: 3 経路一致テストに JIT を追加** — `compiler/tests/consistency.rs` が interpreter (lib API) / compiler (subprocess) / **JIT (interpreter binary spawn with INTERPRETER_JIT=1)** の 3 経路で同一 exit code を保証するように拡張。`OnceLock` で interpreter binary を一度だけビルドしてキャッシュ、各テストで `Command::new(bin).env("INTERPRETER_JIT", "1").arg(src)` で spawn。3-way assertion は interp vs compiler / interp vs jit を別々に diff 表示するため、どのペアがズレたか明示できる。新機能カバレッジ向上のため struct / tuple / const / DbC のケースも追加（計 14 テスト 全 green）。**発見した既知差分**: u64 underflow 時、interpreter は overflow panic、compiler / JIT は wrap。consistency テストでは underflow を起こさないオペランドを選んで意図しないドリフトのみを catch する設計に。

   **2026-05-01: tuple cross-boundary を追加** — `TupleId(u32)` newtype と `Type::Tuple(TupleId)` を IR に追加、`Module.tuple_defs: Vec<Vec<Type>>` で tuple shape を intern（linear-search dedup）。`InstKind::CallTuple { target, args, dests }` を `CallStruct` と並列で追加。`lower_param_or_return_type` が `TypeDecl::Tuple(elements)` を受理（scalar element 限定）し `intern_tuple` で TupleId を取得。`allocate_tuple_elements` で tuple param を per-element local に展開、`lower_tuple_literal_tail` で tail-position tuple literal を `pending_tuple_value` に貯めて implicit return が消費。`lower_let` の Call 検知が `CallTuple` も emit、explicit `return p` が tuple binding 経由 / tuple literal 経由で expand。codegen の `flatten_struct_to_cranelift_tys` を `Type::Tuple` も再帰展開するよう拡張、`InstKind::CallTuple` を multi-result call として lower。e2e テスト 4 件追加（tuple return / tuple param / round trip / call-into-destructure）、計 49 テスト全 green。

   **2026-05-01: --release ゲート と ネストした struct field を追加** — `CompilerOptions.release: bool` を追加、`--release` 指定で `lower.rs` が requires/ensures 検査を一切 emit しない（interpreter の `INTERPRETER_CONTRACTS=off` と同等）。ネストした struct: `FieldBinding` を `{ name, shape: FieldShape }` に refactor、`FieldShape` enum で `Scalar { local, ty }` と `Struct { struct_name, fields }` を表現。`collect_struct_defs` は struct field type も accept、`allocate_struct_fields` ヘルパーで再帰的に local 展開、`store_struct_literal_fields` ヘルパーで入れ子リテラル `Outer { inner: Inner { x: 1 } }` を再帰 store。`a.b.c` chain access は `resolve_field_chain` で walk、`FieldChainResult::Scalar` / `Struct` を返す。codegen の `flatten_struct_to_cranelift_tys` で nested struct param/return を leaf scalar まで再帰展開。e2e テスト 3 件追加（release skip 検証 / nested field read+write / nested struct param）、計 45 テスト全 green。

   **2026-05-01: tuple / top-level const / DbC を追加** — tuple は struct と同じ「per-element ローカル展開」パターンで `Binding::Tuple { elements }` を新設、`Expr::TupleLiteral` を val rhs として allocate、`Expr::TupleAccess` の読み書きを実装。tuple は局所バインディング限定（関数引数 / 戻り値は未対応）、`val (a, b) = (x, y)` のパーサ desugar が自然に動く。const は `evaluate_consts` で program.consts を compile-time fold（リテラル + 既存 const 参照 + arithmetic）し、Identifier 解決時に local binding が無ければ const テーブルにフォールバック。DbC は `lib.rs::ContractMessages` で "requires violation" / "ensures violation" を pre-intern して `&mut interner` 問題を回避、`emit_contract_checks` で各 clause を bool 評価 → 失敗時 `Terminator::Panic` に分岐、`requires` は entry で / `ensures` は全 Return 直前で発火。`ensures` の `result` は scalar 戻り値（struct は first field）に bind。e2e テスト 9 件追加（tuple 4 + const 2 + DbC 3）、計 42 テスト全 green。

   **2026-05-01: cast (`as`) / f64 / struct boundary crossing を追加** — IR に `Type::F64` / `Const::F64` / `InstKind::Cast { value, from, to }` / `InstKind::CallStruct { target, args, dests }` / `Module.struct_defs` を追加、`Terminator::Return` を `Vec<ValueId>` に変更（scalar / void / struct return を vec 長で表現）。lower で `Expr::Cast`、`Expr::Float64`、struct 引数 / 戻り値、`val x = struct_returning_call()` を扱う。tail-position の struct literal は `pending_struct_value` に貯めて implicit return が消費する設計（IR の SSA グラフに struct 値が流れない）。codegen で struct sig を per-field cranelift param / multi-return に展開、`Type::F64` 演算は cranelift の `fadd/fsub/fmul/fdiv/fcmp` + `fneg`、cast は `fcvt_from_sint/uint` / `fcvt_to_sint/uint_sat` で。runtime に `toy_print_f64` / `toy_println_f64`（`%g` / `%.1f` 切替）追加。e2e テスト 10 件追加（i64↔u64 cast、float-int round trip、float-int truncate、f64 算術 / unary neg / 関数呼び出し、struct return / param / round trip / 明示 return）。

   **現在の制約（2026-05-01 時点、live state）** — 詳細は `compiler/README.md`。以下の機能は未対応で、検出時は明確なエラーで reject される:

   - **型**: `i64` / `u64` / `f64` / `bool` / `Unit`、scalar フィールドのみの struct、scalar 要素のみの tuple のみ。`str` は値としては未対応（リテラルのみ）、`ptr` 未対応、`Allocator` 未対応
   - **文字列**: 任意の文字列値（`val s = "foo"` 等）は未対応。文字列リテラルは `panic` / `assert` / `print` / `println` 引数としてのみ受理
   - **キャスト**: `as` で i64↔u64（identity）と {i64,u64}↔f64 はサポート。bool との cast、Unit との cast は不可
   - **f64 制約**: `%` (mod) は cranelift に native fmod が無いため reject
   - **tuple**: 局所バインディング・要素アクセス・要素書き込み・分解、関数引数 / 戻り値として tuple 値を渡せる（codegen が per-element 展開）、tuple-returning call も `val (a, b) = f()` で受けられる。ネストした tuple は未対応
   - **コレクション**: 配列、dict 全般未対応（リテラル / アクセス いずれも reject）
   - **enum / match**: `enum` 宣言と `match` 式いずれも未対応
   - **trait**: `trait` 宣言と `impl <Trait> for <Type>`、trait 経由の dispatch すべて未対応
   - **allocator**: `with allocator = ...`、`<A: Allocator>` bound、heap / pointer builtins (`__builtin_heap_alloc` 系) すべて未対応
   - **DbC**: `requires` / `ensures` 節は実行時チェックされ、違反時は `panic: requires violation` / `panic: ensures violation` で停止。`ensures` 内の `result` は scalar 戻り値（struct は first field）に bind。`--release` フラグで全 contract チェックを skip（`INTERPRETER_CONTRACTS=off` 相当）
   - **const**: top-level `const NAME: Type = expr` 対応。初期化式はリテラル / 既存 const 参照 / 単純な算術 fold のみ（文字列 const、関数呼び出し含む式は不可）
   - **generics**: 型パラメータを持つ関数 / struct はいずれも reject
   - **struct の制約**:
     - 関数引数 / 戻り値として struct 値は渡せる（codegen が per-field 展開）
     - struct-returning call は式位置で使えず、必ず `val x = ...` で受ける
     - struct binding 全体の再代入 (`q = p`) は不可（field 単位の代入のみ）
     - ネストした struct field とそれへの chain access (`a.b.c`) はサポート、ただし leaf scalar への代入のみ（`p.inner = Inner { ... }` 不可）
     - struct を `print` / `println` に渡せない
   - **print / println**: `i64` / `u64` / `f64` / `bool` / 文字列リテラルのみ。struct / tuple 等は不可
   - **panic / assert**: メッセージは文字列リテラル限定（const binding や concat 等は不可）
   - **その他**: 文字列ビルトインメソッド (`.len()` / `.concat()` 等)、associated function (`Foo::new()`)、メソッド呼び出し (`obj.method()`)、関数ポインタはいずれも未対応
   - **既知の挙動差**: compiler は `panic` / `print` / `println` を stdout に出力する。interpreter / JIT は `panic` を stderr に出す（libc `puts` 経由のシンプルな実装に揃えているため。出力方法を選べる仕組みは未着手）
   - **既知の挙動差**: u64 / i64 の overflow 時、interpreter は runtime panic、compiler / JIT は wrap（cranelift の wrapping arithmetic）。3 経路一致テスト (`compiler/tests/consistency.rs`) では overflow を起こさないオペランドを選んで意図しないドリフトのみを検出する設計

   **2026-05-01: print/println と struct を compiler で対応** — `compiler/runtime/toylang_rt.c` に `toy_print_*` / `toy_println_*`（i64 / u64 / bool / str 各 8 関数）を新設、driver が `cc` で同時にコンパイル＋リンク（macOS aarch64 の variadic ABI 問題を回避）。IR に `InstKind::Print { value, value_ty, newline }` と `InstKind::PrintStr { message, newline }` を追加、lower.rs で `BuiltinCall(Print/Println, args)` を引数の static type で振り分け（文字列リテラルは PrintStr 経路）。codegen で 8 つのランタイムヘルパーを `Linkage::Import` で extern 宣言、unique 文字列リテラルを `.rodata` の DataDescription に展開。struct: `lower.rs` の `Binding` を `Scalar` / `Struct { fields: Vec<FieldBinding> }` enum に拡張、struct 定義は `collect_struct_defs` で program 走査時に収集（フィールドは scalar のみ）。`Expr::StructLiteral` / `Expr::FieldAccess` / `Expr::Assign(FieldAccess, _)` を lowering で扱い、struct 値は IR レイヤを通さず field ごとに LocalId へ展開（codegen は変更なし）。e2e テスト 9 件追加（println string / numeric / bool / 改行なし print / loop 内 print / struct literal / field write / field 算術 / loop 蓄積）。**制約**: struct は関数引数/戻り値として渡せず、ネストフィールドアクセス未対応。**既知挙動差**: panic / print / println は stdout に出力。

   **2026-05-01: panic / assert を compiler で対応 + 3 経路一致テスト追加** — `Terminator::Panic { message: DefaultSymbol }` を IR に追加、`lower.rs` で `BuiltinCall(Panic, args)` と `BuiltinCall(Assert, args)` を検出して lower（assert は `Branch` + 失敗 block の `Panic`）。`codegen.rs` で各 unique panic message に `cranelift_module::DataDescription` で `.rodata` エントリ確保（"panic: <msg>\0"）、`puts` / `exit` を `Linkage::Import` で extern 宣言、Panic terminator は `puts(addr); exit(1); trap` で lower。stdout 経由（interpreter は stderr）の差分は MVP の既知の挙動差として README 記載。e2e テスト 4 件追加（panic 出力、assert 通過、assert 失敗、`if c { panic } else { v }` の式位置 panic）。`compiler/tests/consistency.rs` を新規追加: interpreter（lib API）と compiler（subprocess）で 10 件のプログラム（リテラル / 算術 / signed / fib / for-sum / while-break / elif / 短絡 / nested calls / bool 戻り値）を 3 経路一致のうち 2 経路で同 exit code を保証（JIT 一致は `interpreter/tests/jit_integration.rs` で既存検証）。

   **2026-05-01: 中間 IR レイヤを導入** — `compiler/src/ir.rs` に `Module` / `Function` / `Block` / `Instruction` / `Terminator` / `Type` / `ValueId` / `LocalId` / `BlockId` / `FuncId` を定義（`toy` プレフィックス無し）、`Display` で `--emit=ir` 用の textual format を提供。`compiler/src/lower.rs` で AST → IR の lowering pass を実装、`compiler/src/codegen.rs` を IR → Cranelift IR + `.o` 出力に再構成。値モデルは「型付き local slot + 関数ローカル SSA 値」（`val` / `var` / 引数は LocalId 経由で `LoadLocal` / `StoreLocal`、SSA 構築は Cranelift の `FunctionBuilder` に委譲）。CLI に `--emit=clif` 追加（Cranelift IR の textual dump）、既存の `--emit=ir` は新 IR を表示するように切替。e2e テスト 10 件 green（既存 9 件 + `--emit=clif` 検証 1 件）。`AllocatorBinding` の配線は次の段階で IR の `InstKind` に追加予定。

   **2026-05-01: Phase B の MVP 着手完了**。`compiler/` クレートを新設、`compiler input.t -o output` で `.o` を出して `cc` でリンク → 実行ファイルを生成する経路が動作。`--emit=ir|obj|clif|exe`（default exe）対応、CLI / lib API 両用。サポート: `i64` / `u64` / `bool` / `Unit` のみ、リテラル / 算術 / 比較 / 短絡論理 / unary / val/var / assignment / if-elif-else / while / for-range / break / continue / return / 同一プログラム内の関数呼出。未対応で次のフェーズに送り（rejected with clear error）: 文字列・struct・tuple・配列・dict・enum・trait・allocator・contracts・generics・panic/assert・print/println・heap builtins・i64↔u64 以外の cast。残り計画は元の Phase A〜E に従って継続。現状は AST → tree-walking interpreter（+ オプトイン Cranelift JIT）のみで、IR レイヤと実行可能ファイル出力経路は存在しない。`compiler/` クレートを新設し、frontend（parser / type_checker）を共有しつつ独自の codegen パイプラインを構築する。

   **Phase A: IR の新設**
   - 中間表現 `toy_ir`（仮称）の定義: SSA ベース、関数 / 基本ブロック / 命令の最小構成
   - 値表現は scalar / struct / tuple / enum を一級で扱う型付き SSA（cranelift IR にそのまま下ろせる粒度）
   - alloc site ごとに `AllocatorBinding::{Static(id), Generic(type_param), Ambient, Local(var_id)}` を付与（`ALLOCATOR_PLAN.md` Phase 3 の設計に従う）
   - AST → IR の lowering パス（型チェック後に走る、frontend と compiler の境界に置く）
   - `with` ブロックの allocator 式が compile-time 定数かを判定し、内部の `Ambient` を `Static` に置換する定数伝搬パス
   - JIT 側も将来的に同 IR を共有できるよう、IR は backend 非依存の表現に保つ（短期的には JIT は AST 直 codegen のまま、共有は後続フェーズで検討）

   **Phase B: バックエンドと実行ファイル生成**
   - バックエンド候補: **Cranelift Object**（既存 JIT との API 共有が容易・推奨）/ LLVM（最適化と互換性）/ 独自（学習目的）
   - 第一候補は `cranelift-object` で `.o` を出力 → system linker で実行ファイル
   - Windows / macOS / Linux のリンカ駆動とトリプル差異を吸収する thin layer
   - `compiler` CLI: `compiler input.t -o output` で実行ファイルを生成、`--emit=ir` / `--emit=obj` / `--emit=asm` で中間生成物も観察可能に

   **Phase C: 呼び出し規約とランタイム**
   - 案A（隠しパラメータ）: 全関数のシグネチャに `&dyn Allocator` を暗黙追加。`with` は呼び出し時に引数を差し替える
   - 案C（単相化）: `#[specialize_allocator]` 属性 / コンパイル時定数 allocator 経路は allocator 型ごとに複製
   - ランタイムを C ABI の `.o`（または静的ライブラリ）として提供: `HeapManager` / `GlobalAllocator` / `ArenaAllocator` / `FixedBufferAllocator` / `panic` helper / `process::exit` / I/O builtins (`print` / `println`) / 文字列メソッド
   - 文字列リテラルとシンボルテーブルを `.rodata` に埋め込む経路（JIT の `JIT_STRING_INTERNER` を参考に）
   - `main` 関数の数値戻り値をプロセス終了コードに（interpreter / JIT と同じセマンティクス）

   **Phase D: 機能網羅とテスト**
   - サポート対象: i64 / u64 / f64 / bool / str / ptr / 固定配列 / tuple / struct / enum + match / generic（単相化）/ Allocator system / DbC（`requires` / `ensures`）/ panic / assert
   - エンドツーエンドテスト: interpreter / JIT / コンパイラの 3 経路で同一プログラムが同一の exit code + stdout を返すことを検証する `e2e_consistency_tests`
   - 既存の `interpreter/example/*.t` を全てコンパイルして実行できることをゴールに
   - `INTERPRETER_CONTRACTS=off` 相当の release 切替を `--release` フラグで提供

   **Phase E: 最適化**
   - 定数伝搬で `Static` 結合された allocator の vtable 呼び出しを devirtualize
   - インライン化による alloc 呼び出しの完全消去（arena 等）
   - hot path で vtable オーバーヘッドゼロになることを benchmark で確認
   - generic 単相化と code size のトレードオフを measure

   実装規模の目安: Phase A〜D を MVP として通すだけでも数セッション〜週単位。`#121` の Phase 4 native codegen は本タスクの一部として吸収される。

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
- 数値型: u64 / i64 / f64（f64 リテラルは `1.5f64` / `42f64` のように `f64` サフィックス必須、タプルアクセスとの曖昧性回避）。`as` による i64/u64 ↔ f64 変換
- 固定配列: 型推論対応、インデックス型推論、境界チェック
- 配列スライス: `arr[start..end]`、`arr[..]`、負インデックス`arr[-1]`対応
- 辞書（Dict）型: `dict{key: value}`リテラル、Object型キーサポート
- 構造体: 宣言、implブロック、フィールドアクセス（read/write 両対応）、メソッド、非ジェネリック struct でも `Struct::new()` の associated function、`__getitem__`/`__setitem__`
- Trait: `trait Name { fn m(self: Self) -> T }` 宣言、`impl <Trait> for <Struct> { ... }` 実装、`<T: SomeTrait>` bound、conformance チェック（型不一致・欠落メソッド検出）
- 文字列: ConstString/String二重システム、`str.len()`、`.concat()`、`.trim()`、`.to_upper()`、`.to_lower()`、`.split()`、`.substring()`、`.contains()`
- コメント: `#`（行）、`/* */`（ブロック）
- Allocator システム: `with allocator = expr { ... }`、`ambient` キーワード、`<A: Allocator>` bound、自動 ambient 挿入、Arena / FixedBuffer allocator
- Enum + match（Phase 1/2）: unit + tuple variant、`Enum::Variant` / `Enum::Variant(args)`、`match` arm は unit・tuple パターン（バインディング/`_` discard）+ ワイルドカード `_`

### 型システム
- 自動型変換・型推論（数値リテラルのサフィックス省略可）
- ジェネリック関数: `fn identity<T>(x: T) -> T`（パース→型推論→実行）
- ジェネリック構造体: `struct Container<T>`、constraint-based型推論
- ネストジェネリック: `Container<Container<T>>`（C++11スタイル`>>`分割）
- Self キーワード: implブロック内での構造体参照
- Trait bound: `<A: Allocator>` および `<T: UserTrait>` を関数・struct・impl に付与、呼び出し側で検証、bound 連鎖

### モジュール・その他
- Go-styleモジュールシステム: package/import/qualified name resolution
- 統合インデックスシステム: 配列・辞書・構造体で統一`x[key]`構文

### テスト状況
- 合計 894 テスト（100% 成功率、2026-04-22 時点）

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
