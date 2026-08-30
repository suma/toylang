# CLAUDE.md
以下日本語のみで書いてください。ただし、コード内のコメント、gitコミットメッセージ、
および `docs/language.md` (言語仕様の正本) は英語で記述してください。

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## どこを見ればいいか

本ファイルは**作業中に必要な運用ガイダンス**だけを置く (ビルド・テスト
コマンド、横断的な変更の注意、タスク管理ワークフロー)。それ以外は移した。

| 知りたいこと | 見る場所 |
|---|---|
| **「name resolution はどこか」** — 関心事 → 実装サイト | [`design-docs/CODE_MAP.md`](design-docs/CODE_MAP.md) |
| 構文・型・セマンティクスの**正本** | [`docs/language.md`](docs/language.md) |
| `requires` / `ensures` の**書き方・検証・落とし穴** | [`docs/design_by_contract.md`](docs/design_by_contract.md) |
| いつ何が landing したか / 未実装項目 | [`design-docs/todo.md`](design-docs/todo.md) |
| 機能ごとの実装詳細・フェーズ履歴 | [`design-docs/FEATURE_NOTES.md`](design-docs/FEATURE_NOTES.md) |
| LLM 向けの診断・テスト機能の設計 | [`design-docs/LLM_FEEDBACK_LOOP.md`](design-docs/LLM_FEEDBACK_LOOP.md) |
| backtrace / 行番号 / ファイル名の設計 | [`design-docs/DEBUG_OBSERVABILITY.md`](design-docs/DEBUG_OBSERVABILITY.md) |
| `const fn` / コンパイル時実行の設計 | [`design-docs/COMPILE_TIME_EVAL.md`](design-docs/COMPILE_TIME_EVAL.md) |
| closure が捕捉した束縛をどう掴むかの設計 | [`design-docs/CLOSURE_CAPTURE.md`](design-docs/CLOSURE_CAPTURE.md) |
| エフェクト格子と 3 検査の関係 | [`design-docs/EFFECT_SYSTEM.md`](design-docs/EFFECT_SYSTEM.md) |
| allocator のリージョン脱出検査 | [`design-docs/REGIONS.md`](design-docs/REGIONS.md) |
| RUNTIME-TRAP guard をどう消しているか | [`design-docs/GUARD_ELISION.md`](design-docs/GUARD_ELISION.md) |
| 配列 / Vec の layout (AoS / SoA) の設計 (Phase 0・2 landing 済み) | [`design-docs/DATA_ORIENTED.md`](design-docs/DATA_ORIENTED.md) |
| `ptr` を型付きにする設計 (未実装) | [`design-docs/POINTER.md`](design-docs/POINTER.md) |
| SIMD の設計と残りのフェーズ | [`design-docs/SIMD.md`](design-docs/SIMD.md) |
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

# 各宣言が計算以外に何をしうるか (EFFECT-SYSTEM)。`pure` / `alloc, io` 等。
# --api と違い型検査を通すので、実行できるプログラムを渡す
cargo run -q -p interpreter -- --effects prog.t

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

**`--diagnostics=json`** で診断を stderr に JSON 配列で出す
(各要素は `severity` / `code` / `message` / `span` (`line`・`column`・
`offset`・`end_offset`) / `suggestions` を持つ)。既定は
`--diagnostics=text` (スニペット付き、1 エラーあたり ~11 行)。位置情報は
JSON でも保持されるので、機械的に読む場面 (LLM ループ) ではこちらを使う。

### Testing

**`cargo nextest` を使う。**

```bash
# ワークスペース全体
cargo nextest run

# パッケージ / テスト名で絞る (以下はすべて実際に当たることを確認済み)
cargo nextest run -p compiler
cargo nextest run -p interpreter property
cargo nextest run -E 'test(basic_arithmetic)'

# 旧「1 ファイル = 1 バイナリ」時代のファイル単位で走らせる。
# テスト名の先頭がファイル名になっているので prefix で絞れる
cargo nextest run -E 'test(/^language_core_tests::/)'

# 失敗の詳細だけでなく全テストの一覧が欲しいとき
cargo nextest run --profile verbose
```

**フィルタは部分一致で書くこと。** テスト名は
`language_core_tests::basic_execution::test_f64_basic_arithmetic` のように
**ファイル名 + モジュール名**で修飾されているので、`test(=basic_arithmetic)`
のような**完全一致 (`=`) は当たらない** — しかも 0 件でも
`no tests to run` と出るだけでフィルタの綴り間違いと区別がつかない。
まず `cargo nextest list | grep ...` で名前を確認するのが早い。

**出力は失敗のみが既定** (`.config/nextest.toml`)。グリーンな全体実行は
**6〜7 行**で終わる (この設定を入れる前は 1641 行だった)。実行自体は ~7.5 秒なので、
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

**テストファイルを新設したら `tests/suite.rs` に登録すること。** 各クレートは
`autotests = false` で **1 クレート 1 テストバイナリ**にまとめてある
(`tests/suite.rs` が `#[path]` で各ファイルをモジュールとして取り込む)。
そのため `tests/foo_tests.rs` を置いただけでは **cargo が拾わず、テストは
黙って実行されない** — 失敗ではなく無音なので気づきにくい。suite.rs に

```rust
#[path = "foo_tests.rs"]
mod foo_tests;
```

を足す。この構成の理由と実測値は
[`design-docs/todo.md`](design-docs/todo.md) の BUILD-PERF にある
(テストバイナリ 73 本 → 12 本、クリーンビルド 37.5s → 23.7s)。
interpreter のテストは共有ヘルパを `use crate::common::...` で参照する
(以前の `mod common;` は suite.rs 側に 1 つだけある)。

**`target/` を肥大させない。** 過去の成果物が溜まると
`-L dependency=target/debug/deps` の走査だけでビルドが桁で遅くなる
(125 万ファイルまで育ったとき、lib 1 行の変更が 2.35s → 1m55s だった)。

```bash
# 世代 GC (古い成果物を残して今のビルドを保つ)。定期実行の運用に
cargo sweep --time 30
# 全削除 (フルリビルドを覚悟するときだけ)
cargo clean
```

cargo-sweep は `cargo install cargo-sweep` で入る (`cargo sweep --dry-run`
で削除対象を確認してから)。--time は「この日数より新しい成果物は残す」。

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

- 意味論を変える修正には `compiler/tests/consistency/` の
  `assert_consistent` を使ったテストを必ず追加する。ここは
  **機能別のモジュール群**なので、近い機能のファイルに足す
  (`harness.rs` が全レーンと共有ヘルパ、`mod.rs` が目次)
