# CLAUDE.md
以下日本語のみで書いてください。

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
- Types: `u64`, `i64`, `bool`
- **Numeric literals**: 
  - Type suffix: `42u64` (unsigned 64-bit), `42i64` (signed 64-bit)
  - Without suffix: defaults to `u64`, or automatically determined by type inference
  - Examples: `val x = 42` → `u64` type, `val y: i64 = 42` → automatically converted to `i64`
- Control flow: `if/else`, `for i in start to end`, `break`, `continue`
- All programs must have a `main()` function
- **No semicolons required**: Statements are separated by newlines, not semicolons
- **Comments**:
  - Single-line comments: `# comment text`
  - Multi-line comments: `/* comment text */` (C/Java/Rust style)
  - Both comment types can be used inline or as standalone statements
  - Multi-line comments do not support nesting
- Don't use ';' symbol for end of statement. We can't use semicolon for separation of statements.

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

### 重要な原則
- 新しい課題を発見した場合は、TodoWriteツールとtodo.mdの両方に追加
- タスク完了時は、todo.mdの該当項目を「完了済み」セクションに移動
- 大きな改善や機能追加後は、todo.mdファイルをgitにコミット
