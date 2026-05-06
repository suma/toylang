# CLAUDE.md
以下日本語のみで書いてください。ただし、コード内のコメントとgitコミットメッセージは英語で記述してください。

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> **言語仕様の正本は [`docs/language.md`](docs/language.md)** にまとまっています。
> 構文・型・セマンティクスを確認したい場合はそちらを参照してください。
> 本ファイルは Claude Code 向けの運用ガイダンス（プロジェクト構成、ビルド・
> テストコマンド、タスク管理ワークフロー）を中心に置きます。重複を許容する
> 範囲では言語例を残していますが、最新の挙動は `docs/language.md` を信頼
> してください。

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

**推奨**: `cargo nextest` を使用する。並列実行で速く、出力もパッケージ・テストごとに整理される。

```bash
# Run all tests across the workspace with nextest (preferred)
cargo nextest run

# Filter by package / test name
cargo nextest run -p compiler
cargo nextest run -p interpreter proptest
cargo nextest run -E 'test(=basic_arithmetic)'
```

`cargo test` も引き続き使用可能 (doc-tests は nextest が実行しないので必要なときは併用):

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
- Types: `u64`, `i64`, `f64`, `bool`, `str`, `ptr`, `usize`, `dict`, `null`, `Self`
- **Numeric literals**:
  - Type suffix: `42u64` (unsigned 64-bit), `42i64` (signed 64-bit), `1.5f64` / `42f64` (IEEE 754 double)
  - Hex literals: `0xFFu64`, `0xFFi64`, `0xFF`（型サフィックスなしも可）
  - Without suffix: defaults to `u64`, or automatically determined by type inference
  - Examples: `val x = 42` → `u64` type, `val y: i64 = 42` → automatically converted to `i64`
  - **f64 リテラルは必ず `f64` サフィックスを付ける**: タプルアクセス `outer.0.1` のような構文との曖昧性を避けるため、`1.5` 単体は許可しない。整数 → f64 への暗黙変換も無いので、`1.0f64` または `1f64` と書く（必要なら `as f64` キャスト）
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
- **トップレベル `const` 宣言**: `const NAME: Type = expr` を関数の外側に書ける。型注釈必須、起動時に 1 回評価して全関数から参照できる immutable な束縛になる。先行 const は参照可（前方参照は不可）。詳細は [`docs/language.md`](docs/language.md)
- **`panic("msg")` ビルトイン**: 実行を中断するメッセージ付き panic。型検査では「Unknown」を返す扱いで、`if cond { panic("...") } else { value }` のような式位置でも使える。関数全体が panic で発散する場合も戻り型と関係なく型検査が通る
- **`assert(cond, "msg")` ビルトイン**: `cond` が false のときだけ `panic(msg)` する糖衣。`(bool, str) -> ()`。message は false 時にのみ評価される。JIT は `brif cond, cont, fail; fail: call jit_panic; trap` で lower（success path はオーバヘッド最小、failure path は panic と同じ helper）
- **実行時例外 (try/catch/throw) は導入しない**: 言語仕様として例外機構を持たない。回復不能な失敗は `panic("...")` で即時停止 (process exit)、回復可能な失敗は `enum Result<T, E>` / `enum Option<T>` を戻り値で返して呼び出し側で `match` する。例外用の予約語 (`try` / `catch` / `throw` / `finally`) は parser で受理しない。`requires` / `ensures` 違反も `panic` 経路で停止する (例外として伝播しない)
- **OOP・モジュール関連キーワード**: `class`, `struct`, `trait`, `impl`, `Self`, `enum`, `match`
- **`trait` 宣言と `impl <Trait> for <Type>`**: 共通インターフェースを定義する仕組み。
  ```rust
  trait Greet {
      fn greet(self: Self) -> str
  }
  impl Greet for Dog {
      fn greet(self: Self) -> str { "Woof!" }
  }
  fn announce<T: Greet>(x: T) -> str { x.greet() }
  ```
  - trait 本体には method の **シグネチャのみ**を書く（body 無し）。`requires` / `ensures` 節も書ける
  - `impl <Trait> for <Type> { ... }` は body 付き method を提供。型チェッカーが trait のシグネチャと比較し、不足 method や型不一致を検出
  - 型パラメータ bound `<T: SomeTrait>` を関数・struct・impl に書ける。呼び出し時に「実型がその trait を実装しているか」を検証
  - 実装メソッドは inherent method としても登録されるので `value.trait_method()` 形式で直接呼べる
  - **未対応（後続 phase）**: trait ジェネリクス（`trait Foo<T>`）、デフォルトメソッド、複数 bound（`<T: A + B>`）、trait 継承、`dyn Trait`、associated types
