# CLAUDE.md
以下日本語のみで書いてください。ただし、コード内のコメントとgitコミットメッセージは英語で記述してください。

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Structure

This is a toy programming language implementation in Rust with two main components:

- **frontend/**: Shared parser, AST library, and type checker using rflex for lexer generation
- **interpreter/**: Tree-walking interpreter with comprehensive test suite

The language supports functions, variables (val/var), control flow (if/else, for loops with break/continue), basic arithmetic, advanced type checking with context-based inference, and automatic type conversion.

## Commands

### Building and Running

```bash
# Build frontend library
cd frontend && cargo build

# Build interpreter  
cd interpreter && cargo build

# Run the interpreter
cd interpreter && cargo run <source_file.t>

# Example programs are available in interpreter/example/
cd interpreter && cargo run example/fib.t
```

### Testing

```bash
# Run all tests in interpreter (includes property-based tests)
cd interpreter && cargo test

# Run frontend tests
cd frontend && cargo test

# Run property tests only
cd interpreter && cargo test proptest
```

**Note**: インタープリターの`cargo test`は3つのフェーズで実行されます：
1. `src/lib.rs`のテスト
2. `src/main.rs`のテスト (メインテストスイート)
3. `doc-tests`

テスト結果は各フェーズごとに`running X tests`と表示されます。

### Development

```bash
# Frontend uses build script to generate lexer from lexer.l
cd frontend && cargo build

# Type check with clippy
cd frontend && cargo clippy --all-targets --all-features
cd interpreter && cargo clippy --all-targets --all-features
```

## Language Syntax

Example program structure:
```rust
/*
 * Fibonacci sequence calculator
 * Demonstrates recursive function implementation
 */
fn fib(n: u64) -> u64 {
    # Check base cases
    if n <= 1u64 {
        n
    } else {
        /* Recursive case: sum of two previous numbers */
        fib(n - 1u64) + fib(n - 2u64)
    }
}

fn main() -> u64 {
    val result: u64 = /* Calculate 6th Fibonacci number */ fib(6u64)
    result # Returns 8
}
```

- Functions require explicit return types
- Variables: `val` (immutable), `var` (mutable)
- Types: `u64`, `i64`, `bool`, `str`, `ptr`, `usize`, `dict`, `null`, `Self`
- **Numeric literals**:
  - Type suffix: `42u64` (unsigned 64-bit), `42i64` (signed 64-bit)
  - Hex literals: `0xFFu64`, `0xFFi64`, `0xFF`（型サフィックスなしも可）
  - Without suffix: defaults to `u64`, or automatically determined by type inference
  - Examples: `val x = 42` → `u64` type, `val y: i64 = 42` → automatically converted to `i64`
- Control flow: `if/else`, `for i in start to end`, `while`, `break`, `continue`, `return`
- **`else if` 構文は未サポート**: `if expr {} else if expr {}` は使用不可。代わりに `elif` キーワードを使用すること
  ```rust
  # NG: else if は使えない
  if x > 10 { ... } else if x > 5 { ... } else { ... }

  # OK: elif キーワードを使用
  if x > 10 { ... } elif x > 5 { ... } else { ... }
  ```
- All programs must have a `main()` function
- **No semicolons required**: Statements are separated by newlines, not semicolons
- **Comments**:
  - Single-line comments: `# comment text`
  - Multi-line comments: `/* comment text */` (C/Java/Rust style)
  - Both comment types can be used inline or as standalone statements
  - Multi-line comments do not support nesting
- Don't use ';' symbol for end of statement. We can't use semicolon for separation of statements.
- **OOP・モジュール関連キーワード**: `class`, `struct`, `impl`, `Self`, `enum`, `match`
- **可視性・外部連携**: `pub`（公開）, `extern`（外部関数）
- **モジュールシステム**: `package`, `import`, `as`
- **演算子**:
  - 範囲: `..`（例: `0..10`）式として使用可能。`for i in 0..10 { ... }` と `val r = 0..10` の両方が書ける。`for i in 0 to 10` の旧形式も引き続き有効
  - スコープ解決: `::`
  - match arm の区切り: `=>`
  - ビット演算: `&`, `|`, `^`, `~`, `<<`, `>>`
  - 論理演算: `&&`, `||`, `!`
