# デバッグ — 実践ガイド

toylang で書いたプログラムが**思った通りに動かないとき**に、処理系が
最初から持っている道具でどう追い詰めるかを扱う。文法と各 builtin の
正本は [`language.md`](language.md)、契約の書き方は
[`design_by_contract.md`](design_by_contract.md) にあり、ここは
「どの症状にどれを使うか」の運用の話。例はすべて 2026-10-13 に
実際に走らせて出力を写したもの。

| 症状 | 節 |
|---|---|
| 型が合わない・型が分からない | [1. 動かす前に聞く](#1-動かす前に聞く) |
| 値がどこでおかしくなるか追いたい | [2. 走らせながら見る](#2-走らせながら見る) |
| panic した・契約が破れた | [3. 落ちたときの読み方](#3-落ちたときの読み方) |
| 再現条件が分からない・直ったか確かめたい | [4. 仮説を契約とテストに書く](#4-仮説を契約とテストに書く) |
| リーク・解放後の使用・確保が多すぎる | [5. メモリ](#5-メモリ) |
| レーンによって答えが違う | [6. バックエンド間の食い違い](#6-バックエンド間の食い違い) |
| 一通りの手順 | [7. 推奨の流れ](#7-推奨の流れ) |

## 前提

- 何度も検査・実行するなら release の `toy` を 1 回ビルドして直接叩く
  (`cargo build --release -p toy` → `target/release/toy ...`)。
  `cargo run` は毎回の鮮度確認と debug ビルドの遅さを払う。
- 機械で読むなら `--format=json`。型エラーも**実行時エラーも**
  stderr に JSON 配列で出て、`code` / `file` / `span` / `backtrace[]` /
  `suggestions[].edits` を持つ。
- `toy` はパッケージ (`main.t` か `src/` を持つディレクトリ) 単位で動く。
  位置は**パッケージ root からの相対パス**で出る (`main.t:3:5`、
  `src/greet.t:2:1`)。stdlib は `core/std/...`。

## 1. 動かす前に聞く

推測で直す前に、処理系に聞く。

### 型ホール

分からない型の位置に `_` を書くと、推論結果を答えて止まる。1 回の
実行でファイル中の全ホールが答えられ、後続はカスケードしない。

```rust
val t: _ = v.iter().map(fn(x: u64) -> u64 { x * 2u64 })
```

```text
$ toy check g --format=short
main.t:11:16: E0011 type hole: `t` has type `MapIter<u64, u64>`
```

### 位置を指して聞く

```text
$ toy query type g/main.t:11:9 --format=json     # → "type": "MapIter<u64, u64>"
$ toy query def  main.t:9:20                     # 名前 → 定義位置
$ toy query refs main.t:5:4                      # 定義 → 参照一覧
$ toy query callers twice --in mypkg             # / callees
$ toy api core/std/string.t                      # stdlib を grep する代わりに
```

### 診断の意味と直し方

`toy explain E0014` が原因カテゴリ・再現例・直し方を出す。
machine-applicable な提案は `toy fix mypkg` で当てて再検査まで済む
(`--dry-run` で先に見られる)。

### 関数が何をしうるか

```text
$ toy effects g
clamp_sub  pure
main       alloc, free, raw_read, raw_write, panic
```

「この関数は確保しないはず」「I/O しないはず」の確認に使う。静的に
約束させたいなら `never_allocates fn` (破ると `[E0016]` が到達経路を出す)。

## 2. 走らせながら見る

print デバッグを、消し忘れても困らない形で行う。

| 道具 | 出力先 | 使いどころ |
|---|---|---|
| `__builtin_dbg(expr)` | **stdout** | 式の途中に挟む。値をそのまま返す |
| `eprintln(x)` | stderr | プログラムの出力を汚さない |
| `log::debug("...")` | stderr | 残しておける観測点。`TOY_LOG=debug` のときだけ出る |
| `__builtin_backtrace()` | (str を返す) | 「どこから来たか」を自分で出す |
| `__builtin_function_name()` / `__builtin_source_line()` | (値を返す) | ログに位置を添える |

```rust
fn inner(x: u64) -> u64 {
    log::debug("inner x={x} in {__builtin_function_name()}")
    eprintln(__builtin_backtrace())
    __builtin_dbg(x * 3u64) + 1u64
}

fn outer(x: u64) -> u64 { inner(x) }

fn main() -> u64 {
    println(outer(4u64))
    0u64
}
```

```text
$ TOY_LOG=debug interpreter obs.t
DEBUG inner x=4 in inner
   = backtrace (innermost first):
       inner (called at line 7)
       outer (called at line 10)
       main
[obs.t:4] x * 3u64 = 12
13
```

注意:

- **`__builtin_dbg` は stdout に出る** (`println` に展開されるため)。
  プログラムの出力と混ざるので、出力を突き合わせるテストや
  `--all-backends` の比較の前には外す。混ぜたくなければ `eprintln`。
- `log::` のレベルは `TOY_LOG=error|warn|info|debug|trace`、
  `TOY_LOG_TIME=1` で先頭に UTC の時刻が付く。
- `--release` はフレームを記録しないので、`__builtin_backtrace()` は
  入口のフレームだけを返す (`= backtrace (innermost first):` の下に
  `main` の 1 行)。

## 3. 落ちたときの読み方

実行時の失敗は、どの実行系でも同じ形で **stderr** に出る — 位置、
ソースの抜粋、**値つき**の文言、backtrace (最内が先)。

```rust
fn half(n: u64) -> u64
    requires n % 2u64 == 0u64
{
    n / 2u64
}

fn sub(a: u64, b: u64) -> u64 { a - b }

fn main() -> u64 {
    val h = half(13u64)
    sub(1u64, h)
}
```

```text
$ interpreter trap.t
Runtime error occurred:
Error at trap.t:2:14:
   |
 2 |     requires n % 2u64 == 0u64
   |              ^^^^^^^^^^^^^^^^ Contract violation: `requires` clause #1 of function `half` evaluated to false (with n = 13)
   |
   = backtrace (innermost first):
       half (called at line 10)
       main
```

- 黙って誤った値を返さずに止まるもの: `u64` 減算のアンダーフロー
  (`panic: u64 subtraction underflowed: 1 - 6`)、0 除算、符号付き
  `MIN / -1`、配列と `Vec` の範囲外 (`index 5, length 3`)。
  `+` / `*` の overflow は wrap する (止まらない)。止めずに聞くなら
  `core/std/checked.t` の `checked_*`。
- 無限再帰は `panic: recursion limit exceeded (N frames deep)` になり、
  backtrace は畳まれて出る。上限は実行系ごと (tree-walker 30、それ以外 1024)。
- `--format=json` では同じ失敗が `code: "E0020"` (契約) /
  `"E0019"` (panic・trap)、`backtrace: [{function, line}]` 付きで出る。
- **`--release` では契約と backtrace が消え、位置は残る**。デバッグは
  既定のビルドで行う。
- `INTERPRETER_CONTRACTS=off|pre|post` で契約を外すと、契約の向こうに
  ある本当の trap が見える。ただし `off` の間は interpreter が
  `--release` 相当で lowering するので、backtrace から
  `(called at line N)` が落ちる。

## 4. 仮説を契約とテストに書く

「ここはこうなっているはず」を書き残すと、処理系が反例を探してくれる。

### 契約 + `--check` (最も効く)

`requires` を入力フィルタ、`ensures` をオラクルにして入力を生成し、
**最小の反例**と再現用の seed を出す。

```rust
fn clamp_sub(a: u64, b: u64) -> u64
    requires b <= 100u64
    ensures result <= a
{
    if a < b { 0u64 } else { a - b + 1u64 }
}
```

```text
$ toy test g --check --seed=0x1
FAILED  clamp_sub
    minimal counterexample: a = 0u64, b = 0u64
    Contract violation: `ensures` clause #1 of function `clamp_sub` evaluated to false (with a = 0, b = 0, result = 1)
3 contracted function(s)/method(s) checked, 400 case(s), 1 failed  (seed: 0x1; replay with toy test --check --seed=0x1)
```

### `test` ブロック

```rust
test "subtracts" { assert_eq(clamp_sub(5u64, 2u64), 3u64) }
test "too big" panics "requires" { val z = clamp_sub(1u64, 200u64) }
```

```text
$ toy test g --backend vm
FAILED  subtracts (main.t:15)
    Error at main.t:15:20:
    15 | test "subtracts" { assert_eq(clamp_sub(5u64, 2u64), 3u64) }
      left:  4
      right: 3
       = backtrace (innermost first):
           test "subtracts"
3 passed, 1 failed   0.05 s
```

- `assert_eq` / `assert_ne` は両辺の実値を出す。
- `panics "text"` は panic することと、その文言に `text` が含まれることを
  確かめる。違う panic なら実際の文言を見せる。
- `core/std/testing.t` に、失敗が位置と両辺を言うアサーションがある:
  `assert_str_eq` (最初に違うバイトを出す) / `assert_bytes_eq` /
  `assert_close` / `assert_some` / `assert_ok` / `assert_err` /
  `assert_golden` (`toy test --bless` で記録)。
- **`--backend vm` は全部の失敗を 1 回で報告する。** 既定の AOT は
  panic がプロセスを終わらせるので、同じドライバに入った後続のテストは
  `not run: an earlier test ended the process` になる。
- モジュール (`src/*.t`) の中の `test` も拾われ、`greet::名前` と
  名乗る。`toy test FILTER` で名前を絞れる。

## 5. メモリ

| 症状 | 道具 |
|---|---|
| リーク | `--profile=mem` (確保位置ごとの `leaks`)。`--format=json` も可 |
| 確保量を約束したい | `ensures retains(0u64)` / `allocates(N)` / `allocations(N)` |
| テストの区間で確保が増えないこと | `heap_mark()` + `assert_no_growth(mark)` |
| 解放後の使用・末尾越え・二重 free | `--heap-check=poison` |
| 番地の再利用に依存していないか | `--heap-check=reuse --heap-quarantine=0` |
| 二重 free の棚卸し (止めない) | `--heap-check=report` |

```rust
fn work(n: u64) -> u64
    ensures retains(0u64)
{
    val p = __builtin_heap_alloc(n * 8u64)
    0u64
}

fn main() -> u64 { work(2u64) }
```

```text
$ interpreter --profile=mem leak.t
Runtime error occurred:
Error at leak.t:2:13:
   ...  Contract violation: `ensures` clause #1 of function `work`: retained 16 bytes, budget 0 bytes
   = backtrace (innermost first):
       work (called at line 8)
       main
memory profile
  alloc_count       1
  ...
leaks (1 sites, 1 allocations, 16 bytes)
  leak.t:4:13  1 allocations  16 bytes
```

解放後の読みは、既定では**黙って古い値が読めてしまう** (下の例は
終了コード 7 で正常終了する)。`--heap-check=poison` を付けると、
確保位置と解放位置の両方を言って止まる。

```rust
unsafe fn poke(p: ptr) -> u64 { __builtin_ptr_write(p, 0u64, 7u64)  0u64 }
unsafe fn peek(p: ptr) -> u64 { __builtin_ptr_read::<u64>(p, 0u64) }

fn main() -> u64 {
    val p = __builtin_heap_alloc(16u64)
    poke(p)
    __builtin_heap_free(p)
    peek(p)
}
```

```text
$ interpreter --heap-check=poison uaf.t
... panic: heap check: read at offset 0 of a 16-byte block that was already freed (allocated at uaf.t:5:13, freed at uaf.t:7:5)
```

compiled レーンでは poison は**ビルドフラグ**
(`compiler prog.t --heap-check=poison -o prog`、`toy build/run/test
--heap-check=poison`)。AOT バイナリ単体なら `TOY_PROFILE_MEM=1` /
`TOY_HEAP_CHECK=report` を環境変数で渡す。

## 6. バックエンド間の食い違い

```text
$ compiler prog.t --all-backends --format=json   # interpreter / JIT / AOT を突き合わせる
$ toy run mypkg --backend all                     # 同じことをパッケージで
$ toy test mypkg --backend all                    # aot と vm で合否を突き合わせる
```

一致すれば stderr に 1 行。食い違いは**プログラムではなく処理系の
バグ**を示す。

AOT だけが `compiler MVP cannot use ... in expression position; bind the
result with `val`` で拒むことがある。まず `--backend vm` で意味が正しい
かを確かめ、それから言われた通り `val` に束縛して書き直す。

## 7. 推奨の流れ

1. `toy check --format=json` → `toy fix`。
2. 型が分からなければ `val x: _ = ...` か `toy query type`。
3. 思い込みを `requires` / `ensures` に書き、`toy test --check` で最小反例を取る。
4. 再現したら `test` ブロックに固定し、`toy test --backend vm` で全部の失敗を見る。
5. 値を追うのは `log::debug` (+ `TOY_LOG=debug`)。一時的に見るだけなら
   `__builtin_dbg` (stdout に出るので後で外す)。
6. メモリの疑いは `--profile=mem` → `--heap-check=poison` → `reuse` の順に絞る。
7. 最後に `--all-backends` / `--backend all` で実行系どうしの一致を見る。

## 既知の制限

- `__builtin_dbg` の出力先は stdout で固定。
- `INTERPRETER_CONTRACTS=off` の間は backtrace から呼び出し行が落ちる
  (上の 3 節)。
- `toy effects` はテストブロックを内部名 (`__test_0`) で並べる。
- AOT の `toy test` は最初の panic で同じドライバの残りを止める (`--backend vm` を使う)。
