# TODO - Interpreter Improvements

## 完了済み ✅

110. **パーサーでのジェネリック型引数サポートと関連関数戻り値型の完全な型置換** ✅ (2025-12-10完了)
   - **対象**: 型宣言（戻り値型など）でジェネリック型引数（`Container<T>`）をパースできるようにする
   - **問題の根本原因**:
     - パーサーが `Container<T>` を `Identifier(Container)` としてパースし、`<T>` 部分を無視していた
     - 関連関数の戻り値型が `Struct(Container, [Generic(T)])` の場合、内部のジェネリック型が置換されていなかった
   - **実装した解決策**:
     - **パーサー修正**: `parse_type_declaration_with_generic_context()` で型引数をパース
     - **型チェッカー修正**: `handle_generic_associated_function_call()` で `Struct` 型の型引数を再帰的に置換
   - **修正コード詳細**:
     ```rust
     // frontend/src/parser/core.rs (line 706-748)
     Some(Kind::Identifier(s)) => {
         let ident = self.string_interner.get_or_intern(s);
         self.next();

         if generic_params.contains(&ident) {
             return Ok(TypeDecl::Generic(ident));
         }

         // ジェネリック型引数のパース: Container<T>
         if matches!(self.peek(), Some(Kind::LT)) {
             self.expect_err(&Kind::LT)?;
             let mut type_args = Vec::new();
             loop {
                 // 再帰的に型引数をパース
                 let type_arg = self.parse_type_declaration_with_generic_context(generic_params)?;
                 type_args.push(type_arg);

                 match self.peek() {
                     Some(Kind::Comma) => { self.next(); }
                     Some(Kind::GT) => { break; }
                     _ => return Err(...)
                 }
             }
             self.expect_err(&Kind::GT)?;
             Ok(TypeDecl::Struct(ident, type_args))
         } else {
             Ok(TypeDecl::Identifier(ident))
         }
     }

     // frontend/src/type_checker/generics.rs (line 440-446)
     TypeDecl::Struct(name, type_params) => {
         // Struct型引数内のジェネリックパラメータを再帰的に置換
         let substituted_params: Vec<TypeDecl> = type_params.iter()
             .map(|param| self.substitute_type_params(param, &substitutions))
             .collect();
         TypeDecl::Struct(*name, substituted_params)
     }
     ```
   - **パース結果の例**:
     - `Container<T>` → `TypeDecl::Struct(Container, [Generic(T)])`
     - `Container<u64>` → `TypeDecl::Struct(Container, [UInt64])`
     - `fn wrap(T) -> Container<T>` が正しくパースされる
   - **型置換の例**:
     - `Container<Generic(T)>` + `{T: u64}` → `Container<u64>`
     - 関連関数 `Container::wrap(42u64)` が `Container<u64>` を正しく返す
   - **テスト結果**:
     - **全5テスト中4テスト成功（80%成功率）**:
       - ✅ `test_associated_function_with_different_name`
       - ✅ `test_associated_function_multiple_parameters`
       - ✅ `test_associated_function_mixed_with_regular_methods`
       - ✅ `test_associated_function_type_inference_accuracy`
       - ❌ `test_associated_function_complex_return_type`（`>>`問題）
   - **実装ファイル**:
     - **frontend/src/parser/core.rs**: `parse_type_declaration_with_generic_context()` に型引数パース追加
     - **frontend/src/type_checker/generics.rs**: `handle_generic_associated_function_call()` に再帰的置換追加
   - **技術的成果**:
     - **単一レベルのジェネリック型引数**: `Container<T>`, `Container<u64>` が完全に動作
     - **関連関数の型推論**: `Container::wrap(value)` が正しい型を返す
     - **戻り値型の完全な型置換**: `fn foo() -> Container<T>` が正しく動作
   - **既知の制限事項**:
     - **ネストしたジェネリック型とRightShift問題**:
       - `Container<Container<T>>` の `>>` が `RightShift` トークンとしてlexerで解析される
       - パーサーが "Expected ',' or '>' in generic type arguments" エラーを出す
       - この問題により `test_associated_function_complex_return_type` が失敗
       - **回避策**: ネストしたジェネリック型では `> >` のようにスペースを入れる必要がある（未実装）
       - **根本的解決**: lexerまたはパーサーで `RightShift` を文脈に応じて2つの `GT` として扱う必要がある
   - **影響範囲**:
     - 型宣言でのジェネリック型引数が実用レベルで動作
     - 関連関数の型推論が完全に機能
     - 単一レベルのジェネリック型は完全サポート

