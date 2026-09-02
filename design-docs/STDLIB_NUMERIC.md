# STDLIB NUMERIC — 整数の math・ビット演算・限界値・乱数

> 対象: `core/std/math.t` / `core/std/i64.t` / `core/std/f64.t` /
> `core/std/checked.t` と、新設する `core/std/bits.t` /
> `core/std/random.t`
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-NUMERIC**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 実測: 2026-09-03

## Status snapshot

| 項目 | 状態 |
|---|---|
| f64 の初等関数 | `sin` / `cos` / `tan` / `log` / `log2` / `exp` / `floor` / `ceil` / `sqrt` / `fabs` / `pow` (libm へ 11 本) |
| f64 の穴 | `round` / `trunc` / `atan2` / `asin` / `acos` / `log10` / `hypot` / `is_nan` / `is_finite` が無い |
| 整数の math | **`abs(i64)` だけ**。`pow` / `gcd` / `isqrt` / `divmod` が無い |
| ビット演算 | 演算子 (`& | ^ ~ << >>`) はあるが **`popcount` / `leading_zeros` / `rotate` が無い** |
| `min` / `max` | **`i64` と `u64` だけ** (narrow 6 幅と `f64` / `f32` に無い) |
| 限界値 | **書く手段が無い** (`checked.t` は `255u8` を直書きしている) |
| `Checked` | 8 幅すべてに impl 済み (`checked_*` / `saturating_*`) |
| 乱数 | 一様な `u64` と `random_seed` だけ。範囲も分布も無い |
| f32 | **libm ラッパが 1 本も無い** |

## なぜ今これを設計するか

**整数側が空。** `math.t` は f64 の libm ラッパで、整数のために
用意されているのは `abs(i64)` と `min_i64` / `max_i64` / `min_u64` /
`max_u64` の 5 本しかない。この言語で実際に書かれているもの —
ハッシュ表 (`hash_mix` のシフトと乗算)、アロケータのサイズクラス、
SIMD の mask 集計、ビットセット — は**全部ビット演算の上に立つ**のに、
`popcount` も `leading_zeros` も無い。`next_power_of_two` は
`Dict` の表の成長がまさに必要としていて、`dict.t` の中で手書きされている。

そして **`u64::MAX` を書く手段が無い**。`checked.t` が 8 幅ぶんの
限界値をリテラルで直書きしているのは、それしか方法が無かったから。

## 測ったこと (2026-09-03)

1. **`math.t` の中身**: extern 13 本 (f64 の libm 11 + `abs_f64` +
   `abs_i64`)、`pub fn` 15 本。**`min` / `max` は `i64` と `u64` にしか
   無い** — narrow 6 幅と `f64` / `f32` では書けない。

2. **限界値はリテラルの直書き。** `impl Checked for u8` は
   `if self > 255u8 - other` と書いている (`checked.t:67`)。
   associated const も module const (MODULE-CONST) も無いので、
   これが唯一の書き方だった。

3. **IEEE の端は正しく動いている** (interpreter で確認):

   | 式 | 結果 |
   |---|---|
   | `0.0f64 / 0.0f64` | `NaN` (`println` の出力もこの綴り) |
   | `1.0f64 / 0.0f64` | `inf` |
   | `nan == nan` | `false` |
   | `nan < 1.0f64` | `false` |
   | `7.5f64 % 2.0f64` | `1.5` |
   | `-7i64 % 3i64` / `-7i64 / 3i64` | `-1` / `-2` (truncated) |

   `is_nan()` は無いが `x != x` で書ける。**`NaN` / `inf` という綴りは
   JSON に無い**ので、SERIALIZE がこれを扱う (STDLIB_SERIALIZE)。

4. **extern 1 回は interpreter で +6.7 µs、AOT で +5 ns**
   (STDLIB_TIME の実測 3)。同じループの 1 反復が interpreter で約 6 µs
   なので、**ビット演算を toylang のループで書くと 64 反復 ≒ 400 µs** に
   なり、extern 1 本 (6.7 µs) の **60 倍**遅い。ビット演算は extern。

5. **`Checked` が 8 幅すべてに impl されている。** 新しい整数 API も
   **全幅に置く**のが既定 (`Hash` / `Ord` も同じ)。ただし 9 種類の
   ビット演算 × 8 幅を extern にすると 72 本になる — §2 で畳む。

