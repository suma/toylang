# CLAUDE.md
以下日本語のみで書いてください。

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Structure

This is a toy programming language implementation in Rust with three main components:

- **Root crate (langc)**: LLVM-based compiler that compiles to machine code via LLVM IR
- **frontend/**: Shared parser and AST library using rflex for lexer generation
- **interpreter/**: Tree-walking interpreter for the same language

The language supports functions, variables (val/var), control flow (if/else, for loops with break/continue), basic arithmetic, and type checking.

## Commands

### Building and Running

```bash
# Build all components
cargo build

# Run the LLVM compiler (generates .ll files)
cargo run -- <source_file.t>

# Run the interpreter 
cd interpreter && cargo run <source_file.t>
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
cargo clippy --all-targets --all-features
```

## Language Syntax

Example program structure:
```rust
fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}

fn main() -> u64 {
    fib(6u64)
}
```

- Functions require explicit return types
- Variables: `val` (immutable), `var` (mutable)
- Types: `u64`, `bool`
- Control flow: `if/else`, `for i in start to end`, `break`, `continue`
- All programs must have a `main()` function

## Architecture Notes

- AST uses memory pools (StmtPool, ExprPool) for efficient allocation
- Frontend generates lexer from flex-style `.l` file using rflex crate
- LLVM compiler requires LLVM 10.0 (uses inkwell crate)
- Interpreter uses Rc<RefCell<Object>> for runtime values
- Type checker runs before execution in interpreter
- Property-based testing validates language semantics

## Task Management

プロジェクトの改善タスクは `interpreter/todo.md` で管理されています。Claude Codeは以下のワークフローに従ってください：

### タスク管理プロセス
1. **TodoRead/TodoWrite ツールの使用**: セッション中の一時的なタスク追跡に使用
2. **todo.mdファイルの更新**: 永続的な記録として、完了したタスクや新しい課題をファイルに反映
3. **定期的な同期**: Todoツールとtodo.mdの内容を定期的に同期

### ファイル構造
- `interpreter/todo.md`: マスタータスクリスト
  - 完了済み、進行中、未実装のセクションに分類
  - 優先度と詳細な説明を含む
  - 推奨順序とメモを記載

### 重要な原則
- 新しい課題を発見した場合は、TodoWriteツールとtodo.mdの両方に追加
- タスク完了時は、todo.mdの該当項目を「完了済み」セクションに移動
- 大きな改善や機能追加後は、todo.mdファイルをgitにコミット