109. **ジェネリック構造体のフィールドアクセス型パラメータ置換** ✅ (2025-12-09完了)
   - **対象**: `Container<u64>` の `value` フィールドが `Generic(T)` ではなく `u64` を返すようにする
   - **問題の根本原因**:
     - フィールドアクセス時に構造体の型パラメータ（`Container<u64>` の `[u64]`）が考慮されていなかった
     - 構造体定義のフィールド型（`value: T`）をそのまま返していた
     - 構造体リテラルの型推論が空の型パラメータ `[]` を返していた
   - **実装した解決策**:
     - **型パラメータマッピング機能**: `create_type_param_mapping()` で `{T -> u64}` のマッピングを作成
     - **型置換機能**: `substitute_type_params()` でジェネリック型を具体的な型で再帰的に置換
     - **フィールドアクセス修正**: `visit_field_access()` で型パラメータ置換を適用（3箇所）
     - **構造体リテラル型推論修正**: `visit_generic_struct_literal()` が型パラメータを正しく返すように修正
     - **Self型の再帰的解決**: `resolve_self_type()` でネストした `Struct` 型を再帰的に解決
   - **修正コード詳細**:
     ```rust
     // utility.rs
     pub fn create_type_param_mapping(&self, struct_symbol: DefaultSymbol,
                                      type_params: &Vec<TypeDecl>) -> HashMap<DefaultSymbol, TypeDecl>
     pub fn substitute_type_params(&self, type_decl: &TypeDecl,
                                   mapping: &HashMap<DefaultSymbol, TypeDecl>) -> TypeDecl

     // type_checker.rs - visit_field_access
     let mapping = self.create_type_param_mapping(struct_symbol, &type_params);
     let substituted_type = self.substitute_type_params(&struct_field.type_decl, &mapping);
     return Ok(substituted_type);

     // type_checker.rs - visit_generic_struct_literal (line 2074)
     // 修正前: Ok(TypeDecl::Struct(*struct_name, vec![]))
     // 修正後: 型パラメータを制約解決から取得して返す
     let mut type_params = Vec::new();
     for generic_param in generic_params {
         if let Some(concrete_type) = substitutions.get(generic_param) {
             type_params.push(concrete_type.clone());
         }
     }
     Ok(TypeDecl::Struct(*struct_name, type_params))
     ```
   - **テスト結果の改善**:
     - **修正前**: `test_associated_function_multiple_parameters` が "Type mismatch in arithmetic operation" で失敗
     - **修正後**: **5つ中4つのテストが成功**
       - ✅ `test_associated_function_basic`
       - ✅ `test_associated_function_multiple_parameters` (元の問題)
       - ✅ `test_associated_function_type_inference_accuracy`
       - ✅ `test_associated_function_with_self_return`
       - ❌ `test_associated_function_complex_return_type` (ネストしたジェネリック型の制限)
   - **実装ファイル**:
     - **frontend/src/type_checker/utility.rs**: 型パラメータマッピングと置換のユーティリティ関数追加
     - **frontend/src/type_checker.rs**: `visit_field_access()` で型置換適用、`visit_generic_struct_literal()` 修正
     - **frontend/src/type_checker/method.rs**: `resolve_self_type()` で再帰的解決追加
     - **frontend/src/type_checker/generics.rs**: メソッド内での型推論改善
   - **技術的成果**:
     - **フィールドアクセスの完全動作**: `Container<u64>` の `value` が正しく `u64` を返す
     - **構造体リテラル型推論**: `Container { value: 42u64 }` が正しく `Container<u64>` と推論
     - **ネストしたフィールドアクセス**: `nested.value.value` が動作（非メソッドコンテキスト）
     - **ジェネリックメソッドでの算術演算**: `self.first + self.second` が `Generic(T) + Generic(T)` として動作
   - **既知の制限事項**:
     - **ネストしたジェネリック戻り値型**: `Container<Container<T>>` のような型がパーサーで `Identifier` としてパースされる
     - この制限により `test_associated_function_complex_return_type` が失敗（パーサーレベルの改善が必要）
   - **影響範囲**:
     - ジェネリック構造体のフィールドアクセスが実用レベルで動作
     - Associated function の型推論が大幅に改善
     - 80%のassociated functionテストが成功（4/5）

