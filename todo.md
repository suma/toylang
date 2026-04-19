# TODO - Interpreter Improvements

## 完了済み ✅

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

121. **Allocator システムの導入** - ambient current_allocator + コンパイル時特殊化（詳細は下記）
95. **ヒープメモリ管理の完全実装** - heap_realloc、mem_copy/mem_set
96. **パターンマッチングと列挙型（Enum）**
30. **組み込み関数システム** - 型変換・数学関数
65. **frontendの改善課題** - docコメント、プロパティベーステスト、コード重複削減
26. **ドキュメント整備** - 言語仕様やAPIドキュメント
28. **動的配列（List型）** - push, pop, get等の基本操作
29. **Option型によるNull安全性** - Option<T>型とパターンマッチング基礎

## Allocator システム 実装計画（TODO 121）

### 設計の概要

Zig 型の明示的 allocator をベースにしつつ、デフォルトは ambient な `current_allocator`、hot path では型パラメータ化で単相化する**ハイブリッド方式**。interpreter と将来の native code generator の両方で矛盾なく動作する設計。

#### コア方針

- **IR レベルで allocator を一級の値として表現**し、静的／動的の判断はバックエンドに委ねる
- alloc site ごとに「どの allocator を使うか」の参照を IR に残す（静的定数／型パラメータ／ambient／ローカル変数のいずれか）
- interpreter は素直に実行、compiler は specialize か vtable かを選択

#### 言語表層

```
# ambient（デフォルト）
val x = List<u64>::new()

# スコープで上書き
with allocator = arena { ... }

# hot path は型パラメータで単相化
fn hot<A: Allocator>(data: List<u64, A>) -> u64 { ... }
```

#### Allocator trait

```rust
trait Allocator {
    fn alloc(&self, size: usize, align: usize) -> ptr
    fn free(&self, p: ptr)
    fn realloc(&self, p: ptr, new_size: usize) -> ptr
}
```

3 つの使用形態：
1. `&dyn Allocator` — vtable 経由、動的（interpreter のデフォルト）
2. `A: Allocator` (generic) — 型パラメータ、単相化
3. ambient — `current_allocator()` を参照（糖衣として 1 に展開）

### Phase 別ロードマップ

#### Phase 1: Interpreter 基盤（最優先）

- [ ] `Allocator` trait を frontend に定義
- [ ] 標準 allocator 実装：`GlobalAllocator`、`ArenaAllocator`、`FixedBufferAllocator`
- [ ] `EvaluationContext` に `allocator_stack: Vec<Rc<dyn Allocator>>` を追加
- [ ] `with allocator = expr { ... }` の構文・パーサ・AST ノード
- [ ] `with` の enter/exit で push/pop、lexical scope を保証
- [ ] 既存の `heap_alloc`/`heap_free`/`heap_realloc` を allocator_stack 経由に書き換え
- [ ] `current_allocator()` ビルトイン関数
- [ ] interpreter テスト（単体・統合）

#### Phase 2: 型システム拡張

- [ ] `fn f<A: Allocator>(...)` のパース・型チェック（既存ジェネリクス機構を流用）
- [ ] `List<T, A>`、`Box<T, A>` 等のコレクションに allocator 型パラメータを追加
- [ ] `dyn Allocator` vs `impl Allocator` の区別を型システムに組み込み
- [ ] allocator 型パラメータのデフォルト値（省略時は ambient）
- [ ] 型チェックテスト

#### Phase 3: IR 整備（interpreter/compiler 共用）

- [ ] 下位 IR を設計。alloc site ごとに `AllocatorBinding` を付与
  - `AllocatorBinding::Static(allocator_id)` — コンパイル時定数
  - `AllocatorBinding::Generic(type_param)` — 型パラメータ
  - `AllocatorBinding::Ambient` — 実行時スタック
  - `AllocatorBinding::Local(var_id)` — ローカル変数
- [ ] 型チェック後に AST → IR への lowering パスを追加
- [ ] `with` ブロックの allocator 式が compile-time 定数かを判定し、内部の `Ambient` を `Static` に置換するパス
- [ ] interpreter を IR 経由で動かす経路の整備（optional、直接 AST から読んでも可）

#### Phase 4: Native codegen MVP

- [ ] バックエンド選定（Cranelift / LLVM / 独自）
- [ ] 呼び出し規約：案A（allocator を隠しパラメータ化、全関数に `&dyn Allocator` を暗黙追加）
- [ ] `with` は関数呼び出し時の引数差し替えにコンパイル
- [ ] `alloc` は vtable 呼び出し
- [ ] 最小限の動作する静的バイナリ生成
- [ ] 生成バイナリの実行テスト

#### Phase 5: 最適化パス

- [ ] 定数伝搬パス：`with allocator = CONST { ... }` 内の vtable 呼び出しを具体呼び出しに devirtualize
- [ ] 単相化パス：`#[specialize_allocator]` 属性または compile-time 定数 allocator が使われている関数を allocator 型ごとに複製
- [ ] インライン化による alloc 呼び出しの完全消去（arena 等）
- [ ] ベンチマーク：hot path で vtable オーバーヘッドがゼロに近いことを確認

### 設計上の注意点

- **alloc/free の allocator 不一致**：ポインタにヘッダで allocator ID を埋める、または arena 系のみサポートして個別 `free` を型エラーにする
- **クロージャのキャプチャ**：呼び出し時の ambient を使う（Odin/Jai 方式）。キャプチャ時固定が必要なら `with` で明示
- **interpreter/compiler の挙動差**：観測可能な動作は同一とする（allocator の副作用が見える場合の呼び出し順序含む）
- **関数境界での ambient 漏れ**：`with` は lexical のみ。呼ばれた先に自動伝搬し、戻る時点で元に戻る

### 参考とする既存言語

- **Zig** — 明示的 allocator、comptime で単相化可能
- **Odin / Jai** — `context.allocator` による ambient 方式
- **Rust** — `Box<T, A: Allocator>` による型パラメータ単相化
- **C++ std::pmr** — vtable ベースの実行時 allocator

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
