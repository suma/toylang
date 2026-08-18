# CLAUDE.md
以下日本語のみで書いてください。ただし、コード内のコメントとgitコミットメッセージは英語で記述してください。

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## どこを見ればいいか

本ファイルは**作業中に必要な運用ガイダンス**だけを置く (ビルド・テスト
コマンド、横断的な変更の注意、タスク管理ワークフロー)。それ以外は移した。

| 知りたいこと | 見る場所 |
|---|---|
| **「name resolution はどこか」** — 関心事 → 実装サイト | [`design-docs/CODE_MAP.md`](design-docs/CODE_MAP.md) |
| 構文・型・セマンティクスの**正本** | [`docs/language.md`](docs/language.md) |
| いつ何が landing したか / 未実装項目 | [`design-docs/todo.md`](design-docs/todo.md) |
| 機能ごとの実装詳細・フェーズ履歴 | [`design-docs/FEATURE_NOTES.md`](design-docs/FEATURE_NOTES.md) |
| LLM 向けの診断・テスト機能の設計 | [`design-docs/LLM_FEEDBACK_LOOP.md`](design-docs/LLM_FEEDBACK_LOOP.md) |
| このリポジトリで LLM が作業する際の指針 | [`design-docs/COMPILER_DEV_LOOP.md`](design-docs/COMPILER_DEV_LOOP.md) |

以下の「Language Syntax」節は**日常的に踏む要点の早見表**であって仕様書ではない。
挙動が食い違ったら `docs/language.md` を信頼すること。

## Project Structure

This is a toy programming language implementation in Rust with two main components:

