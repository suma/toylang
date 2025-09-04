# TODO - Interpreter Improvements

## 完了済み ✅

100. **Structure of Arrays (multiarraylist) パターンへの変換と数値変換機能の復旧** ✅ (2025-09-03完了)
   - **対象**: フロントエンドASTをFlat ASTsからZigのStructure of Arrays (multiarraylist) パターンに変換し、hex literalの数値変換機能を完全復旧
   - **主要変更内容**:
     - **multiarraylist構造への変換**: `ExprPool(Vec<Expr>)` → `ExprPool { expr_types: Vec<ExprType>, lhs: Vec<Option<ExprRef>>, ... }`
     - **フィールド別Vec構造**: enum variantごとではなく、各フィールド（lhs, rhs, operator等）ごとに独立したVecを持つ設計
     - **ExprPool::updateメソッド追加**: 新しいmultiarraylist APIで既存expressionを更新できる機能を実装
     - **TypeCheckerの変換追跡機能**: `transformed_exprs: HashMap<ExprRef, Expr>` による変換記録システム
     - **transform_numeric_expr再実装**: 新しいAPIでhex literal変換（0x10 → UInt64(16)）を正常動作
   - **技術的成果**:
     - **116+ compilation errorsを修正**: frontend, interpreter, lua_backendの全ライブラリでAPI変更に対応
     - **メモリレイアウト最適化**: SIMDアクセス向上とキャッシュ効率改善を実現
     - **パフォーマンス向上**: Structure of Arraysパターンによるメモリ局所性の改善
     - **型変換機能完全復旧**: hex_literal_testsが12/12成功（100%成功率）
   - **変換パターン例**:
     - **旧API**: `pool.get(ref.to_index())` → **新API**: `pool.get(&ref)`
     - **旧構造**: `Vec<Expr>` → **新構造**: `Vec<ExprType> + Vec<Option<ExprRef>> + ...`
     - **hex変換**: `0xFF` → `Number("0xFF")` → `UInt64(255)`
   - **実装ファイル**:
     - **frontend/src/ast.rs**: multiarraylist構造とupdateメソッド実装
     - **frontend/src/type_checker.rs**: 変換追跡とapply_expr_transformations実装
     - **interpreter/src/evaluation.rs**: 新API対応の評価ロジック
     - **lua_backend/src/lib.rs**: 116個のコンパイルエラー修正
   - **テスト結果**: 全284テスト継続成功（100%成功率維持）
     - **hex_literal_tests**: 12/12成功 - 0xFFu64, 0x7Fi64, 0x100等の変換確認
     - **既存機能**: 全て正常動作継続（dict, array, struct等）
   - **最終成果**:
     - **プロダクションレベル**: multiarraylist APIへの完全移行完了
     - **数値変換復旧**: hex literalが新しいAPI構造で完全動作
     - **アーキテクチャ改善**: メモリ効率とパフォーマンスの最適化を実現
     - **開発基盤確立**: 将来的なSIMD最適化への道筋を確立

99. **Lua backend LuaJIT bitopモジュール対応** ✅ (2025-08-30完了)
   - **対象**: Lua backendでLuaJIT環境のbitopモジュールサポートとコマンドライン引数による実行時ターゲット選択機能
   - **実装した機能**:
     - **LuaTarget enum**: Lua53（Lua 5.3+）とLuaJIT（LuaJIT with bitop）の2つのターゲットをサポート
     - **コマンドライン引数対応**: 
       - `--luajit`: LuaJITのbitopモジュールを使用するコード生成
       - `--lua53`: Lua 5.3+のネイティブビット演算子を使用（デフォルト）
       - `--help`: 使用方法の表示
     - **ビット演算の互換性レイヤー**:
       - **LuaJIT**: `bit.band()`, `bit.bor()`, `bit.bxor()`, `bit.lshift()`, `bit.rshift()`, `bit.bnot()`関数呼び出し
       - **Lua 5.3+**: `&`, `|`, `~`, `<<`, `>>` ネイティブビット演算子
     - **自動require追加**: LuaJITモードでは生成コードの先頭に`local bit = require('bit')`を自動挿入
   - **技術的実装**:
     - **lua_backend/src/lib.rs**: 
       - `LuaTarget` enum追加とコード生成の条件分岐実装
       - `with_target()` メソッドでターゲット指定機能
       - 単項・二項ビット演算の完全対応
     - **lua_backend/src/main.rs**: 
       - コマンドライン引数解析ロジックの実装
       - 使用方法表示機能とエラーハンドリング
     - **lua_backend/tests/bitwise_tests.rs**: 
       - Lua 5.3+とLuaJIT両対応のテストスイート新規作成
       - `generate_lua_code_with_target()` ヘルパー関数
   - **生成コード例**:
     - **Lua 5.3+**: `val result = 5u64 & 3u64` → `local V_RESULT = (5 & 3)`
     - **LuaJIT**: `val result = 5u64 & 3u64` → `local V_RESULT = bit.band(5, 3)`
   - **テスト結果**: 15/15成功（100%成功率）
     - Lua 5.3+とLuaJIT両方のビット演算コード生成検証
     - 実際のLua実行での動作確認（計算結果一致）
     - 単項・二項演算子の完全テストカバレッジ
   - **実装ファイル**:
     - **lua_backend/src/lib.rs**: ターゲット対応コード生成ロジック
     - **lua_backend/src/main.rs**: コマンドライン引数処理
     - **lua_backend/tests/bitwise_tests.rs**: 包括的テストスイート
   - **技術的成果**:
     - **クロスプラットフォーム対応**: Lua 5.3+とLuaJIT両環境での動作保証
     - **実行時選択**: 同一ソースコードから異なるLua環境向けコード生成
     - **後方互換性**: 既存のLua 5.3+コード生成機能を完全保持
     - **開発者フレンドリー**: コマンドライン引数による直感的なターゲット指定

