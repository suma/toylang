# TODO - Interpreter Improvements

## 完了済み ✅

1. **エラーメッセージの一貫性を修正** - 各演算関数で正しい関数名を使用
2. **Enumベースのアプローチで算術演算のコード重複を解消**
3. **object.rsのパニック呼び出しをResult型に変更してエラーハンドリングを改善**
4. **unwrap_*メソッドをパニック版とtry_unwrap_*版に分離してAPIを改善**
5. **for文のUInt64とInt64処理の重複をジェネリクスで統一してパフォーマンスを改善**
6. **while文の未実装部分を完成して言語機能を完全にする**
7. **比較演算のコード重複をEnumベースアプローチで解消して保守性を向上**
8. **エラーハンドリングの明確化** ✅ (2024-06-24完了)
   - `evaluation.rs`のObject::NullをObject::Unitに変更してセマンティクスを明確化
   - while/forループ完了時、EvaluationResult::None、Return(None)処理を改善
9. **論理演算の最適化** ✅ (2024-06-24完了)
   - `evaluate_logical_and_short_circuit`と`evaluate_logical_or_short_circuit`で短絡評価を実装
   - false && expr、true || exprで右辺を評価せずにパフォーマンス向上を実現
   - 4つのテストケースを追加して動作確認済み
10. **未使用importの削除** ✅ (2024-06-24完了)
    - 警告に表示されている`RcObject`と`convert_object`の未使用importを削除
    - コンパイラ警告を解消してコードベースをクリーンアップ
11. **型チェックの改善 - AST変換機能** ✅ (2024-06-26完了)
    - frontendからinterpreterに渡すASTの段階で型変換を完了
    - `Expr::Number`を`Expr::UInt64`/`Expr::Int64`に自動変換する機能を実装
    - `TypeDecl::Number`を追加して型推論を改善
    - ExprPool/StmtPoolに`get_mut`メソッドを追加してAST変換を可能に
    - const変数（val）の型更新時も`is_const`フラグを保持
    - 自動型変換テストケースを追加

11b. **コンテキストベース型推論の改善** ✅ (2024-06-26完了)
    - 関数内で最初に見つかった明示的型宣言をグローバル型ヒントとして使用
    - `val b: i64 = 50`のような明示的宣言が`val a = 100`などの推論に影響
    - 複数操作における一貫した型推論：`(a - b) + (c - d)`で全てInt64
    - visit_valでUnknown型宣言もコンテキスト型推論を適用
    - 包括的テストスイート（41個のテストケースが全て通過）
    - 追加テスト：単一明示的Int64宣言による他変数への影響を検証

12. **固定配列（Fixed Arrays）の実装** ✅ (2024-06-30完了)
    - 配列型宣言：`val a: [i64; 5] = [1i64, 2i64, 3i64, 4i64, 5i64]`
    - 配列リテラル：`[1i64, 2i64, 3i64]`（末尾カンマと改行サポート）
    - 配列アクセス：`a[0u64]`（読み取り・書き込み両対応）
    - 配列要素代入：`a[0u64] = 10i64`
    - AST拡張：`TypeDecl::Array`、`Expr::ArrayLiteral`、`Expr::ArrayAccess`
    - 型チェッカー：配列要素型の統一チェック、境界チェック
    - インタープリター：`Object::Array`、実行時配列操作
    - 包括的テストスイート：14個の単体テスト + 3個のプロパティベーステスト
    - エラーケース：型不一致、境界外アクセス、負インデックス
    - 実用例：フィボナッチ数列計算（example/fibonacci_array.t）

13. **行コメント機能の実装** ✅ (2024-07-05完了)

14. **型チェッカーの状態管理リファクタリング** ✅ (2024-07-21完了)
    - 1636行の巨大ファイルをモジュール分割してメンテナンス性を大幅改善
    - 5つの責任を明確に分離：CoreReferences, TypeCheckContext, Error, Function, Inference, Optimization
    - 186箇所の相互依存を構造化されたアクセスに改善
    - 4つのライフタイム管理を簡素化
    - モジュール構成：
      - `type_checker/core.rs` - AST pools and utilities
      - `type_checker/context.rs` - Variable and function context
      - `type_checker/error.rs` - Error types and handling
      - `type_checker/function.rs` - Function checking state
      - `type_checker/inference.rs` - Type inference management  
      - `type_checker/optimization.rs` - Performance caching
    - 104個の全テストが通過、完全な後方互換性を維持
    - パニック処理をResult型エラーハンドリングに置き換え
    - `#` 記号による行コメント：`# これはコメント`
    - インラインコメント：`val x = 10u64  # 変数定義`
    - Token enumに `Comment(String)` バリアント追加（linter互換性）
    - lexerに `"#".*` パターン追加でコメント内容をキャプチャ
    - パーサーで構文解析中にコメントトークンを自動スキップ
    - 包括的テストスイート：lexer・パーサー両方のテスト追加
    - 使用例：example/comment_test.t でコメント機能のデモ