- **「独立」なのは tree-walker だけ。** `interpreter` の既定エンジンは
  **IR VM** で、これは AOT / JIT と同じ `compiler_lower` を通る。つまり
  `execute_program` を呼ぶ「インタプリタ」レーンは、たいていのプログラムで
  lowering をもう一度走らせているだけになる。**オラクルが要る場面では
  `execute_program_tree_walking` を使うこと** (consistency harness は
  これを使う)。2026-08-25 の MATCH-STRUCT-ARM はこの取り違えのせいで
  4 レーン全一致のまま誤答していた
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
- Types: `u64`, `i64`, `f64`, `f32`, `bool`, `str`, `ptr`, `usize`, `dict`, `Self`,
  SIMD vector (`f64x2` / `f32x4` / `i32x4` / `i64x2` / `u8x16`)
  (`null` は**予約済みで型検査が拒否する** — `[E0015]`。不在は `Option<T>`、生ポインタは `__builtin_null_ptr()`)
- Narrow ints (NUM-W): `u8` / `u16` / `u32` / `i8` / `i16` / `i32` (literal suffix `42u8` / `0xFFi32` 等)。`as` cast で wide ↔ narrow 変換 (暗黙 widening は無し)
- **`soa [T; N]` (DOD Phase 0)**: 配列型の前置修飾子で **SoA (列ごと配置) を選ぶ**。**same-type** — `soa [P; N]` と `[P; N]` は同じ型 (付け外して計測できる)。`ps[i].f` 読み書き・`val p = ps[i]`・range slice は AoS と同書式 (`ps[i] = p` の compound 一括書き込みは不可)。`soa` は contextual keyword (`val soa = 5u64` は従来どおり)。**`ps[i].f` は compiled lane では SoA 以前に未対応だった** ので、AoS 配列でも `arr[i].field` が新規に動く。実装は列方式 (leaf ごとに slot) で IR / codegen / IR VM 無変更。例: `interpreter/example/soa.t`。heap 版は `soa Vec<T>` (下の `SoaVec<T>`)
- **SIMD vector (SIMD)**: 128bit の vector 型 5 種 (`f64x2` / `f32x4` /
  `i32x4` / `i64x2` / `u8x16`)。**通常の演算子が lane-wise に効く**ので
  intrinsic は型と演算子で表せない 13 個だけ (`__simd_splat` /
  `__simd_load` / `__simd_store` / `__simd_extract` / `__simd_insert` /
  `__simd_select` / `__simd_reduce_add|min|max|and|or` / `__simd_any` /
  `__simd_all`)。128bit に限るのは SSE2 (x86-64) と NEON (aarch64) が
  無条件に持つから (ホスト依存ゼロ)。要点:
  - **整数 lane は wrap し trap しない** — scalar の `u64 -` / `/` は
    panic するが (RUNTIME-TRAP)、lane ごとの guard はベクトル化の意味を
    消すので**整数の `/` `%` は型エラー**。float の `/` は IEEE なので可
  - **比較は mask を返す** (bool ではない) — lane 幅と同じ整数 vector で
    全 1 / 全 0。`f64x2` の mask は `i64x2`。全体の等値は `__simd_all(a == b)`
  - **`__simd_reduce_*` は lane 0 → n の逐次畳み込み** (仕様。pairwise
    tree にすると f64 の答えがバックエンド間で割れる)
  - **`__simd_load(p, i)` の `i` は要素 index** (lane k は
    `(i + k) * lane_bytes`)。`__builtin_ptr_read` の offset が**バイト**
    なのと対照的
  - `<<` / `>>` の右辺は **`u64` のスカラー** (全 lane 同じ量)
  - `__simd_splat` / `__simd_load` は**サフィックス無し**なので型は文脈
    (注釈 / 演算子の相手) から取る。型検査器が call に焼き込むので
    バックエンドは引数から読む
  - 3 backend 対応 (interpreter JIT は silent fallback)。例:
    `interpreter/example/simd.t`
