# TODO - Interpreter Improvements

## 完了済み ✅

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

96. **Enum/match 拡張** — Phase 1/2/2c/3（unit + tuple + generic variant、バインディング、網羅性 + 到達性チェック）完了。ネストパターン（`Some(Some(x))`）、複雑な推論、標準 Option/Result ライブラリは未実装
29. **Option<T> を標準的に提供** — ジェネリック enum は動作中。ユーザ空間で書ける（`enum Option<T> { None, Some(T) }`）。標準ライブラリとして組み込むかは別議論
30. **組み込み関数システム** — 型変換（u64 ↔ i64 は既に `as` で可能）、数学関数（`abs`, `min`, `max`, `pow`, `sqrt`）
65. **frontendの改善課題** — docコメント拡充、プロパティベーステスト追加、コード重複削減
26. **ドキュメント整備** — 言語仕様 / API ドキュメント
121. **Allocator システム残作業** — ジェネリック `List<T>` 一般化、IR レベルの `AllocatorBinding`、Phase 4 以降の native codegen（詳細は `ALLOCATOR_PLAN.md`）

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
- 合計 870 テスト（100% 成功率、2026-04-21 時点）

### パーサーの既知制限事項
- bare `self` 構文非対応（`self: Self` が必要）
- `else if` 未サポート（`elif`を使用）
- `val` はキーワードのためパラメータ名に使用不可