15. **コンパイラ警告の未使用import除去** ✅ (2024-07-21完了)
    - dead_code警告を完全解消してコードベースをクリーンアップ
    - 未使用ヘルパーメソッドを削除：type_mismatch_with_location, not_found_with_location, type_mismatch_operation_with_location
    - LookaheadBufferの未使用min_sizeフィールドを実際に活用
    - cleanup処理でバッファの最小サイズ維持機能を実装
    - メモリ効率とパフォーマンスの最適化を両立
    - 104個の全テストが通過、機能への影響なし

14. **配列要素の型推論機能の実装** ✅ (2024-07-05完了)
    - 配列のelement_typeから自動的に要素の型を推論する機能を完全実装
    - 例：`val a: [i64; 3] = [1, 2, 3]` で `1, 2, 3` を自動的に `i64` 型と推論
    - 例：`val a: [u64; 3] = [1, 2, 3]` で `1, 2, 3` を自動的に `u64` 型と推論
    - 負数対応：`val a: [i64; 3] = [-1, -2, -3]` も正常動作
    - 型ヒント伝播システムの改善：配列リテラル内で要素型を適切に推論
    - AST変換処理：Number型から具体的型（Int64/UInt64）への自動変換
    - 包括的テストスイート：5つの配列型推論テストケース追加
    - 詳細なエラーメッセージ：型不一致時により具体的なエラー表示
    - 全68個のテストが成功（既存機能の完全な互換性維持）

15. **配列インデックスの型推論機能の実装** ✅ (2024-07-05完了)
    - 配列アクセス時のインデックス型推論を完全実装：`a[0]` で `0` を適切な整数型に自動推論
    - 数値リテラルインデックス：`a[0]` → UInt64型に自動変換
    - 変数インデックス：`a[i]` → 変数 `i` をUInt64型に自動推論
    - 式インデックス：`a[base + 1]` → 式全体をUInt64型に自動推論
    - AST変換処理：インデックス用のNumber型からUInt64への自動変換
    - 型ヒント伝播システム：配列アクセス時にインデックスへUInt64型ヒントを設定
    - 包括的テストスイート：5つのインデックス型推論テストケース追加
    - 実用例：example/index_inference_test.t, index_variable_test.t, index_expression_test.t
    - 全73個のテストが成功（既存機能との完全な互換性維持）

16. **テストカバレッジの向上** ✅ (2024-07-06完了)
    - 境界値テストを大幅に追加：整数オーバーフロー/アンダーフロー、ゼロ除算、配列境界
    - エラーハンドリングテストを強化：未定義変数・関数、型不一致の詳細検証
    - プロパティベーステストを拡張：算術結合法則、型推論一貫性、ループ境界条件
    - 深い再帰テスト、型変換境界テスト、配列混合型操作テストを追加
    - InterpreterErrorにDisplayトレイト実装でエラーメッセージ表示を改善
    - 20個の新しいテストケース追加（境界値12個、エラーハンドリング5個、プロパティ5個）
    - 全93個のテストが成功（元の73個 + 新規20個）
    - 無意味なテストケースを除去してテスト品質を向上
    - 品質保証の大幅な強化：パニック処理、型チェック時エラー、実行時エラーの包括的カバレッジ

17. **構造体（struct）機能の実装** ✅ (2024-07-06完了)
    - 構造体宣言：`struct Point { x: i64, y: i64 }`
    - 構造体リテラル：`Point { x: 10, y: 20 }`
    - フィールドアクセス：`obj.field`（ドット記法）
    - implブロック：`impl Point { fn new() -> Point { ... } }`
    - メソッド呼び出し：`obj.method(args)`（ドット記法）
    - &selfパラメータ：`fn distance(&self) -> i64`
    - AST拡張：`StructDecl`、`ImplBlock`、`FieldAccess`、`MethodCall`、`StructLiteral`
    - レキサー拡張：`impl`キーワード、`&`演算子のサポート
    - 型チェッカー：構造体関連の型検証、メソッド定義の妥当性チェック
    - インタープリター：メソッドレジストリ、構造体オブジェクトの実行時処理
    - 包括的テストスイート：構造体宣言、implブロック、複数メソッドの定義テスト
    - 15個のサンプルファイル追加：配列・構造体操作の実用例