- **`f32` (SIMD-F32)**: 単精度 float (SIMD の `f32x4` 前提、論点 1 解決)。literal suffix `1.5f32` / `42f32`。算術・比較・単項 `-` は IEEE 754 単精度で f64 と同じ trap 無し。**暗黙 widening は無し** — f32 ↔ f64 / 整数は `as` で明示 (`f64 → f32` は demote、`f32 → int` は f64 同様の saturating)。`__builtin_sizeof(f32値) == 4`。print / 補間は f64 と同じ「整数値に `.0`」規約の単精度版。**format spec (`{x:.2}`) は f32 未対応**。3 バックエンド対応 (interpreter JIT は silent fallback)。例: `interpreter/example/float32.t`
- Stdlib types:
  - `char = u32` (Unicode codepoint alias)。**char リテラル (CHAR-LITERAL-NUM)**:
    `'a'` / `'\n'` / `'\x41'` / `'\u{1F600}'` は **32bit (u32) で保持**
    (`val c = 'a'` は u32、`__builtin_sizeof(c) == 4`)。ただし
    **他の整数型を名指しする位置では、値が収まるならその型を取る** —
    `val b: u8 = '0'` / `s.get(i) == 'h'` / `c - '0'` / `take_i64('\n')`。
    NUM-W の「整数型は暗黙変換しない」規則の**唯一の例外**で、
    対象は**文字として書かれたリテラルだけ** (`42u32` は従来どおり
    `as` が要る — サフィックスが既に型を名乗っているため)。
    収まらなければ範囲エラー (`'\u{1F600}'` → u8 は不可)。
    **stdlib の使い分け**: バイト単位のアクセス / イテレーションは `u8`
    (`String::get` / `String::iter`)、文字単位の API は `u32`
    (`push_char(c: char)`)。実装は型検査器が節点を書き換える方式
    (`coerce_char_literal`) で、**stdlib の body も検査対象**なので
    `core/std/parse.t` の `c < '0' || c > '9'` のような書き方が
    stdlib 内でも効く
  - `String` (`core/std/string.t`) — heap-managed byte buffer の **nominal struct** (`type` alias ではなく独立 struct、`Vec<u8>` と同 memory layout だが nominal identity は別)。inherent method (`new` / `from_str(s)` / `push` / `pop` / `get` / `set` / `size` / `len` / `as_ptr` / `capacity` / `is_empty` / `clear` / `extend_bytes` / `push_str` / `push_char` / `eq` / `to_string`) + 拡張 trait impl (`Substring` / `Trim` / `CaseConvert` / `Concat<String>` / `Contains<String>` / `Split<String, Vec<String>>` from `core/std/str_ops.t`) で `s.len()` / `s.substring(...)` / `s.trim()` / `s.concat(other)` / `s.split(sep)` 等が `str` と同じ call shape で動く (3 backend)。
  - **`SoaVec<T>` (`core/std/collections/soa_vec.t`, DOD Phase 2)** — `Vec<T>` と同じ call surface (`push` / `pop` / `get` / `set` / `size` / `capacity` / `is_empty` / `clear` / `iter`) で、**1 確保を leaf ごとの列に区切る**動的配列。`soa Vec<T>` と書くと parser が `SoaVec<T>` に書き換える (砂糖は checker より前で消える)。stack の `soa [T; N]` と違い **`Vec<T>` とは別型** (heap は layout が観測可能なので付け外しは注釈 + コンストラクタの 2 箇所)。番地は `__builtin_soa_read` / `__builtin_soa_write` が `prefix_j * cap + i * stride_j` で作り、既存の `PtrRead` / `PtrWrite` に展開されるので IR / codegen / IR VM は SoA を知らない。確保総量は `Vec` と一致 (列 stride = leaf 実幅)、drop glue は列を歩く。例: `interpreter/example/soa_vec.t`
  - `Ptr<T>` (`core/std/ptr.t`, POINTER P3+P5) — **型付きポインタ窓**。`T` は field に現れず `addr: ptr` の背後にだけ居るので backend 特殊扱いゼロ (`Box<T>` と同じ手口)。`alloc(count)` (stride は `__builtin_sizeof::<T>()`、**0 要素でも 1 バイト確保して非 null を保証**) / `get` / `set` / `p[i]` / `p[i] = v` (`__getitem__` / `__setitem__`) / `offset(count)` / `as_raw()`。**window であって owner ではない** — free は呼び出し側 (`__builtin_heap_free(p.as_raw())`)、index は unchecked。**non-null 不変** (P5) — 不在は `Option<Ptr<T>>` で表す (`has_next: bool` 方式が消える、16 バイト・niche 最適化は不可)。不変は構成による規約で compiler 強制は無し (field visibility は未強制)
  - `Span<T>` (`core/std/span.t`, POINTER P4) — **境界検査つきの窓**。`Ptr<T>` + 長さのペアで、todo の slice 型 `&[T]` をライブラリ側で回収する形。`from_parts(p, len)` / `get` / `set` / `s[i]` / `s[i] = v` (範囲外は panic、文言は `Vec` と同規約) / `len` / `is_empty` / `as_ptr` / `as_raw` (`__simd_load` の受け口)。**view であって owner ではない**。**escape は未検査** (POINTER.md の既定、選択肢 1) — 参照Rule は `&T` のみなので、`Span` は指す先より長生きできる
  - `Vec<T>` (`core/std/collections/vec.t`) — generic dynamic array。`T` が compound (struct/tuple) も AOT 対応 (`__builtin_ptr_read/write` を per-leaf 展開、`AOT-COMPOUND-PTR-RW`)。**`v.sort()` (STDLIB-ORD)** — `impl<T: Ord> Vec<T>` の安定 in-place insertion sort。`Ord` trait (`core/std/ord.t`) は `fn lt(self: Self, other: Self) -> bool` だけで、primitive 全幅 / `f64` / `bool` / `String` (byte-wise) に impl。method 名が `<` 演算子オーバーロードの `lt` と同じなので `impl Ord` は `<` も自動で得る (3 backend)。`T` が Ord でない `sort()` は **call site で型エラー** (`[E0010] ... bound violation`) — impl block の generic bound は free function と同じく呼び出し側で強制される。
- **`unsafe fn` (POINTER P6)**: **生メモリを読み書きする body は宣言が要る**
  (`[E0024]`)。対象は「指す先」に触る builtin — `__builtin_ptr_read` /
  `__builtin_ptr_write` / `mem_copy` / `mem_move` / `mem_set` /
  `str_from_bytes` / `record_allocator_layout` / `__simd_load` /
  `__simd_store`。**番地を作る・比べるだけは safe**
  (`ptr_offset` / `ptr_eq` / `ptr_is_null` / `null_ptr` / `str_to_ptr`)、
  `heap_alloc` / `heap_free` / `heap_realloc` も safe。
  **検査は直接のみ** — `unsafe fn` を呼んでも呼び出し側は safe なので、
  `Vec` / `String` / `Ptr` / `Span` を経由するコードは宣言不要
  (stdlib 側が `unsafe fn` を持つ)。修飾子は contextual で
  `never_allocates` / `const` と順不同、trait の default body にも書ける
  (omit した impl が継承)。`extern fn` は宣言としてのみ受理。
  `--explain E0024` に直し方 3 通り
- **`==` / `!=` operator overload** (Phase B) — 同型 struct ペアで `eq(&self, other: &Self) -> bool` method に dispatch (3 backend)。`s == t` で String 比較が動く。**`eq` の無い struct と enum は型検査が拒否する** (E0004、enum は `match` に誘導)。
- **`Vec<u8>::push_char(c: char)`** は **UTF-8 encoding 対応** (RFC 3629、1〜4 bytes、surrogate / U+110000+ は panic)。
- **alias-qualified associated function call** も frontend で支援 (`String::from_str("...")` / `String::new()` が直接 dispatch)。
- **Numeric literals**:
  - Type suffix: `42u64` (unsigned 64-bit), `42i64` (signed 64-bit), `1.5f64` / `42f64` (IEEE 754 double)
  - Hex literals: `0xFFu64`, `0xFFi64`, `0xFF`（型サフィックスなしも可）
  - Without suffix: **型を名指しする位置から解決**される (型注釈 / 代入先 /
    引数 / 戻り値 / closure 本体 / struct フィールド / enum payload /
    配列・tuple・dict 要素 / 演算相手 / 兄弟要素)。どの位置にも届かなかった
    ときだけ **`u64`** に倒れる (narrow int も対象、範囲外はエラー)。
    generic 位置は「名指し」ではないのでリテラル側が型引数を決める。
    決定規則の表は [`docs/language.md`](docs/language.md) の
    「How a suffix-less literal gets its type」
  - Examples: `val x = 42` → `u64` type, `val y: i64 = 42` → automatically converted to `i64`
  - **数値リテラル区切り**: `_` を桁の間に挿入できる (`1_000_000u64`、`0xDEAD_BEEFu64`、`3_141.592_653f64`)。最初の文字は数字必須 (`_42` は識別子)。lexer のみで処理、AST / IR / runtime は separator を見ない。
  - **f64 リテラルは必ず `f64` サフィックスを付ける**: タプルアクセス `outer.0.1` のような構文との曖昧性を避けるため、`1.5` 単体は許可しない。整数 → f64 への暗黙変換も無いので、`1.0f64` または `1f64` と書く（必要なら `as f64` キャスト）