## 既存の決定から引く制約

1. **RUNTIME-TRAP の意味論**: `+` / `*` / 符号付き `-` は **wrap**、
   `u64` の `-` と 0 除算と `MIN / -1` は **trap**。新しい演算も
   **この 2 つのどちらかに寄せる** — 3 つ目の規約を作らない。
2. **`Checked` が逃げ道**。溢れを値で受けたい側は
   `checked_*` / `saturating_*`。新しい演算の checked 版は
   **同じ trait に足す** (別の trait を作らない)。
3. **`Self` を返す trait method は bound 越しに呼べない**
   (STDLIB_TRAIT_BASE 実測 4)。`min` / `max` を `Ord` の default body に
   置くと、**generic 文脈で呼べない method** ができる。→ §3。
4. **MODULE-CONST**: module の `const` は届かない。限界値は `pub fn`。
5. **決定性**: `random_seed(s)` の後の列は 3 バックエンド一致
   (既存の pin)。**分布を足しても同じ列から作る** — 実装が違えば
   同じ seed で違う値が出る。
6. **`--check` のプロパティテスト**が使える分野。`gcd` / `isqrt` /
   `pow` は `requires` / `ensures` で性質が書けるので、
   **オラクルつきで検査できる数少ない stdlib** になる。

## 1. 整数の math

```
pub fn pow_u64(base: u64, exp: u32) -> u64        # wrap (`*` と同じ)
pub fn pow_i64(base: i64, exp: u32) -> i64
pub fn gcd_u64(a: u64, b: u64) -> u64             # gcd(0,0) = 0
pub fn lcm_u64(a: u64, b: u64) -> u64             # wrap
pub fn isqrt_u64(x: u64) -> u64                   # floor(sqrt(x)) 厳密
pub fn div_floor_i64(a: i64, b: i64) -> i64       # 0 除算は trap のまま
pub fn mod_floor_i64(a: i64, b: i64) -> i64       # 常に b と同符号
pub fn sign_i64(x: i64) -> i64                    # -1 / 0 / 1
pub fn midpoint_u64(a: u64, b: u64) -> u64        # 溢れない (a + (b-a)/2)
```

- **溢れは wrap** (制約 1)。`checked_pow` は `Checked` に足す (制約 2)。
- **`isqrt` は f64 経由にしない。** `sqrt(x as f64) as u64` は 2^53 を
  超えると 1 ずれる。整数のニュートン法で書き、**`ensures` で
  `result * result <= x` と `(result+1) * (result+1) > x` を言う**
  (制約 6 — `--check` がオラクルとして使える)。
- **`div_floor` / `mod_floor` を置く理由**: 言語の `/` `%` は
  truncated (実測 3) なので、`-7 / 3 == -2`、`-7 % 3 == -1`。
  ハッシュや円環の添字では floor が要る。**言語の演算子は変えない** —
  名前で区別する。

## 2. ビット演算 — 幅ごとに extern を作らない

**採用: u64 の extern 5 本を土台に、幅の補正は toylang。**

```
extern fn __extern_bits_popcount(x: u64) -> u32
extern fn __extern_bits_clz(x: u64) -> u32       # leading zeros。clz(0) = 64
extern fn __extern_bits_ctz(x: u64) -> u32       # trailing zeros。ctz(0) = 64
extern fn __extern_bits_reverse(x: u64) -> u64
extern fn __extern_bits_swap_bytes(x: u64) -> u64
```

その上に `trait Bits` を 8 幅に impl (`Checked` と同じ形):

```
pub trait Bits {
    fn popcount(self: Self) -> u32
    fn leading_zeros(self: Self) -> u32
    fn trailing_zeros(self: Self) -> u32
    fn rotate_left(self: Self, n: u32) -> Self
    fn rotate_right(self: Self, n: u32) -> Self
    fn reverse_bits(self: Self) -> Self
    fn swap_bytes(self: Self) -> Self
    fn is_power_of_two(self: Self) -> bool
    fn next_power_of_two(self: Self) -> Self
}
```

- **補正は 1 行で済む** — `u8` の `leading_zeros` は
  `clz(x as u64) - 56`、`reverse_bits` は `reverse(x as u64) >> 56`。
  9 種 × 8 幅 = 72 の impl body は書くが、**extern は 5 本**。