- **frontend/**: Shared parser, AST library, and type checker using rflex for lexer generation
- **interpreter/**: Tree-walking interpreter with comprehensive test suite

The language supports functions, variables (val/var), control flow (if/else, for loops with break/continue), basic arithmetic, advanced type checking with context-based inference, and automatic type conversion.

## Commands

> すべてリポジトリルートから実行する。`cd <crate> && cargo ...` は使わない —
> `-p <crate>` で同じことができ、cd はツール実行時に余計な確認を挟む。

### Building and Running

```bash
# 型エラーだけ知りたいとき (最速。コード生成をしない)
cargo check --workspace --message-format=short

# ビルド
cargo build -p frontend
cargo build -p interpreter
cargo build -p compiler

# インタプリタで実行
cargo run -q -p interpreter -- <source_file.t>
cargo run -q -p interpreter -- interpreter/example/fib.t

# AOT コンパイル
cargo run -q -p compiler -- <source_file.t> -o <output>

# `test "name" { ... }` ブロックを実行 (LLM-LOOP P4)
cargo run -q -p interpreter -- --test <source_file.t>

# `requires` / `ensures` を入力フィルタ + オラクルとして
# プロパティテストし、最小反例を出す (LLM-LOOP P5)
cargo run -q -p interpreter -- --check <source_file.t>
cargo run -q -p interpreter -- --check --seed=0x99 <source_file.t>   # 再現
```

### 実行せずに聞く / まとめて確認する (LLM-LOOP P7, DEV-LOOP D6)

```bash
# エラーコードの解説 (原因カテゴリ + 再現例 + 直し方)
cargo run -q -p interpreter -- --explain E0003
cargo run -q -p interpreter -- --explain          # 全コードの 1 行要約

# モジュールが提供するシグネチャ一覧 (contract 込み、body 無し)。
# `core/std/*.t` を grep する代わりに使う
cargo run -q -p interpreter -- --api core/std/string.t

# 3 バックエンド (interpreter / JIT / AOT) を 1 コマンドで実行し、
# 不一致だけ報告する。一致なら stderr に 1 行
cargo run -q -p compiler -- <source_file.t> --all-backends

# メモリ確保の集計を実行後に出す (MEMORY_PROFILING M1)
cargo run -q -p interpreter -- --profile=mem <source_file.t>
cargo run -q -p compiler -- <source_file.t> --all-backends --profile=mem
TOY_PROFILE_MEM=1 ./compiled_binary          # AOT バイナリ単体

# 同じレポートを JSON で (M4)。`leaks` は空でも `[]` が出る
cargo run -q -p interpreter -- --profile=mem --profile-format=json <source_file.t>
TOY_PROFILE_MEM=json ./compiled_binary

# 入力ファイル名 `-` で stdin から読む。スクラッチファイルを作らずに済む
echo 'fn main() -> u64 { 0u64 }' | cargo run -q -p interpreter -- --check -
echo 'fn main() -> u64 { 7u64 }' | cargo run -q -p compiler -- - --all-backends
```

**型ホール**: `val x: _ = expr` と書くと推論結果を報告して停止する
(`[E0011] type hole: \`x\` has type \`i64\``)。1 回の実行でファイル中の
全ホールが答えられ、束縛は推論した型で登録されるので後続がカスケードしない。

**`--message-format=short`** を付けると診断が `path:line:col: error[CODE]: msg`
の 1 行形式になる (デフォルトのスニペット付き形式は 1 エラーあたり ~11 行)。
位置情報は保持されるので、機械的に読む場面ではこちらが適している。
cargo の config / 環境変数では設定できないのでフラグで渡すこと。

### Testing

**`cargo nextest` を使う。**

```bash
# ワークスペース全体
cargo nextest run

# パッケージ / テスト名で絞る
cargo nextest run -p compiler
cargo nextest run -p interpreter proptest
cargo nextest run -E 'test(=basic_arithmetic)'

# 失敗の詳細だけでなく全テストの一覧が欲しいとき
cargo nextest run --profile verbose
```

**出力は失敗のみが既定** (`.config/nextest.toml`)。グリーンな全体実行は
**7 行**で終わる (この設定を入れる前は 1641 行だった)。実行自体は ~4 秒なので、
ボトルネックは速度ではなく出力量。全テストの一覧が要るとき
(ハングの二分探索、フィルタが意図通りか確認するとき) だけ
`--profile verbose` を使う。

**環境変数は `.cargo/config.toml` の `[env]` で設定済み**なので、
コマンドラインで前置する必要はない:

| 変数 | 目的 |
|---|---|
| `PROPTEST_CASES=32` | proptest のケース数 (デフォルト 256 は 8x 遅い) |
| `TOYLANG_CRANELIFT_OPT_LEVEL=none` | テスト用の cranelift codegen (~20x 速い) |
| `TOY_LINK_CACHE_DIR` | AOT リンク結果の content-addressed キャッシュ |

nextest の `[profile.*.env]` は**存在しないキー**なので、そこに書いても
黙って無視される (毎回警告が出る)。テスト用の環境変数は必ず
`.cargo/config.toml` の `[env]` に置くこと。

`cargo test` も使用可能 (doc-tests は nextest が実行しないので必要なときに併用):

```bash
cargo test -p interpreter
cargo test --doc --workspace
```

### Development

```bash
# clippy (ワークスペース全体を 1 コマンドで)
cargo clippy --workspace --all-targets --all-features --message-format=short
```

clippy は**無警告が既定状態**。警告が出たら、それは今回の変更が入れたもの。

`frontend` は build script で `lexer.l` から lexer を生成するため、
`lexer.l` を変更したら `cargo build -p frontend` が必要。

### 横断的な変更をするとき

**同じ意味論が 3 バックエンド (tree-walker / IR VM・AOT / JIT) に独立実装されている。**
型チェッカだけ直すと「型は通るが答えが間違う」状態になりうる。

- 意味論を変える修正には `compiler/tests/consistency.rs` の
  `assert_consistent` を使ったテストを必ず追加する
- `compiler/tests/example_consistency.rs` が
  **`interpreter/example/` の全プログラムを 3 バックエンドで突き合わせる**ので、
  example を追加すればカバレッジは自動で増える。
  失敗したら skip リスト (`ERROR_EXAMPLES` / `AOT_UNSUPPORTED` / `KNOWN_CRASHES`)
  に足す前に、まず本当にバックエンドのバグでないかを確認すること —
  リストは**両方向に検査される**ので、直ったのに残っていても失敗する

設定用の構造体 (`CompilerOptions` / `RunOptions` / `SourceLocation`) は
`#[non_exhaustive]` なので、構造体リテラルではなくコンストラクタを使う:

```rust
let mut options = CompilerOptions::new(input_path);
options.emit = EmitKind::Object;

let mut options = RunOptions::default();
options.jit = true;

let loc = SourceLocation::new(line, column, offset, end_offset);
```

フィールドを 1 つ足すたびにワークスペース中のリテラルが壊れるのを防ぐため。

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
- Narrow ints (NUM-W): `u8` / `u16` / `u32` / `i8` / `i16` / `i32` (literal suffix `42u8` / `0xFFi32` 等)。`as` cast で wide ↔ narrow 変換 (暗黙 widening は無し)
- Stdlib types:
  - `char = u32` (Unicode codepoint alias、char literal `'a'` / `'\u{1F600}'` は lexer で `Kind::UInt32` に lex)
  - `String` (`core/std/string.t`) — heap-managed byte buffer の **nominal struct** (`type` alias ではなく独立 struct、`Vec<u8>` と同 memory layout だが nominal identity は別)。inherent method (`new` / `from_str(s)` / `push` / `pop` / `get` / `set` / `size` / `len` / `as_ptr` / `capacity` / `is_empty` / `clear` / `extend_bytes` / `push_str` / `push_char` / `eq` / `to_string`) + 拡張 trait impl (`Substring` / `Trim` / `CaseConvert` / `Concat<String>` / `Contains<String>` / `Split<String, Vec<String>>` from `core/std/str_ops.t`) で `s.len()` / `s.substring(...)` / `s.trim()` / `s.concat(other)` / `s.split(sep)` 等が `str` と同じ call shape で動く (3 backend)。
  - `Vec<T>` (`core/std/collections/vec.t`) — generic dynamic array。`T` が compound (struct/tuple) も AOT 対応 (`__builtin_ptr_read/write` を per-leaf 展開、`AOT-COMPOUND-PTR-RW`)。**`v.sort()` (STDLIB-ORD)** — `impl<T: Ord> Vec<T>` の安定 in-place insertion sort。`Ord` trait (`core/std/ord.t`) は `fn lt(self: Self, other: Self) -> bool` だけで、primitive 全幅 / `f64` / `bool` / `String` (byte-wise) に impl。method 名が `<` 演算子オーバーロードの `lt` と同じなので `impl Ord` は `<` も自動で得る (3 backend)。`T` が Ord でない `sort()` は型エラーにせず runtime/AOT compile で落ちる。
- **`==` / `!=` operator overload** (Phase B) — 同型 struct ペアで `eq(&self, other: &Self) -> bool` method に dispatch (3 backend)。`s == t` で String 比較が動く。
- **`Vec<u8>::push_char(c: char)`** は **UTF-8 encoding 対応** (RFC 3629、1〜4 bytes、surrogate / U+110000+ は panic)。
- **alias-qualified associated function call** も frontend で支援 (`String::from_str("...")` / `String::new()` が直接 dispatch)。
- **Numeric literals**:
  - Type suffix: `42u64` (unsigned 64-bit), `42i64` (signed 64-bit), `1.5f64` / `42f64` (IEEE 754 double)
  - Hex literals: `0xFFu64`, `0xFFi64`, `0xFF`（型サフィックスなしも可）
  - Without suffix: defaults to `u64`, or automatically determined by type inference
  - Examples: `val x = 42` → `u64` type, `val y: i64 = 42` → automatically converted to `i64`
  - **数値リテラル区切り**: `_` を桁の間に挿入できる (`1_000_000u64`、`0xDEAD_BEEFu64`、`3_141.592_653f64`)。最初の文字は数字必須 (`_42` は識別子)。lexer のみで処理、AST / IR / runtime は separator を見ない。
  - **f64 リテラルは必ず `f64` サフィックスを付ける**: タプルアクセス `outer.0.1` のような構文との曖昧性を避けるため、`1.5` 単体は許可しない。整数 → f64 への暗黙変換も無いので、`1.0f64` または `1f64` と書く（必要なら `as f64` キャスト）
- Control flow: `if/else`, `for i in start to end`, `while`, `break`, `continue`, `return`
- **Iterator protocol** (`for x in EXPR { body }`): EXPR が `..` / `to` を含まない場合 parser が `while + match Option::Some(x)/None` に desugar、`fn next(&mut self) -> Option<T>` を持つ任意の struct で動作 (structural / duck-typed; generic trait `trait Iterator<T>` 自体は未対応のため `core/std/iter.t` は documentation-only)。Range-based for-loop (`0..N` / `0 to N`) は既存の整数 fast path を維持。EXPR が bare identifier の場合 desugar は synthetic temporary を skip して `iter.next()` を直接呼び (`&mut self` writeback で user binding が正しく mutate される)。**backend coverage**: interpreter + cranelift JIT + AOT 完全対応 (3-way `assert_consistent` で pin)
  - **iterator アダプタ (STDLIB-ITER-ADAPT, `core/std/collections/vec.t`)**: `VecIter<T>` (`v.iter()`) に `map(fn (T) -> U)` / `filter(fn (T) -> bool)` / `enumerate` / `zip(other: VecIter<U>)` / `collect` (3 バックエンド)。アダプタは普通の `next(&mut self) -> Option<T>` struct で for ループにそのまま渡せる。`collect` は **by-value self** (`self: Self`) でレジスタ制約を回避し、呼び出し側のイテレータは alias のまま (2 回目は最初から)。タプル要素の Vec を産む enumerate / zip の collect は AOT の `__builtin_sizeof` 制限で提供しない。zip は stride を `elems` に 32bit ずつパックして 5 フィールド (writeback レジスタ上限) — 例: `interpreter/example/std_iter_adapt.t`
  - **iterator アダプタ (Dict / String 版)**: `DictIter<K, V>` に `map` / `filter` (`core/std/dict.t`)、`StringIter` に `map` / `filter` / `enumerate` / `collect` (`core/std/string.t`)。Dict 版は AOT 制約で (1) closure は `fn (K, V) -> U` と **k, v を別スカラー引数で** (タプル引数 closure は AOT 不可)、(2) iterator state をフラットに持ち `count` を `index` の上位 32bit にパックして 8 レジスタ上限に収める。例: `std_iter_adapt_dict.t` / `std_iter_adapt_string.t`
- **String interpolation** (`"hello {name}, sum={a + b}"`): lexer が `{...}` を検出して `Kind::InterpolatedString(parts)` を発行、parser-level で `.concat() + __builtin_to_string()` chain に desugar。`{{` / `}}` で literal `{` / `}` (Rust 規約)。任意型 (i64/u64/f64/bool/str + struct/enum) を補間可能。**backend coverage**: interpreter + AOT + cranelift JIT + interpreter JIT 完全対応 (3-way `assert_consistent` で AOT/JIT/interpreter pin、interpreter JIT は `string_interpolation_jit.t` で別途 pin)。AOT / compiler JIT は **同一ソース**の `toylang_rt` crate (`compiler/runtime/toylang_rt/`) の `toy_str_concat` / `toy_to_string_<ty>` ランタイムヘルパを共有 (JIT は出力シンクを差し替えるだけ。旧 `compiler/src/jit.rs` ミラーは RUNTIME_PORT R1 で削除)。interpreter JIT は `ScalarTy::Str` (i64 ポインタ) + `jit_str_concat` / `jit_to_string_*` / `jit_print_str` ランタイムヘルパ (`interpreter/src/jit/runtime.rs`) で同形 layout を実装、ただし str は **function 境界 (param/return) は禁止** (Object lifecycle 整合性のため)。同時に既存の `BuiltinMethod::StrConcat/Substring/Trim/ToUpper/ToLower/Contains/Split` が型 checker で Unit を返していたバグも修正
- **`else if` 構文は未サポート**: `if expr {} else if expr {}` はパーサが拒否する (「`else if` is not supported; write `elif` instead」)。代わりに `elif` キーワードを使用すること
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
- **`test "name" { ... }` ブロック**: トップレベルに書けるテスト。`test` は contextual keyword なので `fn test(...)` や `val test = ...` は従来どおり使える。各ブロックは内部でゼロ引数関数に lower されるため型検査・バックエンドは特別扱い不要。通常実行では呼ばれず、`--test` で実行する。テストごとに独立した評価コンテキストを持つ
- **`assert_eq(a, b)` / `assert_ne(a, b)` ビルトイン**: 失敗時に **left / right の実値**と行番号を出す。パーサマクロで一時束縛 + 比較 + メッセージ組み立てに desugar される
- **`assert(cond, "msg")` ビルトイン**: `cond` が false のときだけ `panic(msg)` する糖衣。`(bool, str) -> ()`。message は false 時にのみ評価される。JIT は `brif cond, cont, fail; fail: call jit_panic; trap` で lower（success path はオーバヘッド最小、failure path は panic と同じ helper）
- **`?` (Try) 演算子**: postfix early-return。`expr?` は inner の型に応じて `Result<T, E>` か `Option<T>` の match に desugar し、success arm では unwrap 値を返し、error arm では enclosing 関数から `return` で伝播する。Parser が `Expr::Try { inner, .. }` を emit、type checker が in-place で `Block { val __try_t = inner; match __try_t { Ok(__try_v) => __try_v as T, Err(__try_e) => { return __try_t; panic("?-unreachable") } } }` に rewrite。backend (interpreter / AOT / JIT) は rewritten Match のみを観測。**制約**: inner は `Result` か `Option` 以外不可、AOT は `match` scrutinee 等の MVP 制約を継承 (function-call enum scrutinee は val-bind 経由)
- **実行時例外 (try/catch/throw) は導入しない**: 言語仕様として例外機構を持たない。回復不能な失敗は `panic("...")` で即時停止 (process exit)、回復可能な失敗は `enum Result<T, E>` / `enum Option<T>` を戻り値で返して呼び出し側で `match` する。例外用の予約語 (`try` / `catch` / `throw` / `finally`) は parser で受理しない。`requires` / `ensures` 違反も `panic` 経路で停止する (例外として伝播しない)
- **間接化なしの再帰型は不可** (`[E0013]`): 自分を by-value で含む struct / enum は有限な layout を持てないので型検査で拒否 (`struct Node { next: Node }` / `enum List { Cons(i64, List), Nil }`)。**型引数が containment になるのは渡し先がそのパラメータを by-value で持つときだけ**なので `struct Tree { kids: Vec<Tree> }` は OK、`struct Held { w: Wrapper<Held> }` (`Wrapper<T> { v: T }`) は NG。cycle を切るのは `ptr` / 関数型 / `dyn Trait` の位置で、`&T` は lowering で消えるので切れない。書き方は `Box<T>` (`core/std/box.t`)、arena + index、raw `ptr` の 3 通り (`interpreter/example/box_linked_list.t` / `linked_list_arena.t` / `linked_list_ptr.t`)
- **所有権の移動** (`[E0014]`): `impl Drop` を持つ型の値を「今のスコープより長生きする場所」(値渡し引数 / struct・tuple・array・enum payload の要素 / 代入右辺) に置くと所有権が移り、以後その名前を読むとエラー。所有は**推移的** (`Vec<Box<i64>>` / payload に Box を持つ enum も対象)。`&T` / `&mut T` 引数は borrow、`val b = a` は**別名で移動ではない** (compound は alias)。分岐・ループ本体からの移動は drop flag が要るので拒否。**移動先は drop glue が再帰的に解放する** (DROP-GLUE): コンテナの死とともに要素 / フィールド / payload / Box の中身が free され、`--profile=mem` の `leaks` は 0 になる。free は全バックエンドで冪等 (never-reuse bump ヒープ)。
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
  - trait 本体には method の シグネチャを書く。`requires` / `ensures` 節も書ける
  - `impl <Trait> for <Type> { ... }` は body 付き method を提供。型チェッカーが trait のシグネチャと比較し、不足 method や型不一致を検出
  - 型パラメータ bound `<T: SomeTrait>` を関数・struct・impl に書ける。呼び出し時に「実型がその trait を実装しているか」を検証
  - 実装メソッドは inherent method としても登録されるので `value.trait_method()` 形式で直接呼べる
  - **trait ジェネリクス** — `trait Foo<T, U>` 宣言と `impl Foo<i64, str> for Counter`。trait 側の型パラメータは impl の `trait_type_args` で置換してから conformance 比較される
  - **default method body** — trait シグネチャに `{ ... }` 本体を書くと、その method を omit した impl が inherent method として継承する。impl 側で同名 method を書けば override。3 backend で動作
  - **多重 trait bound `<T: A + B>`** — call-site の bound check は AND (全 trait 必須)、method dispatch は OR (最初に持つ trait を採用)。3 backend で動作
  - **`dyn Trait`** — `&dyn TraitName` による動的ディスパッチ。interpreter / AOT / compiler 側 JIT で動作 (empty struct / scalar field / nested field、`&mut dyn` writeback、struct / tuple / enum return の全組合せ)。interpreter 側 JIT は silent fallback。fat pointer ABI と vtable の設計は [`design-docs/DYN_TRAIT_AOT.md`](design-docs/DYN_TRAIT_AOT.md)
  - **未対応**: trait 継承 (A3)、associated types (A4)、interpreter 側 JIT の `dyn Trait` 対応 (compiler 側 JIT は対応済み)、`Box<dyn Trait>`、generic trait body 内での T 参照
- **クロージャ / ラムダ** — `fn(params) -> R { body }` の anonymous function literal。関数型は `fn (T1, T2) -> R` (推奨) / `(T1, T2) -> R` (bare)。parameter / return / `val` 注釈 / struct field 型に書ける。capture は生成時スナップショット。interpreter は full support、AOT は env-based ABI で capturing / non-capturing 両対応、JIT は silent fallback。詳細は [`docs/language.md`](docs/language.md)
- **Design by Contract キーワード**: `requires`（事前条件）, `ensures`（事後条件）。関数 / メソッドの `-> ReturnType` の後、body `{` の前に複数並べられる。各節は bool 式で、AND 合成。`ensures` 内では `result` が戻り値を指す。違反時は `ContractViolation` エラーで停止。`INTERPRETER_CONTRACTS=all|pre|post|off`（unset = `all`）で `requires` / `ensures` を独立に切り替えられる（D の `-release` 相当）
- **可視性・外部連携**: `pub`（公開）, `extern`（外部関数）
- **モジュールシステム**: `package`, `import`, `as`
- **演算子**:
  - 算術: `+`, `-`, `*`, `/`, `%`（剰余・truncated remainder で `(-7) % 3 == -1`）
  - 比較: `==`, `!=`, `<`, `<=`, `>`, `>=`。`==` / `!=` は同型 struct ペアで **operator overload** — その struct に `eq(&self, other: &Self) -> bool` method があれば dispatch (3 backend)。`s == t` で String/Vec<u8> 等の比較が動く
  - **全 binary / unary operator overload** (Phase B + OP-OVERLOAD-ARITH + OP-OVERLOAD-EXTEND Phase 1-4): 同型 struct ペアで以下に dispatch (3 backend、let-rhs context):
    - 算術: `+` / `-` / `*` / `/` / `%` → `add` / `sub` / `mul` / `div` / `rem` (`(&self, &Self) -> Self`)
    - 複合代入: `+=` / `-=` / `*=` / `/=` / `%=` (parser desugar `a OP= b → a = a OP b` で算術 method 経由、AOT は `assign.rs` で既存 binding leaf locals に上書き)
    - 順序比較: `<` / `<=` / `>` / `>=` → `lt` / `le` / `gt` / `ge` (`(&self, &Self) -> bool`)
    - ビット: `&` / `|` / `^` / `<<` / `>>` → `bitand` / `bitor` / `bitxor` / `shl` / `shr` (Self 戻り)
    - 単項: `-` / `~` / `!` → `neg` / `bitnot` / `not` (`(&self) -> Self`)
    - **scope 外**: `&&` / `||` (short-circuit semantics)、chain (`a + b + c`)、binary struct literal operand (`a & Foo { ... }`)
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

プロジェクトの改善タスクは `design-docs/todo.md` で管理されています。Claude Codeは以下のワークフローに従ってください：

### タスク管理プロセス
1. **TodoRead/TodoWrite ツールの使用**: セッション中の一時的なタスク追跡に使用
2. **design-docs/todo.md ファイルの更新**: 永続的な記録として、完了したタスクや新しい課題をファイルに反映
3. **定期的な同期**: Todoツールと `design-docs/todo.md` の内容を定期的に同期

### ファイル構造
- `design-docs/todo.md`: マスタータスクリスト。**完了済み / 未実装 /
  検討中の機能 / 現状の把握** の 4 節
  - **完了済み節は 1 行サマリだけ**。経緯・測定値・パス・テスト数は
    git log のコミットメッセージに書く。ここを段落で埋めると、
    常時読まれるファイルが changelog になる (2026-08-10 に 90 KB →
    12 KB に圧縮した。それ以前は `todo.md` を読もうとして出力上限に
    到達していた)
  - **未実装節に完了項目を残さない** — 完了済み節と二重になる
  - **機能一覧を再掲しない** — 言語仕様は `docs/language.md` が正本。
    再掲した節は実際に食い違った (`String` を alias と書き続けていた)

## ビルトイン関数
- ビルトイン関数の実装方針は `design-docs/BUILTIN_ARCHITECTURE.md` に記述されています
- **メモリ確保カウンタ** (`__builtin_live_bytes()` 等 6 種、`() -> u64`) を
  `requires` / `ensures` / `test` から読める。名前と意味は `--profile=mem`
  のレポートと同一、run 単位で 0 から。プロファイルフラグ不要。
  詳細は [`docs/language.md`](docs/language.md) の「Allocation counters」

> **`BuiltinFunctionSymbols::new` に名前を足したら
> `FULL_AST_CACHE_SCHEMA_VERSION` を上げること。** `.toycache` は
> `DefaultSymbol` を保存し、キーはソースのハッシュだけなので、
> intern 順が変わると古いエントリが別の意味に化ける (無関係な
> `val a: u64 = 5u64` が stdlib 由来の型エラーで落ちる)。

## Cranelift JIT

`interpreter` には数値 / bool 関数を cranelift で native code 化するオプトインの JIT が入っている。`INTERPRETER_JIT=1` で有効化、cargo feature `jit` (default on) でビルド時にも切替可。サポート範囲・性能・skip 理由・拡張ロードマップは `design-docs/JIT.md` を参照。

## 入出力ビルトイン

- **`io::` モジュール** (`core/std/io.t`) — `read_line()` / `argc()` /
  `arg(i)` / `env_var(name)` / `read_file(path)` / `file_exists(path)` /
  `now()` / `random()` / `random_seed(seed)` / `strftime(fmt, secs)` /
  `env_count()` / `env_name(i)` / `env_value(i)`。`extern fn` 宣言 +
  バックエンド別実装 (interpreter: `extern_io::build_io_registry`、
  AOT/JIT: `toylang_rt` の `toy_io_*` シンボル)。失敗は `""` 返し +
  `file_exists` プローブ (`Result` は extern 境界が compound return を
  運べないため不可)。`random()` は非決定的だが、`random_seed(s)` で
  再現可能になる (0 も literal に保持、シーケンスは 3 バックエンド一致)。
  `strftime(fmt, secs)` は C `strftime` の文書化された部分集合で
  **UTC 固定** (ローカル時刻にしない、`docs/language.md` の
  Output 節相当の決定性規約)。環境変数一覧は `env_count` +
  `env_name` / `env_value` の `environ` 順アクセス (interpreter の
  `std::env::vars` も同じ順)。プログラム引数は CLI ではファイル後ろの
  引数、`RunOptions.args` で注入。
- `print(value)` — stdout に値を出力（改行なし）
- `println(value)` — stdout に値を出力 + 改行
- 任意の型を受け取り、`Object::to_display_string` で整形。文字列は引用符なし、構造体 / dict はフィールド名順にソートして決定的な出力
- ユーザ向けの日常的な I/O なので、`heap_alloc` 等の低レベル builtin と違って `__builtin_` prefix は付けない
- **`Display`**: `fn to_str(&self) -> str` を持つ型は `print` / `println` /
  文字列補間 `"{v}"` の出方を自分で決める (`core/std/display.t`)。
  型検査器が `println(v)` → `println(v.to_str())` に書き換えるので
  **バックエンドは通常の method 呼び出ししか見ない**。ディスパッチは
  method の有無で決まる (`==` → `eq` と同じ流儀) ので inherent method でも動く。
  詳細は [`docs/language.md`](docs/language.md) の「`Display`」
- 使用例: `interpreter/example/print_demo.t` / `interpreter/example/display_trait.t`

## Allocator システム

`with allocator = ...` による lexical scope で allocator を切り替えられる。heap 系 builtin は常に現在の allocator を経由する。詳細な設計と進捗は `design-docs/ALLOCATOR_PLAN.md` を参照。

### 主要な構文・ビルトイン

| 要素 | 説明 |
|---|---|
| `with allocator = <expr> { body }` | スコープ内で allocator を差し替え |
| `ambient` | 現在の allocator（式として使える糖衣） |
| `__builtin_current_allocator()` | 現在の allocator（スタック top） |
| `__builtin_default_allocator()` | プロセス全体の global allocator |
| `__builtin_sizeof(value)` | 値のバイトサイズ（u64）。primitive に加え struct（フィールド合計）/ tuple / array（要素合計）/ **enum（u64 タグ + 全 variant の payload 連結）** をサポート。enum のサイズは**型の性質で、手元の variant に依存しない** (`Vec<T>` の stride がこれ)。generic `T` の実体サイズ取得に使う |
| `__builtin_ptr_eq(a: ptr, b: ptr) -> bool` | 2 ポインタの addr 等値比較。stdlib `Arena` / `FixedBuffer` の追跡表検索に使用 |
| `__builtin_null_ptr() -> ptr` | null pointer (addr 0)。`__builtin_heap_alloc(0u64)` は AOT で libc malloc に委譲するため非 null を返しうる; 移植性のあるコードは本 builtin を使う |
| `with allocator = a { ... }` | scope 内で allocator を有効化、内部の `__builtin_heap_alloc` 等が経由する |

### stdlib `Arena` / `FixedBuffer` の introspection (Odin/Zig 風)

`core/std/allocator.t` の wrapper struct に名前束縛 (`val arena = Arena::new()`) でアクセスすると、以下の inherent method が使える:

- `arena.alloc(size: u64) -> ptr` / `arena.free(p: ptr)` / `arena.realloc(p: ptr, n: u64) -> ptr` (`trait Alloc`)
- `arena.bytes_used() -> u64` — **live** な追跡バイト数 (reset までは累積と一致するが、意味は live。用語は `design-docs/MEMORY_PROFILING.md` M0 で固定)
- `arena.reset()` — 一括 free + 再利用可能化 (Odin の `mem.free_all` 相当)
- `fb.capacity() -> u64` / `fb.used() -> u64` / `fb.remaining() -> u64` / `fb.is_empty() -> bool`
- `fb.reset()` — quota を 0 に戻す

これらは toylang 側の `(addr, size)` parallel array を見るので、`arena.alloc(...)` 経由で確保した分のみカウントされる。`with allocator = arena { __builtin_heap_alloc(...) }` 形式で raw heap_alloc を呼ぶと wrapper の追跡を通らないため、introspection は反映されない (`_h: Allocator` は default allocator を指しているので、heap_alloc は default 経由になる)。

### 典型的な使い方

- 基本: `interpreter/example/allocator_basic.t`
- bound 付き汎用関数 + `ambient` + 自動挿入: `interpreter/example/allocator_bounded.t`
- ユーザ空間の動的リスト（struct + impl + heap builtin）: `interpreter/example/allocator_list.t`
- 新 introspection API (`bytes_used` / `reset` / `used` / `remaining` / `is_empty`): `interpreter/example/allocator_reuse.t`

### 意味論のポイント

- `with` は lexical scope。ネストは push/pop、body の exit path（値・return・break・error）すべてで必ず pop される
- `Allocator` 値は `Rc::ptr_eq` で同値性を判定。`==` / `!=` のみサポート（順序比較は不可）
- 関数の引数として `Allocator` を渡す形は推奨しない (関数は `with allocator = ...` の active stack を経由して暗黙的に allocator を使う)
- arena は個別 `free` を no-op とし、`Drop` で一括解放。fixed_buffer は quota 超過で `0`（null ポインタ）を返す。両者の policy はすべて toylang stdlib (`core/std/allocator.t`) に実装され、底に default allocator が居る
- `List<T>` のようなコレクションは言語組み込みではなく、`struct` + `impl` + `__builtin_heap_alloc/realloc/ptr_read/ptr_write` で書く。これらの builtin は現在の active allocator を経由する
- `__builtin_ptr_write(p, off, value)` は任意型の値を受け取り、`__builtin_ptr_read(p, off)` は呼び出し側の型ヒント（`val v: T = ...` など）に沿って値を返す。内部的には typed-slot map に値を保存しているため、`List<i64>` / `List<bool>` / `List<MyStruct>` もそのまま動作する。**読み出しの型注釈は必須** (それが唯一の shape の情報源)。generic param (`T`) / primitive に加えて **user 定義の struct / tuple / enum 名**も書ける (3 backend)

### 進捗

- 構文・ランタイム・`GlobalAllocator` 完了 (旧 runtime `ArenaAllocator` / `FixedBufferAllocator` は撤去、stdlib 実装に置換)
- `ambient` 糖衣、`with allocator = ...` 経由の active stack dispatch 完了
- stdlib (`core/std/allocator.t`) に `trait Alloc` + Wrapper 構造体 (`Global` / `Arena` / `FixedBuffer`) 完了
- AOT native codegen 完了 (#121 Phase A / B-min / B-rest Items 1+3 + Item 2 cleanup + arena_drop)

## テスト計画

toylang コンパイラ開発の包括的なテスト戦略と計画は `design-docs/TEST_PLAN.md` に記述されています。このドキュメントでは以下の内容を扱っています：

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

上の「Commands → Testing」を参照。要点だけ再掲:

```bash
cargo nextest run                        # 全テスト (グリーンなら 7 行)
cargo nextest run -p interpreter proptest # 絞り込み
```

### 将来のテスト計画
- Enum とパターンマッチングのテスト
- Option 型と Null 安全性のテスト
- 動的配列と高度な型システムのテスト
- パフォーマンス最適化の検証

詳細なテスト要件、戦略、実装チェックリストについては `design-docs/TEST_PLAN.md` を参照してください。

### Frontend テスト整理計画
frontend コンポーネント内のテストコード整理計画は `design-docs/FRONTEND_TEST_PLAN.md` に記述されています。この計画には以下の内容が含まれます：

- **現在の構成分析**: ユニットテストと統合テストの配置状況
- **テスト統合戦略**: 統合テストファイルの論理的な再構成
- **カテゴリ別再構成**: 型システム、ジェネリック、配列・コレクション、モジュール、エラーハンドリング
- **実装ロードマップ**: 段階的な改善計画
- **品質指標**: テストカバレッジ、ドキュメンテーション基準

詳細な実装方法とロードマップについては `design-docs/FRONTEND_TEST_PLAN.md` を参照してください。

### Interpreter テスト整理計画
interpreter コンポーネント内のテストコード整理計画は `design-docs/INTERPRETER_TEST_PLAN.md` に記述されています。この計画には以下の内容が含まれます：

- **現在の構成分析**: 296個のテストが35ファイルに分散している状況の分析
- **カテゴリ別分類**: コア言語、ジェネリック、コレクション、OOP、メモリ管理など9カテゴリ
- **統合戦略**: 35ファイルを7つの論理的な統合テストファイルに再構成
- **フェーズ別実装計画**: 4-5週間の段階的な改善プロセス
- **品質目標**: テストカバレッジ、ドキュメンテーション、保守性の向上

詳細な実装方法と段階別ロードマップについては `design-docs/INTERPRETER_TEST_PLAN.md` を参照してください。

### 重要な原則
- 新しい課題を発見した場合は、TodoWriteツールと `design-docs/todo.md` の両方に追加
- タスク完了時は、`design-docs/todo.md` の該当項目を「完了済み」セクションに移動
- 大きな改善や機能追加後は、`design-docs/todo.md` ファイルをgitにコミット