- Control flow: `if/else`, `for i in start to end`, `while`, `break`, `continue`, `return`
- **Iterator protocol** (`for x in EXPR { body }`): EXPR が `..` / `to` を含まない場合 parser が `while + match Option::Some(x)/None` に desugar、`fn next(&mut self) -> Option<T>` を持つ任意の struct で動作 (structural / duck-typed; generic trait `trait Iterator<T>` 自体は未対応のため `core/std/iter.t` は documentation-only)。Range-based for-loop (`0..N` / `0 to N`) は既存の整数 fast path を維持。EXPR が bare identifier の場合 desugar は synthetic temporary を skip して `iter.next()` を直接呼び (`&mut self` writeback で user binding が正しく mutate される)。**backend coverage**: interpreter + cranelift JIT + AOT 完全対応 (3-way `assert_consistent` で pin)
  - **iterator アダプタ (STDLIB-ITER-ADAPT, `core/std/collections/vec.t`)**: `VecIter<T>` (`v.iter()`) に `map(fn (T) -> U)` / `filter(fn (T) -> bool)` / `enumerate` / `zip(other: VecIter<U>)` / `collect` (3 バックエンド)。アダプタは普通の `next(&mut self) -> Option<T>` struct で for ループにそのまま渡せる。`collect` は **by-value self** (`self: Self`) でレジスタ制約を回避し、呼び出し側のイテレータは alias のまま (2 回目は最初から)。タプル要素の Vec を産む enumerate / zip の collect は AOT の `__builtin_sizeof` 制限で提供しない。zip は stride を `elems` に 32bit ずつパックして 5 フィールド (writeback レジスタ上限) — 例: `interpreter/example/std_iter_adapt.t`
  - **iterator アダプタ (Dict / String 版)**: `DictIter<K, V>` に `map` / `filter` (`core/std/dict.t`)、`StringIter` に `map` / `filter` / `enumerate` / `collect` (`core/std/string.t`)。Dict 版は AOT 制約で (1) closure は `fn (K, V) -> U` と **k, v を別スカラー引数で** (タプル引数 closure は AOT 不可)、(2) iterator state をフラットに持ち `count` を `index` の上位 32bit にパックして 8 レジスタ上限に収める。例: `std_iter_adapt_dict.t` / `std_iter_adapt_string.t`
- **String interpolation** (`"hello {name}, sum={a + b}"`): lexer が `{...}` を検出して `Kind::InterpolatedString(parts)` を発行、parser-level で `.concat() + __builtin_to_string()` chain に desugar。`{{` / `}}` で literal `{` / `}` (Rust 規約)。任意型 (i64/u64/f64/bool/str + struct/enum) を補間可能。**format spec (STR-INTERP-FMT)**: `"{x:.2}"` / `"{n:<6}"` / `"{n:08x}"` — `[align]['0'][width]['.'precision][type]` (`< > ^` / `x X b o`)。spec は literal の一部なので **parse 時に検証 + u64 に pack** され `__builtin_format(value, <u64>)` に落ちる (不正な spec は parse エラー)。**primitive のみ** — compound は型エラーで `Display` の `to_str` に誘導。depth 0 の `:` だけが区切りなので `{Point { x: 1i64 }}` / `{Color::Red}` は誤爆しない。interpreter + AOT + compiler JIT 対応 (interpreter JIT は silent fallback)。**backend coverage**: interpreter + AOT + cranelift JIT + interpreter JIT 完全対応 (3-way `assert_consistent` で AOT/JIT/interpreter pin、interpreter JIT は `string_interpolation_jit.t` で別途 pin)。AOT / compiler JIT は **同一ソース**の `toylang_rt` crate (`compiler/runtime/toylang_rt/`) の `toy_str_concat` / `toy_to_string_<ty>` ランタイムヘルパを共有 (JIT は出力シンクを差し替えるだけ。旧 `compiler/src/jit.rs` ミラーは RUNTIME_PORT R1 で削除)。interpreter JIT は `ScalarTy::Str` (i64 ポインタ) + `jit_str_concat` / `jit_to_string_*` / `jit_print_str` ランタイムヘルパ (`interpreter/src/jit/runtime.rs`) で同形 layout を実装、ただし str は **function 境界 (param/return) は禁止** (Object lifecycle 整合性のため)。同時に既存の `BuiltinMethod::StrConcat/Substring/Trim/ToUpper/ToLower/Contains/Split` が型 checker で Unit を返していたバグも修正
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
- **`const fn`** (COMPILE-TIME-EVAL): `const fn double(n: u64) -> u64 { n * 2u64 }`
  — コンパイル時に走らせられる関数。`const D: u64 = double(21u64)` は
  **lowering 前にリテラルへ畳まれる**ので 4 実行系すべてで同じ値になる
  (以前は interpreter だけ通っていた)。`const` の次が `fn` なら修飾子、
  名前なら宣言。`never_allocates` とどちらの順でも書ける。
  適格性は到達可能性で検査 (`E0017`): heap / raw pointer builtin、
  `print` / `println`、アロケーションカウンタ、`extern`・closure・`dyn` は
  不可。注釈のない関数は**呼べる**。強制位置 (`const` 初期化子) の失敗は
  コンパイルエラー、それ以外は畳まないだけ。畳めるのは scalar のみ。
自由関数のみ (method は未対応)。**配列長に `const` 識別子 / 計算式を
   書ける** — `const N: u64 = 3u64` → `val a: [i64; N]`、さらに
   `[i64; double(2u64)]` / `[i64; N + 1u64]` (C5)。パーサが defer し
   (型は `ArraySize::Deferred`)、fold が呼び出しを畳み、解決 pass
   (`const_eval.rs::resolve_array_lengths`) が lowering と同じ literal
   リーダーで count に焼き込む。引数が非リテラル (`[i64; double(N)]`)
   は理由つきエラー。計算長は count の cross-check が無い (既知の gap)。
   例: `interpreter/example/const_fn.t`