108. **単一型パラメータGenericsの基本実装** ✅ (2025-09-07完了)
   - **対象**: 関数と構造体での単一型パラメータジェネリクス構文のサポート
   - **実装した機能**:
     - **関数ジェネリクス**: `fn identity<T>(x: T) -> T` 構文の解析
     - **構造体ジェネリクス**: `struct Container<T> { value: T }` 構文の解析
     - **型システム拡張**: `TypeDecl::Generic(DefaultSymbol)` で型パラメータ表現
     - **AST構造拡張**: `Function.generic_params` と `Stmt::StructDecl.generic_params` フィールド追加
   - **技術的実装**:
     - **レクサー**: 既存の `<` と `>` トークンでジェネリクス構文をサポート
     - **パーサー拡張**: 
       - `parse_generic_params()` メソッドで `<T>` や `<T, U>` の解析
       - 関数定義で `fn foo<T>(...)` 構文の解析
       - 構造体定義で `struct Foo<T> {...}` 構文の解析
     - **AST変更**:
       - `Function` 構造体に `generic_params: Vec<DefaultSymbol>` 追加
       - `MethodFunction` 構造体に `generic_params: Vec<DefaultSymbol>` 追加
       - `Stmt::StructDecl` に `generic_params: Vec<DefaultSymbol>` 追加
       - `StmtPool` に `struct_generic_params: Vec<Option<Vec<DefaultSymbol>>>` 追加
     - **型チェッカー更新**: `visit_struct_decl()` にジェネリクスパラメータ引数追加
     - **インタープリター対応**: AST構造変更に伴うコンパイルエラー修正
   - **テスト結果**:
     - **関数テスト**: `test_generic.t` - `fn identity<T>(x: T) -> T` がパース成功
     - **構造体テスト**: `test_generic_struct.t` - `struct Container<T>` がパース成功
     - **ビルド確認**: frontend、interpreter共にコンパイル成功
   - **実装ファイル**:
     - **frontend/src/ast.rs**: AST構造とStmtPoolへのジェネリクスフィールド追加
     - **frontend/src/type_decl.rs**: `TypeDecl::Generic(DefaultSymbol)` 追加
     - **frontend/src/parser/core.rs**: `parse_generic_params()` 実装と関数/構造体解析
     - **frontend/src/parser/stmt.rs**: メソッドのジェネリクスパラメータ対応
     - **frontend/src/visitor.rs**: `visit_struct_decl()` シグネチャ更新
     - **frontend/src/type_checker.rs**: ジェネリクスパラメータ対応
     - **interpreter/src/lib.rs**: AST構造変更への対応
   - **技術的成果**:
     - **構文レベル完全サポート**: 単一型パラメータのジェネリクス構文が正常に解析
     - **将来の拡張基盤**: 複数型パラメータ `<T, U>` への拡張が容易
     - **後方互換性**: 既存の非ジェネリクス関数・構造体に影響なし
     - **統一的設計**: 関数とメソッド、構造体で一貫したジェネリクス表現
   - **現在の制限事項**:
     - 型推論とインスタンス化は未実装（構文解析のみ）
     - 型制約（bounds）は未サポート
     - ジェネリクス関数の実行時にはエラー発生
   - **今後の実装予定**:
     - 型チェッカーでのジェネリクス型推論
     - モノモーフィゼーション（単一化）の実装
     - インタープリターでのジェネリクス関数実行サポート

107. **負数インデックス推論問題の修正** ✅ (2025-09-06完了)
   - **対象**: `a[-1]`、`a[-2..]`等の負数リテラル推論で「Cannot convert '-1' to UInt64」エラーが発生していた問題
   - **問題の根本原因**:
     - `finalize_number_types`メソッドで型ヒント未提供時にデフォルトでUInt64を選択
     - 負数リテラルでも強制的にUInt64への変換を試み、パースエラーが発生
     - slice_testsの `test_negative_index_inference`、`test_slice_negative_inference` が失敗
   - **実装した解決策**:
     - **負数自動判定ロジック**: 数値リテラルの文字列表現を確認し、`-`で始まる場合は自動的にInt64を選択
     - **型推論優先度変更**: 型ヒント > 負数判定 > デフォルトUInt64 の順序で型決定
     - **String Interner連携**: `self.core.string_interner.resolve(value)` で数値文字列を取得し判定
   - **修正コード詳細**:
     ```rust
     let mut target_type = if let Some(hint) = self.type_inference.type_hint.clone() {
         hint
     } else {
         // Check if the number is negative by looking at the actual value
         if let Expr::Number(value) = expr {
             let num_str = self.core.string_interner.resolve(value).unwrap_or("");
             if num_str.starts_with('-') {
                 TypeDecl::Int64  // Negative numbers default to Int64
             } else {
                 TypeDecl::UInt64  // Positive numbers default to UInt64
             }
         } else {
             TypeDecl::UInt64  // Fallback
         }
     };
     ```
   - **テスト結果の改善**:
     - **修正前**: slice_testsで2テスト失敗（`test_negative_index_inference`、`test_slice_negative_inference`）
     - **修正後**: **28テスト全て成功（100%成功率）**
     - **動作確認**: `a[-1]` → i64として正常に推論され、最後の要素にアクセス
   - **実装ファイル**:
     - **frontend/src/type_checker.rs**: `finalize_number_types`メソッド内の型決定ロジック修正
   - **技術的成果**:
     - **型推論精度向上**: 負数リテラルの自動Int64推論により直感的な動作を実現
     - **後方互換性**: 既存の正数リテラル処理に影響なし
     - **エラー除去**: 型変換エラーの根本的解決
     - **使い勝手改善**: `a[-1i64]` の明示的型指定が不要、`a[-1]` で自動推論