18. **大きな関数の分割リファクタリング** ✅ (2024-07-06完了)
    - TypeCheckerVisitor::visit_val関数の分割（91行→4つの小さな関数）
      - `setup_type_hint_for_val()` - 型ヒント設定（19行）
      - `update_variable_expr_mapping()` - 変数-式マッピング管理（25行）
      - `apply_type_transformations()` - 型変換適用（28行）
      - `determine_final_type()` - 最終型決定（7行）
    - evaluate関数の分割（218行→8つの小さな関数）
      - `evaluate_literal()` - リテラル値評価（3行）
      - `evaluate_if_elif_else()` - if-elif-else制御構造（54行）
      - `evaluate_function_call()` - 関数呼び出し（24行）
      - `evaluate_array_literal()` - 配列リテラル（8行）
      - `evaluate_array_access()` - 配列アクセス（32行）
      - `evaluate_field_access()` - フィールドアクセス（17行）
      - `evaluate_method_call()` - メソッド呼び出し（32行）
      - `evaluate_struct_literal()` - 構造体リテラル（18行）
    - evaluate_block関数の分割（246行→11つの小さな関数）
      - `handle_val_declaration()` - val変数宣言処理（5行）
      - `handle_var_declaration()` - var変数宣言処理（12行）
      - `handle_return_statement()` - return文処理（11行）
      - `handle_while_loop()` - whileループ実行（28行）
      - `handle_for_loop()` - forループ実行（40行）
      - `handle_expression_statement()` - 式文処理（20行）
      - `handle_assignment()` - 代入式処理（8行）
      - `handle_variable_assignment()` - 変数代入処理（25行）
      - `handle_array_element_assignment()` - 配列要素代入処理（44行）
      - `handle_identifier_expression()` - 識別子式処理（8行）
      - `handle_nested_block()` - ネストブロック処理（5行）
    - 可読性・保守性・テスト容易性の大幅改善
    - 全99個のテストが引き続き成功（機能の完全な互換性維持）

19. **str.len()メソッドの実装** ✅ (2024-07-07完了)
    - str型組み込みメソッド：`"string".len()` → u64型で文字列長を返す
    - 型チェッカー拡張：str型（TypeDecl::String）メソッド呼び出しの型推論（引数なし、u64返却型）
    - インタープリター拡張：string_internerからの文字列取得と長さ計算
    - 既存のMethodCall構文を活用（構造体メソッドとの統一的処理）
    - 包括的テストスイート：5個の単体テスト（基本・空文字列・算術・比較・式内使用）
    - 実用例：example/string_len_test.t、example/string_len_comprehensive_test.t
    - エラーハンドリング：引数チェック、未知メソッドエラー
    - 全104個のテストが成功（既存99個 + 新規5個のstr.len()テスト）

20. **パフォーマンス測定** ✅ (2024-07-09完了)
    - Criterionによる詳細ベンチマーク：fibonacci_recursive (55.6µs), for_loop_sum (230.6µs), type_inference_heavy (9.8µs)
    - テスト実行時間測定：104テスト全体で4.5秒（コンパイル3.1秒、実行0.61秒）
    - サンプルプログラム実行時間：フィボナッチ1.6秒、その他0.4秒（コンパイル含む）
    - フロントエンドビルド時間：デバッグ・リリース共に9.5秒
    - メモリ使用量測定：実行時1.4-1.5MB、テスト時34.9MB最大フットプリント
    - 軽量で高速な実装を確認、マイクロ秒単位での高いパフォーマンス実現

21. **Object型のメモリレイアウト最適化** ✅ (2024-07-19完了)
    - 大きなバリアント（Array、Struct）をBox化してenumサイズを削減
    - `Array(Vec<RcObject>)` → `Array(Box<Vec<RcObject>>)`
    - `Struct { fields: HashMap<...> }` → `Struct { fields: Box<HashMap<...>> }`
    - メモリレイアウト分析ツールを追加（size_analysis.rs）
    - 全104個のテストが通過、機能の完全な互換性維持
    - メモリ効率性の向上によりパフォーマンスが期待される改善

22. **ObjectError型の拡張** ✅ (2024-07-19完了)
    - より詳細なエラー情報を提供する新しいエラーバリアントを追加
    - `FieldNotFound { struct_type, field_name }` - 構造体フィールドの詳細エラー
    - `IndexOutOfBounds { index, length }` - 配列境界外アクセスの詳細情報
    - `NullDereference` - Null参照エラー
    - `InvalidOperation { operation, object_type }` - 無効な操作の詳細エラー
    - 新しい安全なメソッド追加：`get_array_element()`, `set_array_element()`, `check_not_null()`
    - 既存のエラーハンドリングコードを新しいバリアントに更新
    - 全104個のテストが通過、機能の完全な互換性維持