- **`panic("msg")` ビルトイン**: 実行を中断するメッセージ付き panic。型検査では「Unknown」を返す扱いで、`if cond { panic("...") } else { value }` のような式位置でも使える。関数全体が panic で発散する場合も戻り型と関係なく型検査が通る
- **`test "name" { ... }` ブロック**: トップレベルに書けるテスト。`test` は contextual keyword なので `fn test(...)` や `val test = ...` は従来どおり使える。各ブロックは内部でゼロ引数関数に lower されるため型検査・バックエンドは特別扱い不要。通常実行では呼ばれず、`--test` で実行する。テストごとに独立した評価コンテキストを持つ
- **`assert_eq(a, b)` / `assert_ne(a, b)` ビルトイン**: 失敗時に **left / right の実値**と行番号を出す。パーサマクロで一時束縛 + 比較 + メッセージ組み立てに desugar される
- **`assert(cond, "msg")` ビルトイン**: `cond` が false のときだけ `panic(msg)` する糖衣。`(bool, str) -> ()`。message は false 時にのみ評価される。JIT は `brif cond, cont, fail; fail: call jit_panic; trap` で lower（success path はオーバヘッド最小、failure path は panic と同じ helper）
- **`?` (Try) 演算子**: postfix early-return。`expr?` は inner の型に応じて `Result<T, E>` か `Option<T>` の match に desugar し、success arm では unwrap 値を返し、error arm では enclosing 関数から `return` で伝播する。Parser が `Expr::Try { inner, .. }` を emit、type checker が in-place で `Block { val __try_t = inner; match __try_t { Ok(__try_v) => __try_v as T, Err(__try_e) => { return __try_t; panic("?-unreachable") } } }` に rewrite。backend (interpreter / AOT / JIT) は rewritten Match のみを観測。**success 型の変更を跨ぐ伝播** (`read_file(p)?` を `-> Result<u64, str>` で受ける) は error arm が宣言戻り型に対して `Result::Err(__try_e)` を再構築する (TRY-ERR-RETYPE、`return` の enum 構築は lowering 対応済み)。error 型の変更は `E2: From<E1>` で変換 (無ければ型エラー)。**`return` 式は宣言戻り型と突き合わせて検査される** (closure body は除外 — 合成関数として別戻り型を持つ)。Unit 関数内の `?` は型エラー。**制約**: inner は `Result` か `Option` 以外不可、AOT は `match` scrutinee 等の MVP 制約を継承 (function-call enum scrutinee は val-bind 経由)
- **`??` (null-coalesce) 演算子** (NULL-COALESCE): `a ?? b` は `a` が `Option::Some` / `Result::Ok` なら中身を、`None` / `Err` なら `b` を評価して返す。**右結合** (`a ?? b ?? c` = `a ?? (b ?? c)`)、比較演算子より密で shift 未満 (`a ?? b == c` = `(a ?? b) == c`)。**default は遅延評価** — type checker が `Block { val t = a; match t { Some(v) => v as T, None => b } }` に書き換えるので、`b` は None / Err パスでのみ走る (`unwrap_or` の結果、`unwrap_or_else` の評価規律)。lhs は `Option<T>` / `Result<T, E>` のみ、両 arm は同型 (bare `Option::None` の未解決 success 型は default 側が決める)。binary operand / 条件 / tail など `visit_expr` を通らない位置は checker が型だけ付けて、pool 書き換えは `apply_null_coalesce_rewrites` の post-pass で行う (バックエンドはいずれの経路でも desugar 後の Block のみを見る)。例: `interpreter/example/null_coalesce.t`
- **実行時例外 (try/catch/throw) は導入しない**: 言語仕様として例外機構を持たない。回復不能な失敗は `panic("...")` で即時停止 (process exit)、回復可能な失敗は `enum Result<T, E>` / `enum Option<T>` を戻り値で返して呼び出し側で `match` する。例外用の予約語 (`try` / `catch` / `throw` / `finally`) は parser で受理しない。`requires` / `ensures` 違反も `panic` 経路で停止する (例外として伝播しない)
- **間接化なしの再帰型は不可** (`[E0013]`): 自分を by-value で含む struct / enum は有限な layout を持てないので型検査で拒否 (`struct Node { next: Node }` / `enum List { Cons(i64, List), Nil }`)。**型引数が containment になるのは渡し先がそのパラメータを by-value で持つときだけ**なので `struct Tree { kids: Vec<Tree> }` は OK、`struct Held { w: Wrapper<Held> }` (`Wrapper<T> { v: T }`) は NG。cycle を切るのは `ptr` / 関数型 / `dyn Trait` の位置で、`&T` は lowering で消えるので切れない。書き方は `Box<T>` (`core/std/box.t`)、arena + index、raw `ptr` の 3 通り (`interpreter/example/box_linked_list.t` / `linked_list_arena.t` / `linked_list_ptr.t`)
- **所有権の移動** (`[E0014]`): `impl Drop` を持つ型の値を「今のスコープより長生きする場所」(値渡し引数 / struct・tuple・array・enum payload の要素 / 代入右辺) に置くと所有権が移り、以後その名前を読むとエラー。所有は**推移的** (`Vec<Box<i64>>` / payload に Box を持つ enum も対象)。`&T` / `&mut T` 引数は borrow、`val b = a` は**別名で移動ではない** (compound は alias)。分岐・ループ本体からの移動は drop flag が要るので拒否。**移動先は drop glue が再帰的に解放する** (DROP-GLUE): コンテナの死とともに要素 / フィールド / payload / Box の中身が free され、`--profile=mem` の `leaks` は 0 になる。free は全バックエンドで冪等 (never-reuse bump ヒープ)。
- **struct update (STRUCT-UPDATE)**: `P { x: 5i64, ..base }` — 書かなかった
  フィールドを `base` から埋める。`base` は**同じ struct** の値 (違えば型エラー)、
  `..base` は**末尾のみ**。結果は**新しい値**で `base` の別名ではない
  (`val q = p` の alias とは別物)。型検査器が省略フィールドを
  `base.field` に展開して普通の `StructLiteral` に書き換えるので
  **バックエンドは砂糖を見ない**。base が名前 / フィールドパス
  (`..a` / `..self` / `..o.inner`) なら一時束縛すら要らず、それ以外の式
  (`..make()`) は一時束縛を持つ。どちらも 3 backend 対応。
  例: `interpreter/example/struct_update.t`