- **Enum と match**（Phase 1/2、unit と tuple variant）:
  ```rust
  enum Shape {
      Circle(i64),
      Rect(i64, i64),
      Point,
  }

  fn area(s: Shape) -> i64 {
      match s {
          Shape::Circle(r) => r * r * 3i64,
          Shape::Rect(w, h) => w * h,
          Shape::Point => 0i64,
      }
  }
  ```
  - unit variant は `Color::Red`、tuple variant は `Shape::Circle(5i64)` で生成
  - 各 arm は式。全 arm が同じ型でなければならない
  - パターン: `Enum::Variant` / `Enum::Variant(x, _, y)`（`_` は discard） / `_`（全 catch）
  - 網羅性チェック: wildcard がなく variant が欠落していると型チェックエラー
  - 到達性チェック: 同じ variant を 2 回 arm に書く、または `_` の後ろに arm を置くと型チェックエラー
  - ジェネリック enum: `enum Option<T> { None, Some(T) }` をサポート。タプル variant の引数から型パラメータを推論、ユニット variant（`None`）は `val x: Option<i64> = Option::None` のように型注釈から補完
  - ネストパターンは未対応（Phase 4 以降）

## Architecture Notes

- **Frontend Library**: 
  - AST uses memory pools (StmtPool, ExprPool) for efficient allocation
  - Generates lexer from flex-style `.l` file using rflex crate
  - Advanced type checker with context-based inference and automatic type conversion
  - Shared between different backends (currently interpreter)

- **Interpreter**: 
  - Tree-walking interpreter using Rc<RefCell<Object>> for runtime values
  - Type checker runs before execution for type safety
  - Comprehensive test suite with 40+ tests including property-based testing
  - Example programs in `interpreter/example/` directory

## Task Management

プロジェクトの改善タスクは `todo.md` で管理されています。Claude Codeは以下のワークフローに従ってください：

### タスク管理プロセス
1. **TodoRead/TodoWrite ツールの使用**: セッション中の一時的なタスク追跡に使用
2. **todo.mdファイルの更新**: 永続的な記録として、完了したタスクや新しい課題をファイルに反映
3. **定期的な同期**: Todoツールとtodo.mdの内容を定期的に同期

### ファイル構造
- `todo.md`: マスタータスクリスト
  - 完了済み、進行中、未実装のセクションに分類
  - 優先度と詳細な説明を含む
  - 推奨順序とメモを記載

## ビルトイン関数
- ビルトイン関数の実装方針は `BUILTIN_ARCHITECTURE.md` に記述されています

## 入出力ビルトイン

- `print(value)` — stdout に値を出力（改行なし）
- `println(value)` — stdout に値を出力 + 改行
- 任意の型を受け取り、`Object::to_display_string` で整形。文字列は引用符なし、構造体 / dict はフィールド名順にソートして決定的な出力
- ユーザ向けの日常的な I/O なので、`heap_alloc` 等の低レベル builtin と違って `__builtin_` prefix は付けない
- 使用例: `interpreter/example/print_demo.t`

## Allocator システム

`with allocator = ...` による lexical scope で allocator を切り替えられる。heap 系 builtin は常に現在の allocator を経由する。詳細な設計と進捗は `ALLOCATOR_PLAN.md` を参照。

### 主要な構文・ビルトイン

| 要素 | 説明 |
|---|---|
| `with allocator = <expr> { body }` | スコープ内で allocator を差し替え |
| `ambient` | 現在の allocator（式として使える糖衣） |
| `__builtin_current_allocator()` | 現在の allocator（スタック top） |
| `__builtin_default_allocator()` | プロセス全体の global allocator |
| `__builtin_arena_allocator()` | 新規 arena（drop で一括解放） |
| `__builtin_fixed_buffer_allocator(capacity: u64)` | バイト数 quota 付き（超過で null） |
| `fn f<A: Allocator>(a: A)` | allocator をジェネリックに受け取る関数 |

### 典型的な使い方

- 基本: `interpreter/example/allocator_basic.t`
- bound 付き汎用関数 + `ambient` + 自動挿入: `interpreter/example/allocator_bounded.t`
- ユーザ空間の動的リスト（struct + impl + heap builtin）: `interpreter/example/allocator_list.t`

### 意味論のポイント