23. **型推論キャッシュ機能の実装** ✅ (2024-07-20完了)
    - TypeCheckerVisitorにtype_cacheフィールドを追加してHashMap<ExprRef, TypeDecl>で型推論結果をキャッシュ
    - get_cached_type()、cache_type()ヘルパーメソッドを実装してキャッシュの読み書きを管理
    - visit_exprメソッドでキャッシュを活用し、同じ式の型推論重複実行を回避
    - ExprRefにHashとEqトレイトを追加してHashMapのキーとして使用可能にする
    - 型推論のパフォーマンス向上により大きな式や複雑な型推論処理が高速化
    - frontend・interpreterの全テスト（78個・104個）が通過、機能の完全な互換性維持

24. **型推論キャッシュのスコープ最適化** ✅ (2024-07-20完了)
    - type_check関数で各関数開始時にキャッシュをクリアしてスコープを関数内に限定
    - visit_block関数で各ブロック開始時にキャッシュをクリアしてスコープをブロック内に限定
    - スコープを超えたキャッシュ蓄積を防止してメモリ使用量を削減
    - type_inference_heavyで2.9%、variable_scopesで2.5%のパフォーマンス改善を実現
    - キャッシュオーバーヘッドを最小限に抑えながら局所的な型推論最適化を効果的に実行
    - 全78個のfrontend・104個のinterpreterテストが引き続き通過

25. **execute_program関数のリファクタリング** ✅ (2024-07-20完了)
    - execute_program関数（47行）を4つの専門的な関数に分割
      - `find_main_function()` - main関数の検索とエラーハンドリング（10行）
      - `build_function_map()` - 関数マップの構築（6行）
      - `build_method_registry()` - メソッドレジストリの構築（16行）
      - `register_methods()` - EvaluationContextへのメソッド登録（7行）
    - execute_program本体は17行に簡潔化、エラーハンドリングが早期リターンで改善
    - 単一責任の原則に従い、各関数が明確な役割を持つ設計に変更
    - 可読性・保守性・テスト容易性の大幅改善
    - 全104個のテストが引き続き成功（機能の完全な互換性維持）

26. **lexer.lファイルのリファクタリング** ✅ (2024-07-20完了)
    - `parse_number`マクロを追加して数値パース処理を統一化
    - キーワード、演算子、リテラルを論理的にグループ分けして整理
    - 複数文字演算子を単一文字版より前に配置してマッチング順序を最適化
    - 一貫したフォーマットとインデントで可読性を向上
    - 文字列とコメントの処理にブロック構文を使用して処理を明確化
    - 全78個のfrontendテストが引き続き成功（機能の完全な互換性維持）

27. **TypeCheckerVisitorの状態管理リファクタリング** ✅ (2024-07-20完了)
    - 複雑な状態管理を機能別に4つのグループ構造体に分割
      - `CoreReferences` - AST構造への参照（stmt_pool, expr_pool, string_interner）
      - `TypeInferenceState` - 型推論状態（type_hint, number_usage_context, variable_expr_mapping）
      - `FunctionCheckingState` - 関数チェック状態（call_depth, is_checked_fn）
      - `PerformanceOptimization` - パフォーマンス最適化（type_cache）
    - 各グループ構造体に初期化関数（new()）を追加
    - 130箇所以上のフィールドアクセスを新しいグループ化構造に更新
    - 関連する状態の論理的なグループ化により可読性と保守性を大幅改善
    - 全78個のfrontend・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

28. **エラーメッセージシステムの統一化** ✅ (2024-07-20完了)
    - 手動文字列作成と一貫性のないエラーメッセージ問題を解決
    - 構造化エラーシステム（TypeCheckErrorKind enum）を導入
      - `TypeMismatch`, `TypeMismatchOperation`, `NotFound`, `UnsupportedOperation`
      - `ConversionError`, `ArrayError`, `MethodError`, `InvalidLiteral`, `GenericError`
    - 専用コンストラクタメソッドを提供（type_mismatch, not_found, conversion_error等）
    - エラーメッセージの統一フォーマットとコンテキスト情報追加機能を実装
    - 全てのformat!による手動エラーメッセージを構造化システムに置き換え
    - 型安全性とエラーメッセージの一貫性を大幅改善
    - 全78個のfrontend・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