- **0 の扱いを決める**: `clz(0) = 64` / `ctz(0) = 64` (幅ごとに補正
  した後はその幅)。Rust と同じで、**未定義にしない**。
- `next_power_of_two(0) = 1`、溢れる入力は **trap ではなく wrap で 0**
  ではなく **panic** — ここだけは `Vec` の範囲外と同じ扱いにする。
  `checked_next_power_of_two` は置かない (要る場面が `Dict` の成長だけで、
  そこでは容量が u64 の半分を超えたら他所が先に壊れている)。
- **IR 命令にはしない (今は)。** cranelift は `popcnt` / `clz` を
  1 命令で持つので AOT では extern 呼び出し (5 ns) が無駄になるが、
  **測ってから昇格する** — SIMD-VM-SLOT が「測って払う」と決めた
  のと同じ規律。extern なら実装は 1 つ (4 レーン共有)、IR 命令にすると
  3 つになる。

## 3. `min` / `max` / `clamp` — free function で全幅に

**`Ord` の default method body には置かない。** 制約 3 の通り、
`fn min(self: Self, other: Self) -> Self` は `Self` を返すので
**generic 文脈から呼べない method** になる (`fn smallest<T: Ord>(v: Vec<T>) -> T`
の中で `a.min(b)` が書けない)。TRAIT_BASE の B1 が landing するまでは、

```
pub fn min_u8(a: u8, b: u8) -> u8      ...  8 幅 + f64 + f32
pub fn clamp_u64(x: u64, lo: u64, hi: u64) -> u64
```

の free function で置く (`min_i64` / `max_i64` が既にその形)。
**B1 の後に `Ord` へ移す**ことを doc に書いておく — 移すときに
free function は残す (呼び出し側を壊さない)。

- **f64 の `min` / `max` は NaN を後ろに落とす** (`min(NaN, 1.0) = 1.0`)。
  IEEE 754 の `minNum` と同じ。`<` だけで書くと NaN が伝播するので、
  ここは明示的に書く。

## 4. 限界値

```
# core/std/limits.t
pub fn u8_max() -> u8      ...  8 幅 × (max, min)
pub fn f64_max() -> f64 / f64_min_positive() / f64_epsilon()
pub fn f64_inf() -> f64 / f64_nan() / f32_inf() / f32_nan()
```

- **`pub fn` なのは MODULE-CONST のため** (制約 4)。poll.t が
  `interest_read()` を関数にしたのと同じ。
- **`checked.t` を書き換えて利用者にする** — 実測 2 の直書きが消えて、
  「限界値の定義が 1 箇所」になる。これが landing の受け入れ条件。
- `f64_nan()` は**リテラルで書けない値**を作る唯一の口になる
  (`0.0/0.0` で作れるが、意図が読めない)。

## 5. f64 の穴

libm へ 7 本足す: `round` / `trunc` / `atan2` / `asin` / `acos` /
`log10` / `hypot`。純 toylang で 3 つ: `is_nan(x) = x != x` /
`is_infinite` / `is_finite`。

- **`round` は libm の `round` (半分は 0 から遠い方)**。
  「銀行家丸め」は入れない — 2 つあると呼び出し側がどちらか分からない。
- `fmod` は演算子 `%` が既にある (実測 3) ので足さない。

## 6. f32

**`f32` には libm ラッパが 1 本も無い。** SIMD-F32 が型と演算子を
入れたところで止まっている。`sinf` / `sqrtf` / `fabsf` / `powf` …の
7 本を `f32` 版として足す。

- **`f64` に上げて計算して落とす、はしない。** `sqrtf(x)` と
  `sqrt(x as f64) as f32` は最後の 1 bit が違いうる。SIMD の
  `f32x4` を使うコードが scalar 版と食い違うのは避ける。
- 併せて **format spec (`{x:.2}`) の f32 未対応** (CLAUDE.md に既載) も
  この Phase で片付ける — 同じ「f32 が後から来たので通っていない経路」。

## 7. 乱数

