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
| どの関数が漏らしたか・作業領域が大きすぎる・確保の思い込みを確かめたい | [6. 契約で確保を追い詰める](#6-契約で確保を追い詰める) |
| レーンによって答えが違う | [7. バックエンド間の食い違い](#7-バックエンド間の食い違い) |
| 一通りの手順 | [8. 推奨の流れ](#8-推奨の流れ) |

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

## 6. 契約で確保を追い詰める

`--profile=mem` は**どこで確保されたか** (確保位置) を言うが、**どの
関数が持ったまま帰ったか**は言わない。漏れたのが `Vec` や `String` の
中身なら、確保位置は `core/std/...` の 1 行で、利用者のコードのどこが
原因かは分からない。ここを埋めるのが、確保カウンタ
(`__builtin_live_bytes()` ほか 6 つ) を読む契約である。

契約の書き方そのもの (`allocates` / `retains` / `allocations` の違い、
生の式の書き方) は [`design_by_contract.md` のアロケーション契約](design_by_contract.md#アロケーション契約)
にある。ここではそれを**調べる道具として**使う技法をまとめる。

### 前提: カウンタが何を見ているか

確かめた性質で、以下の技法はすべてこれに依っている。

- **カウンタは実行全体のもの**で、呼び出し先の確保も含む。関数の
  契約は「この呼び出しの間に、呼び出し先も含めて何が起きたか」を見る。
- **`ensures` は、本体のローカルが解放された後で評価される。** 本体で
  作って捨てた `Vec` は `retains(0u64)` を破らない。返した値は残る。
- **予算には引数を使った式が書ける** — `ensures retains(n * 8u64)`。
- **数えるのは要求した分だけ。** `str` (連結・補間・リテラル) は数えない。
  `realloc` は移動の有無に関係なく「リサイズ 1 回」で、peak は新しい
  サイズしか見ない (新旧 2 ブロックが同時に生きている瞬間は数えない)。
- **カウンタは `main` と各 `test` の入口で 0 から始まる。**
- 契約は `--release` と `INTERPRETER_CONTRACTS=off` で消えるので、
  調査用に足した節は出荷するバイナリのコストにならない。残せば
  そのまま仕様の記述になる。

### 技法 1: `retains(0u64)` を関数境界に置いて二分探索する

```rust
fn label(x: u64) -> String {
    var s = String::from_str("item-")
    s.push_char('0' + (x % 10u64) as u32)
    s
}

# Hands out item-1, item-2, ... item-<end>.
struct Labels {
    at: u64,
    end: u64,
}

impl Labels {
    fn next(&mut self) -> Option<String> {
        if self.at == self.end {
            return Option::None
        }
        self.at = self.at + 1u64
        Option::Some(label(self.at))
    }
}

fn total_len(n: u64) -> u64 {
    var total = 0u64
    var it = Labels { at: 0u64, end: n }
    for s in it {
        total = total + s.len()
    }
    total
}

fn count_big(v: &Vec<u64>) -> u64 { ... }

fn report(v: &Vec<u64>) -> u64 {
    total_len(v.size()) + count_big(v)
}
```

`--profile=mem` は stdlib の行しか指さない:

```text
leaks (1 sites, 5 allocations, 50 bytes)
  core/std/string.t:143:25  5 allocations  50 bytes
```

疑わしい呼び出し木の根に `ensures retains(0u64)` を足すと、その関数が
持ったまま帰ったバイト数が出る:

```text
Contract violation: `ensures` clause #1 of function `report`: retained 50 bytes, budget 0 bytes
   = backtrace (innermost first):
       report (called at line 49)
       main
```

次に、その関数が呼ぶ関数 (`total_len` と `count_big`) へ同じ節を下ろす。
**最初に破れた節**が、漏らした関数の中で一番内側にあるものを指す:

```text
Contract violation: `ensures` clause #1 of function `total_len`: retained 50 bytes, budget 0 bytes
   = backtrace (innermost first):
       total_len (called at line 47)
       report (called at line 53)
       main
```

原因は、`next` という名前の method が返したものは**コンテナが持つ要素の
別名**として扱われ、for-in の item が drop されないこと (NEXT-ITEM-ALIAS)。
`Vec` の `iter()` ならそれで正しいが、`Labels` の `next` は値を新しく
作っているので、誰も解放しない。stdlib の `map` はレシーバの型を見て
区別しているが、自前のイテレータの `next` は名前でしか判断されない。
新しい値を作るメソッドには別の名前を付け、`while val` で受ける。
呼び出しの結果は束縛されるので、各周の終わりで解放される:

```rust
    fn take_next(&mut self) -> Option<String> { ... }   # 本体は同じ

    while val Option::Some(s) = it.take_next() {
        total = total + s.len()
    }
```

この版はリークが 0 になり、3 バックエンドで一致する。

- 破れた節の実測値 (`retained 50 bytes`) は**漏れた量そのもの**なので、
  要素数との関係 (5 要素 × 10 バイト) から「1 周に 1 個漏れている」と読める。
- 値を返す関数は `retains(0u64)` では必ず破れる。返す分を予算に書く —
  `ensures retains(n * 8u64)`。

### 技法 2: `--check` でまれな経路のリークを探す

確保の契約も `--check` の判定に使われる。特定の入力でしか通らない
経路のリークは、手で書いたテストではまず踏まないが、入力を生成させると
見つかる:

```rust
fn checksum(n: u64) -> u64
    requires n <= 1000u64
    ensures retains(0u64)
{
    val buf = __builtin_heap_alloc(n * 8u64 + 8u64)
    if n % 7u64 == 3u64 {
        return 0u64       # forgets to free
    }
    __builtin_heap_free(buf)
    n * 2u64
}
```

```text
$ interpreter --check --seed=0x5 prog.t
FAILED  checksum
    minimal counterexample: n = 444u64
    Contract violation: `ensures` clause #1 of function `checksum`: retained 3560 bytes, budget 0 bytes
```

「最小反例」は本当の最小 (`n = 3`) まで縮むとは限らない — 縮小は値を
半分にしていくので、`n % 7` のような条件は途中で外れる。それでも
漏れた量 (`3560 = 444 * 8 + 8`) から、どの確保が残ったかを逆算できる。

### 技法 3: 一時的な作業領域を `peak_live_bytes` で測る

`retains(0u64)` は「確保して全部返した」関数を通す。入力と同じ大きさの
作業コピーを取る関数は漏れていないが、メモリの山は高い。これを見るのが
`__builtin_peak_live_bytes()` である。

peak は**実行全体で単調に増える値**なので、関数の契約に書ける形は
「この呼び出しは、実行全体の山を B バイトより多く押し上げない」になる:

```rust
fn count_repeats(v: &Vec<u64>) -> u64
    ensures retains(0u64)
    ensures __builtin_peak_live_bytes() <= old(__builtin_peak_live_bytes()) + 4096u64
{
    var w: Vec<u64> = Vec::with_capacity(v.size())
    for i in 0u64..v.size() { w.push(v.get(i)) }
    w.sort()
    ...
}
```

100 要素 (作業領域 800 バイト) は通り、1000 要素 (8000 バイト) で
止まる (3 バックエンド一致):

```text
Contract violation: `ensures` clause #2 of function `count_repeats` evaluated to false (with result = 993)
```

この形には限界が 2 つある。

- **呼ぶ前に山が既に高いと見逃す。** 押し上げなければ通るので、
  それ以前に大きな山があった実行では、関数自身の作業領域は測れない。
- **生の式の違反は実測値を言わない** (`with result = ...` だけ)。
  糖衣の 3 節は実測値を出すが、peak の糖衣は無い。

**正確に測るなら `test` の中で測る。** カウンタは `test` の入口で 0 に
なるので、呼ぶ直前に「山 = 今の live」であることを確かめれば、
呼んだ後の山との差がその関数の作業領域になる。`assert` の文言に
実測値を入れておけば、破れたときに数字が出る:

```rust
test "count_repeats needs no more than the input again" {
    val v = input(1000u64)
    val base = __builtin_live_bytes()
    assert_eq(__builtin_peak_live_bytes(), base)
    val r = count_repeats(&v)
    val extra = __builtin_peak_live_bytes() - base
    assert(extra <= v.size() * 8u64, "working set {extra} bytes")
    assert_eq(__builtin_live_bytes(), base)
}
```

```text
$ toy test pk --backend vm      # 予算を v.size() * 4u64 に絞った場合
       |     ^^^^^^ panic: working set 8000 bytes
2 passed, 1 failed
```

この test は vm と aot の両レーンで通る。最後の `assert_eq` は
「呼ぶ前と live が同じ」で、増えていないことだけを見るなら
`core/std/testing.t` の `assert_no_growth(base)` でも書ける。

### 技法 4: stdlib についての思い込みを `ensures` で確かめる

「この操作はメモリを返すはず」「容量は足りているはず」という前提は、
カウンタの比較で 1 行の節になる。破れれば前提が間違っている:

```rust
# Assumption: clear() gives the buffer back.
fn reset(v: &mut Vec<u64>)
    ensures __builtin_live_bytes() < old(__builtin_live_bytes())
{
    v.clear()
}

# Assumption: the capacity is enough, nothing is grown inside.
fn fill(v: &mut Vec<u64>, n: u64)
    ensures __builtin_realloc_count() == old(__builtin_realloc_count())
{
    for i in 0u64..n { v.push(i) }
}
```

- `reset` は破れる — **`Vec::clear()` は長さを 0 にするだけで容量を
  残す**。返したいなら `Vec` ごと捨てる (束縛し直す)。
- `with_capacity(16u64)` の `Vec` に `fill(&mut v, 16u64)` は通り、続く
  `fill(&mut v, 1u64)` は破れる (17 個目の `push` で伸ばし直す)。
  `Vec::new()` から 1000 個積むと `realloc_count` は 8、
  `with_capacity(1000u64)` なら 0。

### 書き方の注意

- **差を取るときは足し算の形にする。** `live_bytes() - old(live_bytes())`
  は、入口より live が減る関数で u64 アンダーフローの panic になる。
  `live_bytes() <= old(live_bytes()) + B` と書く。
- `old(...)` は**関数の入口**の値。ループの周回ごとの性質は、本体を
  関数に切り出してから契約を付ける。
- 漏れる量が分かっているなら、`retains` の予算をその式にして**直った
  ことの確認**にも使える (予算 0 に戻したときに通れば直っている)。

## 7. バックエンド間の食い違い

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

## 8. 推奨の流れ

1. `toy check --format=json` → `toy fix`。
2. 型が分からなければ `val x: _ = ...` か `toy query type`。
3. 思い込みを `requires` / `ensures` に書き、`toy test --check` で最小反例を取る。
4. 再現したら `test` ブロックに固定し、`toy test --backend vm` で全部の失敗を見る。
5. 値を追うのは `log::debug` (+ `TOY_LOG=debug`)。一時的に見るだけなら
   `__builtin_dbg` (stdout に出るので後で外す)。
6. メモリの疑いは `--profile=mem` → `--heap-check=poison` → `reuse` の順に絞る。
   リークの持ち主が stdlib の行しか分からなければ、`retains(0u64)` を
   関数境界に置いて二分探索する (6 節)。
7. 最後に `--all-backends` / `--backend all` で実行系どうしの一致を見る。

## 既知の制限

- `__builtin_dbg` の出力先は stdout で固定。
- `INTERPRETER_CONTRACTS=off` の間は backtrace から呼び出し行が落ちる
  (3 節)。
- `toy effects` はテストブロックを内部名 (`__test_0`) で並べる。
- AOT の `toy test` は最初の panic で同じドライバの残りを止める (`--backend vm` を使う)。
- 自前のイテレータの `next` が新しく作った所有値 (`String` 等) を
  for-in の item で受けると、1 周に 1 個漏れる (6 節 技法 1)。stdlib の
  `map` は漏れないが、compiled レーンは `String` を返す closure を
  そもそもコンパイルできない (closure の引数と戻り値はスカラーだけ)。
- peak の契約は「実行全体の山を押し上げない」しか言えない (6 節 技法 3)。