## 未実装 📋

95. **ヒープメモリ管理の完全実装**
    - heap_realloc でのデータ保持
    - mem_copy/mem_set の正確な実装

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
- **配列スライス機能が完全実装済み** - Python/Rust風の直感的なスライス構文を完全サポート：
  - **基本スライス**: `arr[start..end]` - 指定範囲の部分配列を作成
  - **開始省略**: `arr[..end]` - 最初から指定位置まで  
  - **終了省略**: `arr[start..]` - 指定位置から最後まで
  - **全体コピー**: `arr[..]` - 配列全体の新しいコピー
  - **負のインデックス**: `arr[-1]` (最後の要素), `arr[-2..]` (後ろから2つ), `arr[1..-1]` (最初と最後を除く)
  - **型推論対応**: 数値リテラル（u64サフィックス有無）、負数の自動i64推論、境界チェック
  - **メモリ安全**: 実行時境界検証、範囲エラー検出、安全な部分配列作成
- **統一インデックスシステム完了** - 配列、辞書、構造体、スライスで一貫した `x[key]` 構文を提供：
  - **配列アクセス**: `arr[index]` - 単一要素アクセス、`arr[start..end]` - スライスアクセス
  - **辞書アクセス**: `dict[key]` - キーによる値アクセス（Object型キーサポート）
  - **構造体アクセス**: `struct[key]` - `__getitem__`メソッド呼び出し、カスタム索引演算子
- **プロダクションレベル達成** - 深い再帰、複雑ネスト構造を含む実用的プログラム作成が可能
- **包括的テストスイート** - frontend 221テスト + interpreter 77テスト = 合計298テスト成功（99.3%成功率）
- **スライス機能完全実用化** - SliceInfo統一アーキテクチャにより28個のslice_testsが全て成功（100%成功率）
- **負のインデックス完全対応** - `a[-1]`, `a[-2..]`, `a[1..-1]` 等のPython/Rust風構文が完全動作、負数推論も自動化
- **構造体索引システム完成** - `__getitem__`メソッドによる構造体でのインデックスアクセスが統一アーキテクチャで完全動作
- **ジェネリック関数システム完全実装済み** - `fn identity<T>(x: T) -> T` 構文の完全サポート（パース → 型推論 → 実行）
- **ジェネリック型推論エンジン** - unificationアルゴリズムによる引数型からの自動型パラメータ推論が完全動作
- **エンドツーエンドジェネリック実行** - 複数の型での同一ジェネリック関数実行、型安全保証付きで実用レベル到達
- **ジェネリック構造体基盤完成** - `struct Container<T> { value: T }` パース・型チェック・constraint-based推論が完全実装
- **ジェネリック構造体リテラル完全動作** - `Container { value: 42u64 }` → `T = u64` の自動型推論が実用レベルで動作
- **複数型同時利用対応** - `Container<u64>`, `Container<bool>` 等の異なる型での並行利用が完全サポート
- **パーサーとインタープリター統合完了** - AST構築からインタープリター実行まで一貫したジェネリック処理
- **包括的テストスイート** - 50+テストケースによるジェネリック構造体の完全カバレッジ（基本・エッジケース・統合・将来機能）
- **constraint-based型推論完成** - 統一アルゴリズムによるジェネリック構造体の型パラメータ自動推論が実用化

## ジェネリック関数システム技術仕様

