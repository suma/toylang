# Design by Contract — 実践ガイド

`requires` / `ensures` の**使い方**を扱う。文法と規則の正本は
[`language.md` の Design by Contract 章](language.md#design-by-contract)
にあり、ここはその上に載る運用の話 — 何を契約に書くべきか、書いた契約を
どう検証するか、どこで足をすくわれるかを、実際に動かして確かめた例で
説明する。

| 知りたいこと | 節 |
|---|---|
| とりあえず 1 つ書きたい | [最初の契約](#最初の契約) |
| 契約が破れたとき何が出るか | [診断の読み方](#診断の読み方) |
| 変化について書きたい | [`old(...)`](#old---変化について語る) |
| メモリ挙動を約束したい | [アロケーション契約](#アロケーション契約) |
| 書いた契約が本当か確かめたい | [`--check`](#check---契約をプロパティテストにする) |
| 契約は遅くないのか | [契約と性能](#契約と性能) |
| 落とし穴を先に知りたい | [Tips](#tips) / [よくある失敗](#よくある失敗) |

動くサンプルは
[`interpreter/example/design_by_contract.t`](../interpreter/example/design_by_contract.t)、
題材別に
[`contracts.t`](../interpreter/example/contracts.t)（基本形）、
[`memory_contract.t`](../interpreter/example/memory_contract.t)（カウンタ）、
[`alloc_contract.t`](../interpreter/example/alloc_contract.t)（`old` と予算）。

---

## 最初の契約

`requires` は**呼び出し側が負う義務**、`ensures` は**実装側が負う義務**。
この非対称が契約の本質で、違反したときにどちらのバグかが決まる。

```rust
fn average(total: u64, count: u64) -> u64
    requires count != 0u64      # caller's obligation
    ensures result <= total     # implementation's obligation
{
    total / count
}
```

- 両方とも `bool` 式で、複数書けば AND で合成される
- `ensures` では `result` が戻り値を指す
- メソッドでは `self` が両方で使える
- 違反は `panic` と同じ経路で停止する（catch できない — 言語に例外機構が無い）

**契約に書くべきでないもの**は「回復可能な失敗」。ファイルが無い、入力が
不正、といった**起きて当然の事態**は `Option<T>` / `Result<T, E>` を返して
呼び出し側に `match` させる。契約は「起きたらプログラムが間違っている」
事柄に使う。

---

## 診断の読み方

違反すると、どの節が破れたかが 1-based の番号で出る。

```
$ cargo run -q -p interpreter -- broken.t
Runtime error occurred:
Contract violation: `ensures` clause #1 of function `f` evaluated to false (with a = 1, b = 5, result = -4)
```

`with ...` はそのとき実際に入っていた値で、これが原因究明のほぼ全部を
持っていく。`clause #1` は**その種類の中での順番**なので、`requires` が
2 つ `ensures` が 2 つある関数の `ensures` 2 番目は `#2` と出る。

> **注意**: `&self` / `&mut self` と書いたメソッドでは、`with ...` に
> **`self` の値が出ない**（引数は出る）。暗黙形の receiver は
> パラメータリストに入らないため。`self: Self` と明示した形なら
> `self = C { n: 1 }` のように表示される。

---

## `old(...)` — 変化について語る

事後条件は「最終状態」だけでなく「何が変わったか」を言いたいことが多い。
`ensures` の中の `old(expr)` は、`expr` が**関数に入った時点で持っていた
値**を指す。

```rust
struct Counter { n: u64 }

impl Counter {
    fn bump(&mut self, by: u64) -> u64
        ensures result == old(self.n) + by
        ensures self.n == result
    {
        self.n = self.n + by
        self.n
    }
}
```

`old` 無しではこの節は**書きようがない**。`ensures` が走る時点で `self.n`
は既に新しい値になっているからで、`result == self.n + by` と書くと
（`42 == 42 + 2` で）必ず偽になる。

評価は `requires` の**後**、body の**前**に 1 回だけ。したがって
事前条件が「そのスナップショット式が合法であること」を保証できる。
事後条件を切っている実行モードではスナップショット自体を取らない。

---

## アロケーション契約

**関数のメモリ挙動をシグネチャに載せられる。** 3 つの節が使える:

```rust
# "1 バイトも確保しない" — 腐るコメントではなく、
# body に heap 呼び出しが増えた日に止まる節
fn triangle(n: u64) -> u64
    ensures allocates(0u64)
{ ... }

# "最大 256 バイトまで要求し、1 回で取り、全部返す"
fn scratch(n: u64) -> u64
    ensures allocates(256u64)
    ensures allocations(1u64)
    ensures retains(0u64)
{ ... }
```

| 節 | 数えるもの | 答える問い |
|---|---|---|
| `allocates(N)` | 要求したバイト総量 | そもそも確保したか |
| `retains(N)` | 返さなかったバイト数 | 漏らしていないか |
| `allocations(N)` | 確保の回数 | 1 回に抑えているか |

**3 つは畳めない。** `retains(0)` は「確保して解放した」関数も通すが、
`allocates(0)` は 1 バイトの要求も許さない。arena に取る関数を縛るなら
バイト数ではなく `allocations` が自然、というように、意図に合う軸を選ぶ。

破れると**実測値が出る**:

```
Contract violation: `ensures` clause #1 of function `leaky`: retained 128 bytes, budget 0 bytes
```

これは 3 バックエンドで同じ文言になる（コンパイル済みバイナリも同じ）。

### 生の式でも書ける

節は糖衣で、[アロケーションカウンタ](language.md#allocation-counters)を
`old` と組み合わせた式に展開される。カウンタは 6 つあり、糖衣が扱わない
軸（`free_count` など）を使いたいときは生で書く:

```rust
ensures __builtin_free_count() <= old(__builtin_free_count()) + 2u64
```

> **生で書くときは「引き算」ではなく「足し算」に寄せる。** `live` が
> 入口より**減る**関数（引数で渡されたポインタを解放する等）では、
> `live_bytes() - old(live_bytes()) <= 0u64` が u64 アンダーフローで
> panic する。糖衣は最初からこの形を避けて展開するので、この罠は
> `retains(0u64)` と書く限り踏まない。

> **数えるのは「プログラムが要求した分」だけ。** `__builtin_heap_alloc` /
> `realloc` とその上に建つ stdlib が対象で、`str` を保持するために
> 言語ランタイムが使うメモリ（連結・`to_string`・補間・リテラル）は
> **数えない**。バックエンドごとに文字列の持ち方が違うので、数えると
> 数字が engine 依存になるため。つまり `println("n = {n}")` はどの
> バックエンドでも 0 で、`__builtin_heap_alloc(32u64)` はどこでも 32。

カウンタは**リクエスト単位**で、3 バックエンドで同じ数字になる。
`realloc` は移動したかどうかに関係なく「リサイズ 1 回」として数える。
数字の意味は `--profile=mem` のレポートと同一なので、契約が主張している
値をそのまま `--profile=mem` で観察できる。

**呼び出し先の確保も数える。** カウンタはプロセス全体なので、
`ensures allocates(0u64)` を宣言した関数が確保する関数を呼べば、そこで
捕まる。「確保しない」が推移的に効くということで、これは意図した性質。

### 静的版 — `never_allocates`

`ensures allocates(0u64)` は**その呼び出しで確保しなかった**ことを実行時に
確かめる。`never_allocates` は**確保しえない**ことをコンパイル時に確かめる:

```rust
never_allocates fn triangle(n: u64) -> u64 {
    var total: u64 = 0u64
    var i: u64 = 1u64
    while i <= n { total = total + i  i = i + 1u64 }
    total
}
```

関数から到達できる**すべての経路**を辿り、`__builtin_heap_alloc` /
`__builtin_heap_realloc` に届いたら型エラー。診断は**経路**を出す —
確保しているのはたいてい自分の関数ではないため:

```
[E0016] `build` is declared `never_allocates`, but it can reach the
        allocator: build -> new -> __builtin_heap_alloc
```

| | `never_allocates` | `ensures allocates(0u64)` |
|---|---|---|
| いつ | コンパイル時 | 実行時 |
| 何を保証 | 確保しえない | その呼び出しでは確保しなかった |
| `extern` の先 | 保証外（申告ベース） | **数える** |
| 実行時コスト | ゼロ | カウンタ読み + 比較 |

**追えない呼び出しは拒否される。** closure 値・`dyn Trait`・`extern fn`
経由は呼び先が静的に決まらないので、「確保しない」と仮定せずエラーにする
（仮定すると保証全体が無意味になるため）。`extern` だけは書き手が責任を
持つ逃げ道がある:

```rust
never_allocates extern fn getchar() -> i32 from "c"
```

これは検査ではなく**申告**で、実装は言語の外にある。

`println("{x}")` は許される — `str` を保持するために言語ランタイムが使う
メモリはプログラムの確保ではなく、カウンタも数えないため。

`never_allocates` は contextual なので、自分の関数や変数を
`never_allocates` と名付けているプログラムは影響を受けない。
例: [`never_allocates.t`](../interpreter/example/never_allocates.t)。

---

## `--check` — 契約をプロパティテストにする

`requires` は「どんな入力が合法か」、`ensures` は「何が成り立つべきか」を
既に述べている。`--check` はそれを**入力フィルタ**と**オラクル**として読み、
反例を探して最小化する。

```
$ cargo run -q -p interpreter -- --check example.t
FAILED  divide
    minimal counterexample: a = 1i64, b = 2i64
    Contract violation: `ensures` clause #1 of function `divide` evaluated to false (with a = 1, b = 2, result = 0)
1 contracted function(s) checked, 1 failed  (seed: 0x18cdd50397c3b698; replay with --check --seed=0x18cdd50397c3b698)
```

**書いた契約は必ず `--check` にかけること。** もっともらしい事後条件ほど
偽であることがある。実例として、この言語の仕様書と example が長らく載せて
いた模範例が壊れていた:

```rust
ensures result * b == a     # 整数除算は切り捨てるので 7 / 2 で偽
```

正しくは剰余を勘定に入れる。ついでに、非ゼロ除数だけでは
`MIN / -1`（表現できない結果でトラップする）を排除できないことも
`--check` が教えてくれる:

```rust
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    requires !(a == -9223372036854775808i64 && b == -1i64)
    ensures  result * b + (a % b) == a
{ a / b }
```

`--seed=0x...` で同じ列を再現できる。失敗したときは必ず seed が印字される。

### `--check` の守備範囲

踏む前に知っておくと時間を無駄にしない:

- **対象は自由関数だけ**。`impl` 内のメソッドは掃かれない。メソッドの契約は
  `test` ブロックか実行で確かめる
- **生成できる型は `bool` / `i64` / `u64` / `f64`**。それ以外（`str`、struct、
  enum など）を引数に持つ関数は**黙って対象外**になる。`0 contracted
  function(s) checked` と出たらこれを疑う
- **`requires` が狭すぎる場合は報告される**。`requires n == 42u64` のような節は
  ほぼ全ての生成値を弾くので、通ったケースが一握りなら `THIN` として出る。
  サマリにも合計ケース数が入るので、「200 ケース通った」と「1 ケース通った」を
  取り違えない:

  ```
  THIN  exact — only 1 input(s) satisfied `requires` (4000 discarded)
  1 contracted function(s) checked, 1 case(s), 0 failed  (seed: 0x...)
  ```

  全部弾かれた場合は別立てで出る:

  ```
  INCONCLUSIVE  impossible — `requires` rejected all 4000 generated inputs
  ```

---

## 契約と性能

`requires` が[実行時トラップ](language.md#runtime-traps)を排除している場合、
コンパイラは**そのガードを消す**。契約は入口で 1 回検査され、演算ごとの
チェックが不要になる。

```rust
fn div(a: i64, b: i64) -> i64
    requires b != 0i64      # ここで 1 回検査され…
{
    a / b                   # …ここの 0 除算ガードが消える
}
```

| 節の形 | 消えるガード |
|---|---|
| `x != 0` / `0 != x` / `x > 0` / `x >= 1` | `a / x`・`a % x` の 0 除算 |
| `a >= b` / `b <= a` | `a - b` の u64 アンダーフロー |

除算と減算を 1 回ずつ回すループの実測で **0.13s → 0.06s**（1 億回、AOT、
cranelift `speed`）。cranelift 自身にはこれができない — 事実は事前条件の
中にあり、その時点では単なる分岐にしか見えないため。

**したがってホットパスの除算には `requires` を書く価値が二重にある。**
ただし消える条件は限定的で、対象は**パラメータ名**だけ。フィールド
（`self.n`）、添字、計算式はガードを保つ。同名の `val` / `var` が
パラメータを覆った時点でも事実は失効する。

> **`--release` はガードを残す。** リリースでは事前条件が emit されない
> ＝誰も検証しないので、未検証の契約でメモリ安全チェックを外すことは
> しない。**検査ビルドで最適化が効き、無検査ビルドでは効かない**という
> 通常と逆の向きだが、偽の契約が境界外アクセスに化けないための配置。

---

## 実行モードの切り替え

`INTERPRETER_CONTRACTS` で `requires` / `ensures` を独立に切れる
（`all` / `pre` / `post` / `off`、既定は `all`）。詳細は
[language.md の Runtime gating](language.md#runtime-gating)。

**原則として切らない。** 契約を切るのは「潜在バグがリリースまで生き延びる」
条件そのもので、D の `-release` が批判される理由と同じ。ホットパスの計測など、
意図と範囲を限った例外にとどめる。

なお `panic` / `assert` は**設計上どのモードでも常に有効**で、切る手段は無い。

---

## Tips

**1. 契約は 2 人の間の取り決めとして書く。**
`requires` を読むのは呼び出し側、`ensures` を読むのは実装側。
「実装がこう動くから」ではなく「呼び出し側に何を保証させるか」で
`requires` を決めると、契約が仕様になる。

**2. `requires` を強めると `ensures` が書きやすくなる。**
`requires a % b == 0i64` を足せば `ensures result * b == a` が書ける。
弱い事前条件に無理な事後条件を載せると、たいてい事後条件の方が嘘になる。

**3. `requires b != 0` は「除算が安全」を意味しない。**
符号付き除算には `MIN / -1` があり、これは非ゼロ除数でもトラップする。
どちらも許したくないなら `checked_div`（`core/std/checked.t`）が
`Option::None` を返す形で両方を吸収する。

**4. trait に書いた契約は impl に継承される — ただし引数名を変えないこと。**
`trait` の method シグネチャに書いた `requires` / `ensures` は、その trait
を実装する impl の method に自動で適用される。impl 側でも節を書けば、
**trait のものが先、impl のものが後**という順で AND される（違反時の
`clause #N` もその順）。

契約は引数名を使った式なので、impl が引数名を変えると節が解決できない。
その場合は型エラーで拒否される:

```
[E0010] impl Grow for G: method 'grow' renames parameter `by` to `amount`,
        but Grow declares a contract over `by` — rename the parameter back
        so the trait's `requires` / `ensures` still resolve
```

契約を持たない trait では引数名を自由に変えてよい。

**5. 契約に副作用を書かない。**
実行モードで消える（`off` では評価すらされない）ので、契約の中で状態を
変えると、モードによって挙動が変わるプログラムができあがる。
カウンタの読み出しのような**観測**は安全。

**6. `old` は入口の値。ループの各周回ではない。**
`ensures` はループ不変条件を書く場所ではない（`invariant` は未実装）。
周回ごとの性質を書きたいなら、ループ本体を関数に切り出してその関数に
契約を付ける。

**7. メモリ契約は `retains` と `allocates` を使い分ける。**
「漏らさない」は `retains(0u64)`、「そもそも確保しない」は
`allocates(0u64)`。前者は確保して解放した関数も通す。arena に取る関数を
縛るならバイト数ではなく `allocations(N)` が自然。

**8. `test` ブロックと契約は補完関係。**
契約は**あらゆる呼び出し**を検査し、`test` は**気になる 1 ケース**を
検査する。境界値（0 個、1 個、最大値）は `test` に、不変な性質は契約に。

**9. 契約付きの関数を書いたら 3 つ回す。**
`--check`（反例探し）、`--test`（意図したケース）、
`--all-backends`（3 バックエンド一致）。この 3 つが通って初めて
「契約が正しく、実装が契約を満たし、どのバックエンドでも同じ」が言える。

---

## よくある失敗

**`ensures` の中で `old(...)` を使ったらエラーになる場所がある**

```
[E0010] `old(...)` is only meaningful in an `ensures` clause:
        it snapshots the value an expression had on entry to the function
```

`requires` の中、body の中、`old(old(x))` のネストはいずれも拒否される。
入口の値が要るのは事後条件だけ、という理由。

**`--check` が `0 contracted function(s) checked` と言う**

契約付きの自由関数が無いか、その引数の型を生成できない（`str`・struct・
enum など）。メソッドしか契約が無い場合もこうなる。

**`INCONCLUSIVE` と言われる**

`requires` が狭すぎて生成値が全部弾かれた。テストしたい形なら、
その関数を呼ぶ `test` ブロックを書く方が早い。

**契約は通るのに答えが違う**

契約が弱すぎる。`ensures result >= 0i64` のような節は「間違った正の値」を
全部許す。`--check` が通ったからといって実装が正しいわけではない
（契約が言っていることだけが保証される）。

---

## 現在の制限

- **`invariant`（型・ループの不変条件）は未実装**
- **trait 契約の継承に制限がある** — 引数名が一致する必要があり
  （Tips 4）、trait と impl の**両方**が `old(...)` を使っている場合は
  継承されない（スナップショットは位置で参照されるため、連結すると
  片方の番号がずれる）
- **静的検証は `never_allocates` だけ** — 他の契約は実行時にのみ検査される

- **名前付きタプル返し**（`-> (q: i64, r: i64)`）が無いので、複数の戻り値
  成分に対する事後条件は書きにくい
- `--check` の対象は自由関数のみ、生成できる型は 4 つ（上記）
- **アロケーション契約の節は 3 つ**（`allocates` / `retains` /
  `allocations`）。`free_count` / `realloc_count` / `peak_live_bytes` を
  縛りたいときは生の式で書く

計画は [language.md の Out of scope](language.md#out-of-scope-planned) と
[`design-docs/todo.md`](../design-docs/todo.md) にある。