- `with` は lexical scope。ネストは push/pop、body の exit path（値・return・break・error）すべてで必ず pop される
- `Allocator` 値は `Rc::ptr_eq` で同値性を判定。`==` / `!=` のみサポート（順序比較は不可）
- `<A: Allocator>` の**末尾**パラメータが省略された呼び出しは、型チェック時に `ambient`（= `__builtin_current_allocator()`）が自動挿入される
- bound は関数・struct・impl の各レベルで伝播。呼び出し側 generic の bound が一致すれば連鎖通過
- arena は個別 `free` を no-op とし、`Drop` で一括解放。fixed_buffer は quota 超過で `0`（null ポインタ）を返す
- `List<T>` のようなコレクションは言語組み込みではなく、`struct` + `impl` + `__builtin_heap_alloc/realloc/ptr_read/ptr_write` で書く。これらの builtin が自動で現在の allocator を通るため、`with allocator = arena { ... }` で囲むだけで arena 経由になる

### 進捗（2026-04-19 現在）

- Phase 1a/1b/1c: 構文・ランタイム・`Allocator` trait・`GlobalAllocator`・`ArenaAllocator`・`FixedBufferAllocator` 完了
- Phase 2a/2b: `<A: Allocator>` bound（関数・struct・impl）、呼び出し時 bound 検証、bound 連鎖 完了
- Phase 3 部分: `ambient` 糖衣、自動 ambient 挿入、ユーザ空間 `List<u64>` 完了
- 未完: ジェネリック `List<T>` 一般化、IR レベルの `AllocatorBinding`、native codegen（Phase 4 以降）

## テスト計画

toylang コンパイラ開発の包括的なテスト戦略と計画は `TEST_PLAN.md` に記述されています。このドキュメントでは以下の内容を扱っています：

### テスト層の設計
- **ユニットテスト**: パーサー、型チェッカー、字句解析器のコンポーネント
- **統合テスト**: フロントエンド全体（解析 → 型チェック）
- **エンドツーエンドテスト**: インタープリター実行動作
- **プロパティベーステスト**: 言語システムの数学的性質（一貫性、正確性）

### テストカバレッジ領域
- コア機能：ジェネリック型推論、配列スライス、構造体操作
- 言語機能：モジュールシステム、メソッド呼び出し、複合型
- エッジケース：境界条件、エラーハンドリング、互換性

### テスト実行
```bash
# 全テストを実行
cd frontend && cargo test && cd ../interpreter && cargo test

# 特定のテストスイートを実行
cd interpreter && cargo test proptest
```

### 将来のテスト計画
- Enum とパターンマッチングのテスト
- Option 型と Null 安全性のテスト
- 動的配列と高度な型システムのテスト
- パフォーマンス最適化の検証

詳細なテスト要件、戦略、実装チェックリストについては `TEST_PLAN.md` を参照してください。

### Frontend テスト整理計画
frontend コンポーネント内のテストコード整理計画は `FRONTEND_TEST_PLAN.md` に記述されています。この計画には以下の内容が含まれます：

- **現在の構成分析**: ユニットテストと統合テストの配置状況
- **テスト統合戦略**: 統合テストファイルの論理的な再構成
- **カテゴリ別再構成**: 型システム、ジェネリック、配列・コレクション、モジュール、エラーハンドリング
- **実装ロードマップ**: 段階的な改善計画
- **品質指標**: テストカバレッジ、ドキュメンテーション基準

詳細な実装方法とロードマップについては `FRONTEND_TEST_PLAN.md` を参照してください。

### Interpreter テスト整理計画
interpreter コンポーネント内のテストコード整理計画は `INTERPRETER_TEST_PLAN.md` に記述されています。この計画には以下の内容が含まれます：

- **現在の構成分析**: 296個のテストが35ファイルに分散している状況の分析
- **カテゴリ別分類**: コア言語、ジェネリック、コレクション、OOP、メモリ管理など9カテゴリ
- **統合戦略**: 35ファイルを7つの論理的な統合テストファイルに再構成
- **フェーズ別実装計画**: 4-5週間の段階的な改善プロセス
- **品質目標**: テストカバレッジ、ドキュメンテーション、保守性の向上

詳細な実装方法と段階別ロードマップについては `INTERPRETER_TEST_PLAN.md` を参照してください。

### 重要な原則
- 新しい課題を発見した場合は、TodoWriteツールとtodo.mdの両方に追加
- タスク完了時は、todo.mdの該当項目を「完了済み」セクションに移動
- 大きな改善や機能追加後は、todo.mdファイルをgitにコミット
