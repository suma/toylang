# TODO - Interpreter Improvements

## 完了済み ✅

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

95. **ヒープメモリ管理の完全実装** - heap_realloc、mem_copy/mem_set
96. **パターンマッチングと列挙型（Enum）**
30. **組み込み関数システム** - 型変換・数学関数
65. **frontendの改善課題** - docコメント、プロパティベーステスト、コード重複削減
26. **ドキュメント整備** - 言語仕様やAPIドキュメント
28. **動的配列（List型）** - push, pop, get等の基本操作
29. **Option型によるNull安全性** - Option<T>型とパターンマッチング基礎

## 検討中の機能

* FFI/拡張ライブラリ
* 文字列操作
* ラムダ式・クロージャ
* モジュール拡張（バージョニング、リモートパッケージ）
* 言語組み込みテスト機能
* 言語内からのAST取得・操作

## 実装済み機能サマリー

### コア言語機能
- 基本言語機能: if/else/elif、for、while、break/continue、return
- 変数: val（不変）/var（可変）、コンテキストベース型推論
- 固定配列: 型推論対応、インデックス型推論、境界チェック
- 配列スライス: `arr[start..end]`、`arr[..]`、負インデックス`arr[-1]`対応
- 辞書（Dict）型: `dict{key: value}`リテラル、Object型キーサポート
- 構造体: 宣言、implブロック、フィールドアクセス、メソッド、`__getitem__`/`__setitem__`
- 文字列: ConstString/String二重システム、`str.len()`
- コメント: `#`（行）、`/* */`（ブロック）

### 型システム
- 自動型変換・型推論（数値リテラルのサフィックス省略可）
- ジェネリック関数: `fn identity<T>(x: T) -> T`（パース→型推論→実行）
- ジェネリック構造体: `struct Container<T>`、constraint-based型推論
- ネストジェネリック: `Container<Container<T>>`（C++11スタイル`>>`分割）
- Self キーワード: implブロック内での構造体参照

### モジュール・その他
- Go-styleモジュールシステム: package/import/qualified name resolution
- 統合インデックスシステム: 配列・辞書・構造体で統一`x[key]`構文

### テスト状況
- frontend 486テスト + interpreter 301テスト = 合計787テスト（100%成功率）

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