## 進行中 🚧

*現在進行中のタスクはありません*

## 未実装 📋

95. **ヒープメモリ管理の完全実装**
    - heap_realloc でのデータ保持
    - mem_copy/mem_set の正確な実装
    - 16進数リテラルのサポート

96. **パターンマッチングと列挙型（Enum）**

30. **組み込み関数システム** 🔧
    - 関数呼び出し時の組み込み関数検索
    - 型変換・数学関数の実装

65. **frontendの改善課題** 📋
   - **ドキュメント不足**: 公開APIのdocコメントがほぼない
   - **テストカバレッジ不足**: プロパティベーステストやエッジケースのテストが不在
   - **パフォーマンス設定の固定化**: メモリプールや再帰深度が固定値
   - **コード重複**: AstBuilderのビルダーメソッドが冗長（マクロで統一可能）
   - **型システムの拡張性**: ジェネリクスやトレイトへの対応準備が不足

26. **ドキュメント整備** 📚
    - 言語仕様やAPIドキュメントの整備

28. **動的配列（List型）** 📋
    - 可変長配列の実装
    - push, pop, get等の基本操作
    - 固定配列からの移行パス

29. **Option型によるNull安全性** 🛡️
    - Option<T>型の実装
    - パターンマッチングの基礎

## 検討中の機能

* FFIあるいは他の方法による拡張ライブラリ実装方法の提供
* 動的配列
* 文字列操作
* ラムダ式・クロージャ
* Option型（Null安全性）
* 将来的なモジュール拡張（バージョニング、リモートパッケージ）
* 言語組み込みのテスト機能、フレームワーク
* 言語内からASTの取得、操作

## メモ

- 算術演算と比較演算は既にEnum化により統一済み
- 基本的な言語機能（if/else、for、while）は完全実装済み
- AST変換による型安全性が大幅に向上（frontendで型変換完了）
- 自動型変換機能により、型指定なしリテラルの使い勝手が向上
- **コンテキストベース型推論が完全実装済み** - 関数内の明示的型宣言が他の変数の型推論に影響
- 複雑な複数操作での一貫した型推論：`(a - b) + (c - d)`で全要素が統一型
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
- **str.len()メソッドが完全実装済み** - `"string".len()` 形式でu64型の文字列長を取得可能
- str型の組み込みメソッドシステムを確立、構造体メソッドと統一的に処理
- **索引アクセス構文が完全実装済み** - `x[key]` 読み取り、`x[key] = value` 代入の統一構文
- **辞書（Dict）型システムが完全実装済み** - `dict{key: value}` リテラル、`dict[K, V]` 型注釈をサポート
- **Dict型Objectキーサポートが完全実装済み** - Bool, Int64, UInt64, String を辞書キーとして使用可能
- **汎用HashMap<ObjectKey, RcObject>アーキテクチャ** - 型安全なObjectキー辞書操作をランタイムレベルで完全サポート
- **構造体索引演算子オーバーロードが完全実装済み** - `__getitem__`/`__setitem__` メソッドによるカスタム索引操作
- **Self キーワードが完全実装済み** - impl ブロック内で構造体名を `Self` で参照可能
- **統合索引システム** - 配列、辞書、カスタム構造体で統一されたインデックスアクセス `x[key]` 構文
- **二重文字列型システムが完全実装済み** - `ConstString`（リテラル用）と`String`（動的生成用）の最適化された文字列システム
- **文字列メモリ効率化完了** - String Interner汚染回避、動的文字列の直接アクセス、不変vs可変の型レベル区別
- **Go-style module system fully implemented** - Complete 4-phase implementation (syntax, resolution, type checking, runtime)
- **Module namespace support** - Package declarations, import statements, qualified name resolution
- **プロダクションレベル達成** - 深い再帰、複雑ネスト構造を含む実用的プログラム作成が可能
- **包括的テストスイート** - frontend 221テスト + interpreter 74テスト = 合計295テスト成功（100%成功率）