29. **パーサーのBuilderパターンリファクタリング** ✅ (2024-07-20完了)
    - 従来のチェーンメソッド型BuilderパターンからミュータブルAPIスタイルに変更
    - 新しいASTBuilder実装：`&mut self`を使った直接的で簡潔なAPI
      - `binary_expr(&mut self, op: Operator, lhs: ExprRef, rhs: ExprRef) -> ExprRef`
      - `call_expr(&mut self, fn_name: DefaultSymbol, args: Vec<ExprRef>) -> ExprRef`
      - `var_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: Option<ExprRef>) -> StmtRef`
    - 古いExprBuilder・StmtBuilderクラスを削除してコードベースを簡潔化
    - パーサーコード全体を新しいAPIに移行：parse_primary、parse_block、parse_stmt等すべて更新
    - メモリプールベースの効率的な実装を維持しながら可読性を大幅改善
    - 全78個のfrontend・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

30. **パーサーの返り値型統一とパフォーマンス改善** ✅ (2024-07-20完了)
    - 混在していたbool返り値をResult<()>型に統一してエラーハンドリングを一貫化
    - `expect()`メソッドをResult<()>返り値に変更、一貫性のあるエラーメッセージ提供
    - インデックスベースのトークン管理システムを導入してパフォーマンスを改善
      - `ahead_pos`フィールド追加でO(n)のremove(0)操作をO(1)のインデックスアクセスに変更
      - `ensure_token_available()`ヘルパーメソッドで効率的なlookahead管理
      - 定期的なメモリクリーンアップでメモリ使用量を最適化
    - `expect_err()`メソッドに非推奨警告を追加してAPIの統一を促進
    - `peek_position_n()`と`next()`メソッドを新しいインデックスベースアプローチに更新
    - 全78個のfrontend・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

31. **Parserのモジュール化リファクタリング** ✅ (2024-07-20完了)
    - 1700行の巨大なParser実装を4つの専門モジュールに分割
      - `core.rs` - コアParser構造体とトークン管理（299行）
      - `expr.rs` - 式解析関数（416行）
      - `stmt.rs` - 文解析とstruct/impl処理（295行）
      - `tests.rs` - 全パーサーテストとlexerテスト（693行）
    - 単一責任の原則に従い、各モジュールが明確な役割を持つ設計に変更
    - メソッド数を大幅削減し、Parser構造体を簡潔化
    - `frontend/src/lib.rs`を8行に簡潔化（モジュール宣言のみ）
    - 全88個のfrontendテスト・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

32. **LookaheadBufferの最適化と分離** ✅ (2024-07-20完了)
    - Parser内のlookahead bufferを独立したモジュールに分離
    - リングバッファ最適化による高効率なトークン管理システム実装
      - `LookaheadBuffer`構造体 - VecDequeベースの効率的なトークン保存
      - 動的クリーンアップ機能 - メモリ使用量の自動最適化
      - 統計情報機能 - デバッグとパフォーマンス監視
    - TokenSourceトレイト抽象化によるトークンソースの柔軟性向上
    - TokenProviderによる統合管理とコメント自動フィルタリング
    - 包括的テストスイート - LookaheadBufferとTokenProviderの独立テスト
    - 全88個のfrontendテスト・104個のinterpreterテストが引き続き成功（機能の完全な互換性維持）

33. **TypeCheckErrorにソースコード位置情報を追加** ✅ (2024-07-21完了)
    - `SourceLocation`構造体を実装：`line: u32`, `column: u32`, `offset: u32`フィールド
    - `TypeCheckError`構造体に`location: Option<SourceLocation>`フィールドを追加
    - `with_location()`メソッドでエラーに位置情報を付与可能
    - エラーメッセージ表示時に位置情報を含む形式：`3:18:45: Type mismatch...`
    - パーサーに位置情報取得機能を追加：`current_source_location()`メソッド
    - `offset_to_line_col()`メソッドで絶対オフセットから行列番号を計算
    - `Node`構造体に`to_source_location()`ヘルパーメソッドを追加
    - AST構造と位置情報の連携機能を実装
    - デモプログラムで位置情報機能の動作確認を完了
    - エラー発生箇所の正確な特定により、デバッグ効率が大幅に向上

34. **型チェック実行時のSourceLocation表示機能を実装** ✅ (2024-07-21完了)
    - `check_typing`関数にソースコード参照パラメータを追加
    - `calculate_line_col_from_offset`関数で位置情報計算機能を実装
    - TypeCheckErrorの`location`フィールドを公開してアクセス可能に変更
    - `main.rs`と`lib.rs`でソースコード連携を実装
    - 型チェックエラー時に適切な位置情報付きメッセージを表示
    - 未定義関数エラーでの動作確認：`Function 'unknown_function' not found`
    - 位置情報追跡インフラストラクチャの基盤を完成
    - セミコロンなし構文仕様をCLAUDE.mdに明記
    - エラーメッセージの品質向上によりデバッグ効率が改善