```
# core/std/random.t
pub fn random_u64() -> u64                       # 既存 io::random() の別名
pub fn random_range(lo: u64, hi: u64) -> u64     # 半開 [lo, hi)、偏り無し
pub fn random_i64_range(lo: i64, hi: i64) -> i64
pub fn random_f64() -> f64                       # [0, 1)、53 bit
pub fn random_bool() -> bool
pub fn random_normal() -> f64                    # 平均 0 分散 1 (Box-Muller)
pub fn shuffle<T>(v: &mut Vec<T>)                # Fisher-Yates
```

- **範囲は棄却法で偏りを消す** (`random() % n` は n が 2 の冪でない
  限り偏る)。`hi <= lo` は panic。
- **すべて `random_u64()` の上の純 toylang** (制約 5)。同じ seed から
  同じ列が出ることを 3 レーンで pin する。`random_normal` を extern に
  すると libm の実装差で値が割れる。
- **`shuffle` は `&mut Vec<T>` を取る。** TRAIT_BASE の実測 3
  (`&mut T` が単一化できない) に**当たるかどうかは着手時に測る** —
  外側が具体型 (`Vec<T>`) なので通る見込みだが、通らなければ
  `impl<T> Vec<T> { fn shuffle(&mut self) }` に置き換える (その場合
  `vec.t` が `random` に依存するので、依存の向きを先に決める)。
- **暗号用途に使えないと書く。** `random()` は再現可能な PRNG で、
  seed は時刻と pid。CSPRNG は非目標。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **N0** | `limits.t` (§4) + `checked.t` を利用者に書き換え | 4 レーン一致。直書きリテラルが消えること |
| **N1** | `bits.t` — extern 5 本 + `trait Bits` 8 幅 (§2) | 4 レーン一致。0 と最大値の端を全幅で pin |
| **N2** | 整数 math (§1) | 4 レーン一致 + `--check` で `isqrt` / `gcd` の `ensures` が通ること |
| **N3** | `min` / `max` / `clamp` 全幅 (§3) | 4 レーン一致。f64 の NaN 規約も |
| **N4** | f64 の 7 本 + 分類 3 本 (§5) | 4 レーン一致 (値はホストの libm 依存なので、端の値だけ pin) |
| **N5** | f32 の libm 7 本 + format spec (§6) | 同上 + `{x:.2}` が f32 で通ること |
| **N6** | `random.t` (§7) | 同じ seed の列が 3 レーン一致、範囲の一様性は統計ではなく**棄却法の実装**を pin |

N0 が先頭なのは、**他の全部が限界値を使う**から (`next_power_of_two` の
溢れ判定も `saturating_*` も)。N1 は `Dict` の手書き
`next_power_of_two` を置き換えられるので、landing の効果が測れる。

## 非目標

- **多倍長整数 (bignum)** — 確保と `Drop` を持つ数値型で、`Vec<u64>` の
  上に書けば済む。stdlib に入れるかは、書いてみてから。
- **有理数 / 10 進小数 (decimal)** — 金額の計算が要るときに考える。
  今は「無い」と書いておく方が誠実。
- **複素数 / 行列 / 線形代数** — SIMD の上に載る話で、`Vec` と
  `Column<T>` が既に土台を持っている。分野として別。
- **統計 (平均・分散・分位数)** — `Vec<f64>` の上に純 toylang で
  書けるので、必要になった人が書く。
- **暗号品質の乱数 (CSPRNG)** — `getrandom` / `arc4random` の extern を
  足すこと自体は簡単だが、**「安全な乱数」を名乗る API は、それを
  使ったコードの安全性を保証したことになる**。この言語がその責任を
  取れるようになるまで置かない。
- **`u128` / `i128`** — 幅を足すと NUM-W の 8 幅すべての表
  (`Checked` / `Hash` / `Ord` / `Bits`) が 10 幅になる。要る場面が
  出てから。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — 分野の俯瞰
- [`STDLIB_TIME.md`](STDLIB_TIME.md) — extern 1 回のコスト (実測 4 の出典)
- [`STDLIB_TRAIT_BASE.md`](STDLIB_TRAIT_BASE.md) — `min` / `max` を
  `Ord` に置けない理由 (制約 3)
- [`STDLIB_SERIALIZE.md`](STDLIB_SERIALIZE.md) — `NaN` / `inf` の綴りと
  JSON の扱い (実測 3)
- [`SIMD.md`](SIMD.md) — f32 と lane 単位の演算 (§6)
- [`COLLECTIONS.md`](COLLECTIONS.md) — `next_power_of_two` の利用者
