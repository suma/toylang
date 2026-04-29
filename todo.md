# TODO - Interpreter Improvements

## 完了済み ✅

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
- Trait bound: `<A: Allocator>` を関数・struct・impl に付与、呼び出し側で検証、bound 連鎖

### モジュール・その他
- Go-styleモジュールシステム: package/import/qualified name resolution
- 統合インデックスシステム: 配列・辞書・構造体で統一`x[key]`構文

### テスト状況
- 合計 894 テスト（100% 成功率、2026-04-22 時点）

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