35. **ASTノードの包括的な位置情報統合** ✅ (2024-07-21完了)
    - LocationPoolを実装してExprとStmtの位置情報を効率的に管理
    - 全AstBuilderメソッドにlocationパラメータを追加してBuilderパターンを維持
    - パーサーで全AST生成時に位置情報を取得・設定する機能を実装
    - TypeCheckerVisitorを拡張してLocationPoolアクセス機能を追加
    - visit_exprとvisit_stmtメソッドで自動的に位置情報をエラーに付与
    - エラーメッセージ形式：`6:29:78: Function 'unknown_function' not found`
    - 行:列:オフセット形式での正確な位置情報表示を実現
    - デバッグ効率の大幅な向上：エラー発生箇所の瞬時特定が可能
    - ベンチマークテストの修正完了：Criterionベンチマークが全て正常実行
    - `check_typing`関数の新しいsource_codeパラメータに対応済み
    - 未定義`source`変数を`complex_program`に修正してコンパイルエラー解消
    - 既存機能の完全な互換性維持：全テストが引き続き通過

36. **unwrapの完全削除によるエラーハンドリング強化** ✅ (2024-07-21完了)
    - プロジェクト全体から安全でないunwrap()を完全に除去
    - frontendディレクトリの改善：
      - `type_checker.rs`の60箇所以上のunwrap → `ok_or_else()`による適切なエラーハンドリング
      - `type_checker/context.rs`の変数スタック操作 → `expect()`による明示的エラーメッセージ
      - `build.rs`の環境変数 → `expect()`による明確なエラー表示
      - パーサーテストの引数不整合 → LocationPool引数追加で修正
    - interpreterディレクトリの改善：
      - `main.rs`の3箇所のunwrap → `match`文による適切な分岐処理
      - `evaluation.rs`の25箇所以上のunwrap → InterpreterErrorによる構造化エラー
      - `lib.rs`のstring_interner → `ok_or_else()`によるエラーハンドリング
      - `environment.rs`の変数スタック操作 → `if let`パターンによる安全な処理
    - 借用チェックエラーの修正：ブロックスコープによる借用期間制御
    - 全テストが引き続き通過：frontend 92テスト、interpreter 104テスト
    - サンプルプログラム（fib.t）の正常動作確認完了
    - エラーハンドリングの堅牢性向上により、パニック発生リスクを大幅削減

37. **Bool配列型推論の実装** ✅ (2024-07-27完了)
    - Bool型配列の型推論機能を完全実装：`[true, false, x > y]` 形式の自動型推論対応
    - TypeCheckerVisitorのvisit_array_literalメソッドを拡張してBool型サポートを追加
    - AST変換処理：Bool型配列要素の一貫性チェックと型統一機能
    - 包括的単体テストスイート：基本テスト、エラーケース、パフォーマンステスト、互換性テスト
    - 統合テスト：複雑な条件式を含むBool配列の実用的テストケース
    - 配列パーサーの改行処理修正：改行トークンの適切な処理によりマルチライン配列リテラル対応

38. **Struct配列型推論の実装** ✅ (2024-07-27完了)
    - Struct型配列の型推論機能を実装：構造体配列リテラルの型推論対応
    - TypeCheckContextに構造体定義レジストリを追加して構造体型管理を強化
    - visit_struct_literalとvisit_array_literalメソッドの大幅拡張
    - 構造体配列の型統一チェックと異なる構造体型混在エラーの適切な検出
    - 包括的単体テストスイート：基本テスト、エラーケース、パフォーマンステスト、ネストテスト
    - 統合テスト準備：将来のパーサー実装に備えた基本・ネスト構造体配列テスト
    - 構造体パーサー未実装の課題を明確化（将来の実装課題として文書化）

39. **構造体メソッド呼び出しの型推論を修正** ✅ (2024-07-27完了)
    - 構造体メソッド呼び出しでUnknown型を返していた問題を修正
    - TypeCheckContextにstruct_methodsフィールドを追加してメソッド管理機能を実装
    - register_struct_method()とget_struct_method()メソッドを追加
    - visit_impl_blockでメソッドをcontextに登録するロジックを実装
    - visit_method_callでメソッドの実際の戻り値型を返すよう修正
    - check_typing関数でimplブロックを事前処理してメソッド登録順序を調整
    - frontend/src/visitor.rsモジュールをpublicに変更してAstVisitorトレイトへのアクセスを可能に
    - 失敗していたtest_struct_method_call_with_literalとtest_struct_method_call_with_argsテストが通過
    - 全117個のテストが成功、構造体メソッドの型推論が正常に動作