- **タプル struct (NEWTYPE)**: `struct Meters(i64)` — フィールドを位置で宣言する。
  パーサが `"0"` / `"1"` ... という名前のフィールドを持つ通常の struct に desugar し、
  `Meters(v)` / `m.0` / パターン `Meters(v)` は型検査器が `StructLiteral` /
  `FieldAccess` / `Pattern::Struct` に書き換えるので、**バックエンドは砂糖を見ない**
  (3 backend 対応)。`impl` / generics / trait / `Drop` は named struct と同じ。
  同名の `fn` があればそちらが勝つ。`struct Empty()` は拒否。`println` は
  書いた形で出す (`Meters(3)`)。例: `interpreter/example/tuple_struct.t`
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
  - trait 本体には method の シグネチャを書く。`requires` / `ensures` 節も書ける。
    trait の契約は impl に継承される。**impl 側で `requires` を足すのは
    `[E0023]` で拒否** (DBC-LISKOV — `&dyn Trait` / `<T: Trait>` 経由の
    呼び出しは trait の節しか読めないので、追加要求は正しい呼び出しを落とす。
    trait 側が無契約でも同じ)。`ensures` は逆向きに健全なので足せる。
    inherent impl は対象外
  - `impl <Trait> for <Type> { ... }` は body 付き method を提供。型チェッカーが trait のシグネチャと比較し、不足 method や型不一致を検出
  - 型パラメータ bound `<T: SomeTrait>` を関数・struct・impl に書ける。呼び出し時に「実型がその trait を実装しているか」を検証
  - 実装メソッドは inherent method としても登録されるので `value.trait_method()` 形式で直接呼べる
  - **trait ジェネリクス** — `trait Foo<T, U>` 宣言と `impl Foo<i64, str> for Counter`。trait 側の型パラメータは impl の `trait_type_args` で置換してから conformance 比較される
  - **default method body** — trait シグネチャに `{ ... }` 本体を書くと、その method を omit した impl が inherent method として継承する。impl 側で同名 method を書けば override。3 backend で動作
  - **多重 trait bound `<T: A + B>`** — call-site の bound check は AND (全 trait 必須)、method dispatch は OR (最初に持つ trait を採用)。3 backend で動作
  - **`dyn Trait`** — `&dyn TraitName` による動的ディスパッチ。interpreter / AOT / compiler 側 JIT で動作 (empty struct / scalar field / nested field、`&mut dyn` writeback、struct / tuple / enum return の全組合せ)。interpreter 側 JIT は silent fallback。fat pointer ABI と vtable の設計は [`design-docs/DYN_TRAIT_AOT.md`](design-docs/DYN_TRAIT_AOT.md)
  - **未対応**: trait 継承 (A3)、associated types (A4)、interpreter 側 JIT の `dyn Trait` 対応 (compiler 側 JIT は対応済み)、`Box<dyn Trait>`、generic trait body 内での T 参照
- **クロージャ / ラムダ** — `fn(params) -> R { body }` の anonymous function literal。関数型は `fn (T1, T2) -> R` (推奨) / `(T1, T2) -> R` (bare)。parameter / return / `val` 注釈 / struct field 型に書ける。**capture の仕方は closure が捕捉した束縛より長生きしうるかで決まる** (CLOSURE-CAPTURE): `val f = fn(...)` に束縛して**同じ関数の中で呼ぶだけ**なら束縛を共有し (読みは live、書きは外へ通る — カウンタが書ける)、値として渡す / 返す / struct に入れる / 別の closure から呼ぶなら**コピー**を持つ (書き込みは `[E0021]`)。共有 capture は**書かれたスコープで解決する** (呼び出し側で同名を宣言しても持っていかれない)。interpreter は full support、AOT は env-based ABI で capturing / non-capturing 両対応だが **capture できるのは scalar だけ** (struct / tuple / array / dict の capture は名前つきで拒否、interpreter でのみ動く)、JIT は silent fallback。詳細は [`docs/language.md`](docs/language.md) と [`design-docs/CLOSURE_CAPTURE.md`](design-docs/CLOSURE_CAPTURE.md)
- **契約述語は副作用を持てない** (COMPILE-TIME-EVAL C4、`E0018`、**現状は警告**):
  `requires` / `ensures` から heap 確保 / free / ptr write / `print` /
  追えない呼び出し (`extern` / closure / `dyn`) に到達すると警告。
  カウンタ読みは許す。**定数引数の呼び出しが自分の `requires` を破る**
  場合も同じコードで警告 (値つき)。`const` 初期化子ではエラー (`E0017`)
- **Design by Contract キーワード**: `requires`（事前条件）, `ensures`（事後条件）。
  `ensures` 内では **`old(expr)`** が「関数入口時点の値」を指す (ALLOC-CONTRACT、3 backend)。
  **`requires` は RUNTIME-TRAP の guard を消す** (CONTRACT-ELISION): `requires b != 0`
  で 0 除算 guard、`requires a >= b` で u64 underflow guard が lowering から落ちる
  (パラメータ名のみ、`--release` では契約が検査されないので guard は残る)。
  **同じ事実を `if` の条件と `for` の範囲からも取る** (制御フロー版):
  `if b != 0u64 { a / b }` / `if a < b { 0 } else { a - b }` (else は条件の否定) /
  `for i in 0u64..8u64 { arr[i] }`。分岐は実際に評価されるので
  **`--release` でも効く**。guard 対象のコードがその名前に書く
  (代入 / 再束縛 / `&mut` / method 呼び出し) なら事実は取らない。
  アロケーションカウンタと組み合わせると**メモリ挙動をシグネチャで約束できる**。
  専用の節 **`ensures allocates(N)` / `retains(N)` / `allocations(N)`** (ALLOC-CONTRACT-SUGAR)
  があり、破れると実測値が出る (`retained 128 bytes, budget 0 bytes`、3 backend 同文言)。
  例: `interpreter/example/alloc_contract.t`。
  **静的版 `never_allocates fn f()`** (NEVER-ALLOCATES) は「確保しえない」を
  コンパイル時に検査 (`[E0016]`、診断は到達経路を出す)。closure / `dyn` /
  `extern` 経由は追えないので拒否 (`never_allocates extern fn ...` で申告は可能)。
  例: `interpreter/example/never_allocates.t`。関数 / メソッドの `-> ReturnType` の後、body `{` の前に複数並べられる。各節は bool 式で、AND 合成。`ensures` 内では `result` が戻り値を指す。違反時は `ContractViolation` エラーで停止。`INTERPRETER_CONTRACTS=all|pre|post|off`（unset = `all`）で `requires` / `ensures` を独立に切り替えられる（D の `-release` 相当）