- **クロージャ / ラムダ**: `fn(params) -> R { body }` 形式の anonymous function literal、関数型は `fn (T1, T2) -> R` (推奨、`fn` prefix で意図を明示) または `(T1, T2) -> R` (bare 形、後方互換) を parameter / return / val 注釈 / **struct field 型** 位置に書ける。bind は `val f = fn(x: i64) -> i64 { x + 1i64 }` (closure literal から型推論)、または `val f: fn (i64) -> i64 = fn(x: i64) -> i64 { x + 1i64 }` (明示注釈)。struct field に格納する場合は `struct S { f: fn (i64) -> i64 }`、call は `s.f(args)` (field-call dispatch)。free var capture は creation time の snapshot (primitive は値コピー、compound は Rc 共有)。**backend coverage**: interpreter は full support (literals + captures + HOF args + return + nest)、JIT は silent fallback、**AOT compiler は env-based ABI 統一 (Phase 6b) で direct call + HOF 引数の両方で capturing/non-capturing 両対応** — 残: closure を return / field 格納、narrow int capture (Phase 6c)。captures は 8-byte scalar (i64/u64/f64/bool) のみ。closure value = env_ptr (`[fn_ptr, cap0, cap1, ...]` を heap allocate)、callee body の第 1 param は env: U64。詳細は [`docs/language.md` → Closures](docs/language.md)。
- **Design by Contract キーワード**: `requires`（事前条件）, `ensures`（事後条件）。関数 / メソッドの `-> ReturnType` の後、body `{` の前に複数並べられる。各節は bool 式で、AND 合成。`ensures` 内では `result` が戻り値を指す。違反時は `ContractViolation` エラーで停止。`INTERPRETER_CONTRACTS=all|pre|post|off`（unset = `all`）で `requires` / `ensures` を独立に切り替えられる（D の `-release` 相当）
- **可視性・外部連携**: `pub`（公開）, `extern`（外部関数）
- **モジュールシステム**: `package`, `import`, `as`
- **演算子**:
  - 算術: `+`, `-`, `*`, `/`, `%`（剰余・truncated remainder で `(-7) % 3 == -1`）
  - 複合代入: `+=`, `-=`, `*=`, `/=`, `%=`（パーサで `lhs op= rhs` を `lhs = lhs op rhs` に desugar。LHS は identifier / フィールドアクセス対応）
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
  - リテラルパターン: scrutinee が `bool`/`i64`/`u64`/`str` のとき、`0i64 => ...` / `true => ...` / `"hello" => ...` のようにリテラルで分岐可能。`bool` は両値で網羅、整数・文字列は wildcard 必須
  - ネストパターン: `Option::Some(Option::Some(v))` や `Box::Put(Color::Red)` のように、タプル variant のサブパターンに再帰的にパターンを書ける。サブパターン位置には Name バインディング、`_` ワイルドカード、リテラル、ネストした enum variant を記述可能

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

## Cranelift JIT

`interpreter` には数値 / bool 関数を cranelift で native code 化するオプトインの JIT が入っている。`INTERPRETER_JIT=1` で有効化、cargo feature `jit` (default on) でビルド時にも切替可。サポート範囲・性能・skip 理由・拡張ロードマップは `JIT.md` を参照。

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
| `__builtin_sizeof(value)` | 値のバイトサイズ（u64）。primitive に加え struct（フィールド合計）/ enum（1-byte タグ + payload）/ tuple / array をサポート。generic `T` の実体サイズ取得に使う |
| `with allocator = a { ... }` | scope 内で allocator を有効化、内部の `__builtin_heap_alloc` 等が経由する |

### 典型的な使い方

- 基本: `interpreter/example/allocator_basic.t`
- bound 付き汎用関数 + `ambient` + 自動挿入: `interpreter/example/allocator_bounded.t`
- ユーザ空間の動的リスト（struct + impl + heap builtin）: `interpreter/example/allocator_list.t`

### 意味論のポイント

- `with` は lexical scope。ネストは push/pop、body の exit path（値・return・break・error）すべてで必ず pop される
- `Allocator` 値は `Rc::ptr_eq` で同値性を判定。`==` / `!=` のみサポート（順序比較は不可）
- 関数の引数として `Allocator` を渡す形は推奨しない (関数は `with allocator = ...` の active stack を経由して暗黙的に allocator を使う)
- arena は個別 `free` を no-op とし、`Drop` で一括解放。fixed_buffer は quota 超過で `0`（null ポインタ）を返す
- `List<T>` のようなコレクションは言語組み込みではなく、`struct` + `impl` + `__builtin_heap_alloc/realloc/ptr_read/ptr_write` で書く。これらの builtin が自動で現在の allocator を通るため、`with allocator = arena { ... }` で囲むだけで arena 経由になる
- `__builtin_ptr_write(p, off, value)` は任意型の値を受け取り、`__builtin_ptr_read(p, off)` は呼び出し側の型ヒント（`val v: T = ...` など）に沿って値を返す。内部的には typed-slot map に値を保存しているため、`List<i64>` / `List<bool>` / `List<MyStruct>` もそのまま動作する

### 進捗

- 構文・ランタイム・`GlobalAllocator`・`ArenaAllocator`・`FixedBufferAllocator` 完了
- `ambient` 糖衣、`with allocator = ...` 経由の active stack dispatch 完了
- stdlib (`core/std/allocator.t`) に `trait Alloc` + Wrapper 構造体 (`Global` / `Arena` / `FixedBuffer`) 完了
- AOT native codegen 完了 (#121 Phase A / B-min / B-rest Items 1+3 + Item 2 cleanup + arena_drop)

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