40. **TypeCheckerVisitorのライフタイム引数統合** ✅ (2024-08-01完了)
    - `TypeCheckerVisitor<'a, 'b, 'c, 'd>`を`TypeCheckerVisitor<'a, 'b>`に簡素化
    - `CoreReferences<'a, 'b, 'c, 'd>`を`CoreReferences<'a, 'b>`に統合
    - 不変参照のライフタイム（'a、'c、'd）を単一ライフタイム'aに統合
    - 可変参照'bは独立して保持し、借用チェッカーの要件を満足
    - コードの可読性と保守性が向上、型シグネチャの複雑さが軽減
    - frontend 107テスト・interpreter 117テスト全て成功、機能の完全な互換性維持

41. **TypeCheckerVisitorの機能別トレイト分割** ✅ (2024-08-01完了)
    - 単一の巨大構造体を6つの機能別トレイトに分割してアーキテクチャを改善
    - トレイト設計：
      - `LiteralTypeChecker` - リテラル値の型チェック（数値、文字列、真偽値、null）
      - `ExpressionTypeChecker` - 式の型チェック（バイナリ演算、配列アクセス、代入等）
      - `StatementTypeChecker` - 文の型チェック（var/val宣言、制御構造、return等）
      - `StructTypeChecker` - 構造体関連の型チェック（宣言、フィールドアクセス、メソッド）
      - `FunctionTypeChecker` - 関数関連の型チェック（関数呼び出し、型推論）
      - `TypeInferenceManager` - 型推論とキャッシュ管理
      - `TypeCheckerCore<'a, 'b>` - 共通機能とアクセサメソッド
    - 既存コードの完全移行：`visit_*_literal`メソッドを`check_*_literal`トレイトメソッドに変更
    - ライフタイム修正とコンパイルエラー解決：ジェネリックライフタイムパラメータの適切な管理
    - 単一責任の原則の実現：各トレイトが明確な機能範囲を持つ設計
    - 拡張可能性の向上：新機能追加時のトレイト実装による柔軟な拡張
    - frontend 107テスト・interpreter 117テスト全て成功、機能の完全な互換性維持

42. **TypeCheckerVisitorのvisit_*メソッドカテゴリ別グループ化** ✅ (2024-08-01完了)
    - 1000行を超えるTypeCheckerVisitorの`visit_*`メソッドを7つの機能カテゴリに整理
    - カテゴリ別セクション区切り：
      - `Core Visitor Methods` - 基本的なVisitorインターフェース（visit_expr, visit_stmt）
      - `Expression Type Checking` - 式の型チェック（visit_binary, visit_assign, visit_identifier）
      - `Function and Method Type Checking` - 関数・メソッドの型チェック（visit_call, visit_method_call）
      - `Literal Type Checking` - リテラル値の型チェック（visit_*_literal メソッド群）
      - `Array and Collection Type Checking` - 配列・コレクションの型チェック（visit_array_*, visit_expr_list）
      - `Statement Type Checking` - 文の型チェック（visit_var, visit_val, visit_return, visit_expression_stmt）
      - `Control Flow Type Checking` - 制御構造の型チェック（visit_for, visit_while, visit_break, visit_continue）
      - `Struct Type Checking` - 構造体関連の型チェック（構造体宣言、フィールドアクセス、メソッド）
    - 明確なセクション区切りコメントによる論理的構造の可視化
    - 関連メソッドの物理的グループ化により保守性と可読性を大幅改善
    - 機能追加時の適切なセクション配置による開発効率向上
    - frontend 107テスト・interpreter 117テスト全て成功、機能の完全な互換性維持

43. **同一ファイル内での構造体名の相互参照サポート** ✅ (2024-08-03完了)
    - 既存の二パスシステムがstruct名の相互参照を完全サポートしていることを確認
    - テストケース作成と検証：
      - 基本的なstruct相互参照（struct_cross_ref_test.t）
      - 構造体間の循環参照（mutual_struct_test.t）
      - フォワード参照（前方参照）
    - 検証結果：
      - `struct Node { next: LinkedList }` と `struct LinkedList { head: Node }` の循環参照が正常動作
      - 後で定義されるstructを先に参照するフォワード参照も正常動作
      - 既存の関数向け二パスシステムがstructにも適用済み
    - 全テスト成功：frontend 107テスト・interpreter 120テスト（lib: 3、main: 117、doc: 0）
    - 追加実装は不要：既存アーキテクチャで完全な機能サポートを確認