- **可視性・外部連携**: `pub`（公開）, `extern`（外部関数）
- **モジュールシステム**: `package`, `import`, `as`
- **演算子**:
  - 算術: `+`, `-`, `*`, `/`, `%`（剰余・truncated remainder で `(-7) % 3 == -1`）
  - **実行時トラップ (RUNTIME-TRAP)**: `u64` 減算のアンダーフロー / 整数の 0 除算 /
    符号付き `MIN / -1` / 配列添字の境界外は **`panic`**（4 バックエンド一致、
    `compiler/tests/consistency/` が pin）。一方 `+` / `*` / 符号付き `-` の
    overflow は **wrap**（ビルドプロファイルに依らず 1 つの意味論）。逃げ道は
    `core/std/checked.t` の `checked_*` → `Option<T>` / `saturating_*`
    (`u64` / `i64` のみ、レシーバは名前束縛、enum 結果は `val` 束縛してから `match`)
  - 比較: `==`, `!=`, `<`, `<=`, `>`, `>=`。`==` / `!=` は同型 struct ペアで **operator overload** — その struct に `eq(&self, other: &Self) -> bool` method があれば dispatch (3 backend)。`s == t` で String/Vec<u8> 等の比較が動く
  - **全 binary / unary operator overload** (Phase B + OP-OVERLOAD-ARITH + OP-OVERLOAD-EXTEND Phase 1-4): 同型 struct ペアで以下に dispatch (3 backend、let-rhs context):
    - 算術: `+` / `-` / `*` / `/` / `%` → `add` / `sub` / `mul` / `div` / `rem` (`(&self, &Self) -> Self`)
    - 複合代入: `+=` / `-=` / `*=` / `/=` / `%=` / `&=` / `|=` / `^=` / `<<=` / `>>=` (parser desugar `a OP= b → a = a OP b` で算術 / ビット method 経由、AOT は `assign.rs` で既存 binding leaf locals に上書き)
    - 順序比較: `<` / `<=` / `>` / `>=` → `lt` / `le` / `gt` / `ge` (`(&self, &Self) -> bool`)
    - ビット: `&` / `|` / `^` / `<<` / `>>` → `bitand` / `bitor` / `bitxor` / `shl` / `shr` (Self 戻り)
    - 単項: `-` / `~` / `!` → `neg` / `bitnot` / `not` (`(&self) -> Self`)
    - **scope 外**: `&&` / `||` (short-circuit semantics)。加えて compiled レーンは **let-rhs 位置以外すべて** — chain (`a + b + c`)、struct literal operand (`a & Foo { ... }`)、結果のフィールド (`(a + b).x`)、引数位置 (`take(a + b)`)、条件位置 (`if (a + b) == c`)。interpreter には制限が無いので**インタプリタで動いた形が AOT で落ちる**。`val sum = a + b` に束縛してから使う (todo.md の OP-OVERLOAD-CHAIN)
  - 複合代入: 算術 5 種 (`+=`, `-=`, `*=`, `/=`, `%=`) とビット 5 種 (`&=`, `|=`, `^=`, `<<=`, `>>=`)（パーサで `lhs op= rhs` を `lhs = lhs op rhs` に desugar するので型検査もバックエンドも触らない。LHS は identifier / フィールド / タプル添字 / 添字の 4 形）。`>>=` は 1 トークンなので `Option<Option<u64>>= ..` のような形は型引数パーサが `>` `>` `=` に割り直す（`Vec<u64>= ..` は従来どおり parse error）。short-circuit の `&&=` / `||=` は追加しない
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
  - **struct パターン (PATTERN-STRUCT)**: `Point { x: 0i64, y }` / 省略形 `{ x }` /
    `..` で残りを無視。全フィールド列挙が既定 (`..` なしで省くと型エラー)。
    irrefutable な arm が 1 つあれば網羅。`if val` でも使える。**3 backend 対応**
    (tuple パターンも同時に lowering 対応した)
  - **or / 範囲 / `@` (PATTERN-EXTEND、完了)**: `1i64 | 2i64 => ...` /
    `0i64..5i64 => ...` (**半開区間**、整数リテラル端点のみ) / `n @ 2i64 => n`。
    3 つとも**実 pattern** なので **sub-pattern 位置にも書け** (`Circle(1i64 | 2i64)` /
    `Point { x: 0i64 | 1i64, y }` / `Just(n @ 3i64)`)、**網羅性・到達性に寄与する** —
    範囲は隣接区間を merge した区間集合で判定するので earlier arm に含まれる arm は
    unreachable (`0i64..10i64` の後の `3i64..5i64`)、空範囲 (`5i64..5i64`) は型エラー。
    ただし整数型を**跨がない**範囲だけでは網羅にならないので `_` は要る
    (診断は「add a wildcard `_` arm (or ranges that span the type)」)。
    `@` の網羅性は内側の pattern のもの (`x @ Color::Red` は Red を覆う)。
    3 backend 対応。interpreter JIT は範囲 / `@` / struct / tuple パターンで
    silent fallback
  - 網羅性チェック: wildcard がなく variant が欠落していると型チェックエラー
  - 到達性チェック: 同じ variant を 2 回 arm に書く、または `_` の後ろに arm を置くと型チェックエラー
  - ジェネリック enum: `enum Option<T> { None, Some(T) }` をサポート。タプル variant の引数から型パラメータを推論、ユニット variant（`None`）は `val x: Option<i64> = Option::None` のように型注釈から補完
  - リテラルパターン: scrutinee が `bool`/`i64`/`u64`/`str` のとき、`0i64 => ...` / `true => ...` / `"hello" => ...` のようにリテラルで分岐可能。`bool` は両値で網羅、整数・文字列は wildcard 必須
  - ネストパターン: `Option::Some(Option::Some(v))` や `Box::Put(Color::Red)` のように、タプル variant のサブパターンに再帰的にパターンを書ける。サブパターン位置には**任意の pattern** — Name バインディング / `_` / リテラル / ネストした enum variant に加え、struct・tuple・or・範囲・`@` も書ける

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