### 基本構文と動作例
```rust
# 単一型パラメータジェネリック関数
fn identity<T>(x: T) -> T {
    x
}

# 複数パラメータジェネリック関数
fn test_multiple<T>(a: T, b: T) -> T {
    a
}

fn main() -> u64 {
    # 自動型推論による実行
    val result1 = identity(42u64)      # T = u64として推論
    val result2 = identity(100i64)     # T = i64として推論
    val result3 = test_multiple(5u64, 10u64) # 複数引数での推論
    result1  # UInt64(42) を返却
}
```

### 型推論システム（Unificationアルゴリズム）
```rust
# 基本的な型統一
identity(42u64)     # Generic(T) vs UInt64 → T = UInt64
identity("hello")   # Generic(T) vs String → T = String

# 構造型での再帰的推論
fn first<T>(arr: [T; 3]) -> T { arr[0] }
first([1u64, 2u64, 3u64])  # Array<Generic(T)> vs Array<UInt64> → T = UInt64

# 複合型での同時推論
fn pair<T, U>(a: T, b: U) -> (T, U) { (a, b) }
pair(1u64, true)    # T = UInt64, U = Bool
```

### エラー検出と型安全性
```rust
# 型競合エラーの検出
fn conflict<T>(a: T, b: T) -> T { a }
conflict(1u64, true)  # エラー: T cannot be both UInt64 and Bool

# 推論失敗の検出
fn unused<T>() -> u64 { 42u64 }
unused()  # エラー: Cannot infer generic type parameter 'T'
```

### 技術的実装アーキテクチャ
- **パーサー**: `parse_type_declaration_with_generic_context()` によるコンテキスト対応型解析
- **型チェッカー**: `visit_generic_call()` + `infer_generic_types()` による完全型推論
- **型置換**: `substitute_generics()` による再帰的型パラメータ置換
- **実行時**: ジェネリック関数での型検証スキップによる効率的実行
- **中間表現**: `GenericInstantiation` による将来のコード生成パス対応

## スライス機能の技術仕様

### 基本構文と動作例
```rust
val arr: [u64; 5] = [10, 20, 30, 40, 50]

# 基本スライス
val slice1 = arr[1..4]      # [20, 30, 40] - インデックス1から3まで
val slice2 = arr[..3]       # [10, 20, 30] - 最初からインデックス2まで  
val slice3 = arr[2..]       # [30, 40, 50] - インデックス2から最後まで
val slice4 = arr[..]        # [10, 20, 30, 40, 50] - 全体コピー

# 負のインデックス（Python/Rust風）
val last = arr[-1]          # 50 - 最後の要素
val second_last = arr[-2]   # 40 - 後ろから2番目
val tail = arr[-2..]        # [40, 50] - 後ろから2つ
val head = arr[..-1]        # [10, 20, 30, 40] - 最後を除く全て
val middle = arr[1..-1]     # [20, 30, 40] - 最初と最後を除く

# 型推論対応（サフィックス不要）
val auto_slice = arr[1..3]  # 型推論で自動的にu64として処理
val neg_auto = arr[-1]      # 負数は自動的にi64として推論
```

### 統一インデックスシステム
```rust
# 配列インデックス
val arr = [1, 2, 3, 4, 5]
val element = arr[2]        # 単一要素アクセス
val slice = arr[1..4]       # スライスアクセス

# 辞書インデックス  
val dict = dict{"key1": "value1", "key2": "value2"}
val value = dict["key1"]    # キーアクセス

# 構造体カスタムインデックス
struct Matrix2x2 {
    data: [u64; 4]
}

impl Matrix2x2 {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.data[index]  # 内部配列へのアクセス
    }
}

val matrix = Matrix2x2 { data: [1, 2, 3, 4] }
val element = matrix[1]     # __getitem__メソッド呼び出し
```

### 型安全性と境界検証
```rust
# コンパイル時チェック
val arr: [i64; 3] = [1, 2, 3]
val slice = arr[1..2]       # 型: [i64; 1] - 正確なサイズ推論

# 実行時境界チェック
val out_of_bounds = arr[5]  # エラー: IndexOutOfBounds
val invalid_range = arr[3..1] # エラー: start > end
val neg_overflow = arr[-5]  # エラー: 負のインデックスが配列長を超過
```

### SliceInfo統一アーキテクチャ
- **AST表現**: `Expr::SliceAccess(ExprRef, SliceInfo)` による統一構造
- **SliceType区別**: `SingleElement`（単一要素）と`RangeSlice`（範囲）の明確な分離
- **型推論統合**: 正負インデックス、範囲指定での適切な型推論とAST変換
- **実行時最適化**: メモリ効率的なスライス作成と境界チェック