44. **frontendでの複数エラー収集・返却機能** ✅ (2024-08-09完了)
    - パーサーでの複数構文エラー収集：`MultipleParserResult<T>` 型を実装
    - 型チェッカーでの複数型エラー収集：`MultipleTypeCheckResult<T>` 型を実装
    - Parser::expect_errと他のエラー処理を統一：
      - `collect_error()` メソッドでエラー収集を統一化
      - `expect_or_collect()` メソッドで条件チェックとエラー収集を統合
      - `anyhow!`エラーを全て`collect_error`に置き換えて継続解析を実現
    - 統合されたエラー収集ポイント：
      - expect_err: 期待トークン不一致エラー
      - parse_program: 関数/構造体/impl宣言の構文エラー
      - parse_expr_impl: 式解析でのEOF・予期しないトークンエラー
      - parse_block_impl: ブロック内ステートメント解析エラー
      - その他：フィールドアクセス、配列要素、構造体リテラル等のエラー
    - 包括的テストスイート：6個のテストケース
      - 複数パーサーエラー・複数型エラー・成功ケース・混合エラーのテスト
      - 統合エラー収集・expect_err統合のテスト
    - APIエクスポート：`MultipleParserResult`、`MultipleTypeCheckResult`をlib.rsで公開
    - 全テスト成功：frontend 6個の複数エラーテスト + 既存テスト全通過
    - エラー発生時も解析を継続し、一度に複数のエラーを報告して開発効率を大幅改善


## 進行中 🚧

現在進行中のタスクはありません。

## 未実装 📋

23. **ドキュメント整備** 📚
    - 言語仕様やAPIドキュメントの整備

24. **str型と基本文字列操作** 📝
    - str型の実装（concat, substring, contains等）
    - len()メソッドは実装済み ✅
    - 基本的な文字列操作関数群

25. **動的配列（List型）** 📋
    - 可変長配列の実装
    - push, pop, get等の基本操作
    - 固定配列からの移行パス

26. **Option型によるNull安全性** 🛡️
    - Option<T>型の実装
    - パターンマッチングの基礎

27. **組み込み関数システム** 🔧
    - builtin.rsモジュールの作成
    - 関数呼び出し時の組み込み関数検索
    - 型変換・数学関数の実装

## 検討中の機能

* 組み込み関数の定義
* FFIあるいは他の方法による拡張ライブラリ実装方法の提供
* 動的配列
* パターンマッチング
* 列挙型（Enum）
* 文字列操作
* 数値型のbitwise operation
* ラムダ式・クロージャ
* Option型（Null安全性）
* モジュール・名前空間
* 言語組み込みのテスト機能、フレームワーク
* 言語内からASTの取得、操作

## メモ

- 算術演算と比較演算は既にEnum化により統一済み
- 基本的な言語機能（if/else、for、while）は完全実装済み
- AST変換による型安全性が大幅に向上（frontendで型変換完了）
- 自動型変換機能により、型指定なしリテラルの使い勝手が向上
- **コンテキストベース型推論が完全実装済み** - 関数内の明示的型宣言が他の変数の型推論に影響
- 複雑な複数操作での一貫した型推論：`(a - b) + (c - d)`で全要素が統一型
- **全93個のテストが通過している状態を維持**（プロパティベーステスト含む）
- 追加された単一明示型宣言テストにより、コンテキスト推論のカバレッジがさらに向上
- **固定配列機能が完全実装済み** - 14個の単体テスト + 3個のプロパティベーステストで品質保証
- 配列の基本構文サポート：`val a: [i64; 5] = [1i64, 2i64, 3i64, 4i64, 5i64]`、`a[0u64] = 10i64`
- **行コメント機能が完全実装済み** - `#` 記号による行コメントとインラインコメント対応
- linter互換性のためコメント内容をTokenに保存、パーサーで自動スキップ
- **配列要素の型推論機能が完全実装済み** - `val a: [i64; 3] = [1, 2, 3]` 形式の自動型推論対応
- 型ヒント伝播システムとAST変換処理により、配列リテラル内の数値型が適切に推論・変換
- **配列インデックスの型推論機能が完全実装済み** - `a[0]`、`a[i]`、`a[base + 1]` 形式の自動型推論対応
- 配列操作の使いやすさが大幅に向上、明示的型指定と自動推論の両方をサポート
- **構造体機能が完全実装済み** - 構造体宣言、implブロック、フィールドアクセス、メソッド呼び出し対応
- ドット記法による直感的な構造体操作：`obj.field`、`obj.method(args)`、`Point { x: 10, y: 20 }`
- **大きな関数の分割リファクタリング完了** - visit_val関数（91行→4関数）、evaluate関数（218行→8関数）、evaluate_block関数（246行→11関数）
- コードの可読性・保守性・テスト容易性が大幅に向上、全99個のテストが引き続き成功
- **str.len()メソッドが完全実装済み** - `"string".len()` 形式でu64型の文字列長を取得可能
- str型の組み込みメソッドシステムを確立、構造体メソッドと統一的に処理