- **`parse::` モジュール** (`core/std/parse.t`、RUNTIME-LIB P0-B) —
  `to_u64` / `to_i64` / `to_f64` / `to_bool` が
  `Result<_, ParseError>` (`Empty` / `Invalid` / `Overflow`) を返す。
  **文法は厳しい側**: trim しない / 10 進のみ (`0x` も `_` も不可) /
  `to_u64("-1")` は `Invalid` / `1e999` は `Err(Overflow)` (無限大に
  しない) / `to_bool` は `true` `false` のみ。整数と bool は純 toylang、
  `to_f64` だけ extern (10 進 → 2 進変換) だが**文法判定は toylang 側**
  (でないと Rust `str::parse` と libc `strtod` で受理集合が食い違う)。
  詳細は `docs/language.md` の「Parsing numbers」
- **`io::` モジュール** (`core/std/io.t`) — `read_line()` / `argc()` /
  `arg(i)` / `env_var(name)` / `read_file(path)` /
  **`write_file(path, contents)`** / **`append_file(path, contents)`** /
  `file_exists(path)` / **`exit(code)`** /
  `now()` / `random()` / `random_seed(seed)` / `strftime(fmt, secs)` /
  `env_count()` / `env_name(i)` / `env_value(i)`。`extern fn` 宣言 +
  バックエンド別実装 (interpreter: `extern_io::build_io_registry`、
  AOT/JIT: `toylang_rt` の `toy_io_*` シンボル)。`read_file` / `env_var` /
  `read_line` は `Result<_, IoError>` を返す (RUNTIME-IO): payload を運ぶ
  extern が失敗 status を runtime 側に記録し、ペアの
  `__extern_io_*_status` extern が直後に読む (境界は scalar のまま)。
  `write_file` / `append_file` も同じ形 (`Ok(n)` は書けたバイト数、
  0 バイト書き込みは `Ok(0)`。ディレクトリが無い path は `NotFound`)。
  **`io::exit(code)`** は即座にプロセスを終える (`Drop` は走らない、
  どのバックエンドでも戻らない — in-process の埋め込みは道連れになる)。
  `Err` は `IoError` variant (`NotFound` / `PermissionDenied` /
  `IsADirectory` / `ReadError` / `WriteError` / `EndOfInput` /
  `Unknown`) — 網羅的な
  match で処理し、`Display` で `println(err)` が `not found` 等の
  文言を出す。compound 戻りなので `val` で受ける (compiled レーンは
  式位置の compound 呼び出しを拒否)。
  `random()` は非決定的だが、`random_seed(s)` で
  再現可能になる (0 も literal に保持、シーケンスは 3 バックエンド一致)。
  `strftime(fmt, secs)` は C `strftime` の文書化された部分集合で
  **UTC 固定** (ローカル時刻にしない、`docs/language.md` の
  Output 節相当の決定性規約)。環境変数一覧は `env_count` +
  `env_name` / `env_value` の `environ` 順アクセス (interpreter の
  `std::env::vars` も同じ順)。プログラム引数は CLI ではファイル後ろの
  引数、`RunOptions.args` で注入。
- `print(value)` — stdout に値を出力（改行なし）
- `println(value)` — stdout に値を出力 + 改行
- **`eprint(value)` / `eprintln(value)` (RUNTIME-LIB P0-A)** — 同じ整形で
  **stderr** に出す (`Display` dispatch も同じ、effect も `io`)。
  出力とdiagnosticsを分けたいときに使う。**IR の 3 つの print 命令が
  `stderr` フラグを持つ**形で通しており、AOT / JIT は
  `toy_print_stream(stderr)` で挟む (ヘルパは二重化していない)。
  runtime のシンクは 2 本 (`sink` / `err_sink`)。
  **interpreter 側 JIT は silent fallback**
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
| `__builtin_sizeof(value)` / `__builtin_sizeof::<T>()` | 値 / 型引数のバイトサイズ（u64）。primitive に加え struct（フィールド合計）/ tuple / array（要素合計）/ **enum（u64 タグ + 全 variant の payload 連結）** をサポート。enum のサイズは**型の性質で、手元の variant に依存しない** (`Vec<T>` の stride がこれ)。generic `T` の実体サイズ取得に使う。型引数形 (POINTER P1) は値なしで型から答え、generic は呼び出し引数 / レシーバ / `val` 注釈から解決（interpreter JIT は silent fallback） |
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
- **リージョン脱出は型検査で拒否 (`[E0022]`、REGION)**: スコープ付き
  allocator (同じ関数の `val` / `var` 束縛、またはインラインの
  `with allocator = Arena::new()`) から確保した値は、その allocator より
  長生きする場所 (`return` / 外側スコープの束縛・代入) へ出せない。
  スコープ内に留まるのは合法。パラメータ / フィールドの allocator は
  対象外 (`Arena::alloc` 自身がその形)。詳細は
  [`design-docs/REGIONS.md`](design-docs/REGIONS.md)
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

## 型検査の対象

**型検査器は body を検査するだけでなく書き換える** (`?` の desugar、
`Display` の `to_str` 挿入、char リテラルの narrowing)。したがって
**検査しない body は書き換え前の AST のままバックエンドに流れる**。
2026-08-30 に `interpreter/src/lib.rs` の `take(user_func_count)` を
外し、**integrate 後の全関数 (stdlib 含む) を検査する**ようにした
(impl block の method は元から検査対象)。コストは trivial program で
~2ms / process。stdlib に新しい機能を使うコードを書くときは、
これに依存していることを意識すること。

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

### テスト整理について

かつてここには `design-docs/FRONTEND_TEST_PLAN.md` と
`design-docs/INTERPRETER_TEST_PLAN.md` の要約 (「296個のテストが35ファイルに
分散」「35ファイルを7つに再構成」など) が置かれていたが、**その 2 つの
ファイルは一度もコミットされたことがない**。git 履歴にも存在しない。
存在しない文書の目次だけが 2 節にわたって残っていたので削除した
(2026-08-19)。**書かれていない計画をここに要約しないこと** —
`docs/language.md` を再掲して食い違った件と同じ失敗になる。

テストの物理配置は 2026-08-19 に別の理由 (ビルド時間) で決着している。
`tests/*.rs` は 1 クレート 1 バイナリに束ねてあり、経緯と実測値は
[`design-docs/todo.md`](design-docs/todo.md) の BUILD-PERF にある。
新しいテストファイルの追加手順は上の「Testing」節を参照。

### 重要な原則
- 新しい課題を発見した場合は、TodoWriteツールと `design-docs/todo.md` の両方に追加
- タスク完了時は、`design-docs/todo.md` の該当項目を「完了済み」セクションに移動
- 大きな改善や機能追加後は、`design-docs/todo.md` ファイルをgitにコミット
