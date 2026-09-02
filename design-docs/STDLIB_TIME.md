# STDLIB TIME — 単調時計・sleep・性能カウンタ・暦と日付のパース

> 対象: 新設する `core/std/time.t` と、`core/std/io.t` の `now()` /
> `strftime()`、`toylang_rt` の `strftime_utc` / `civil_from_days`
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-TIME**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P2
> 実測: 2026-09-03 (この文書の数値はすべてこの日・このホストで取った。
> ビルドは debug — この repo の既定)

## Status snapshot

| 項目 | 状態 |
|---|---|
| 壁時計 | `io::now()` — libc `time(2)` の**秒だけ** |
| 単調時計 | **無い** (ベンチも指数バックオフも書けない) |
| sleep | **無い** |
| CPU 時間 | **無い** |
| 日付 → 文字列 | `io::strftime(fmt, secs)` (UTC 固定、`toylang_rt` の Rust 実装) |
| 文字列 → 日付 | **無い** (出力だけあって入力が無い) |
| 暦の計算 | Rust 側に `civil_from_days` / `day_of_year` がある。**toylang からは見えない** |
| 確保カウンタ | ある (`__builtin_live_bytes()` 等 6 種) — 性能計測の半分は既にある |

## なぜ今これを設計するか

**秒しか読めない時計では、この repo が自分でやっている作業が書けない。**
COLLECTIONS が「n 個入れて n 回引く」を測ったのも、SIMD Phase 3 が
kernel の前後を比べたのも、全部シェルの `time` でやっている。
**言語の中から測れない。**

そして時間は「非決定だから後回し」にされてきたが、**非決定なのは
時計を読む部分だけで、暦は完全に純粋**。日付のパース・整形・
`from_unix` / `to_unix` は 4 レーンで pin できる。分野を「非決定だから
テストが弱い」と一括りにすると、pin できる 9 割を落とす。

## 測ったこと (2026-09-03)

1. **今ある時間 API は 2 つだけ。**
   `io::now()` は `extern fn time(t: ptr) -> i64 from "c"`、
   `io::strftime(fmt, secs)` は `toylang_rt::toy_io_strftime`。前者は
   壁時計の**秒**、後者は UTC 固定。他に時間を扱うものは無い。

2. **必要な OS の口はこのホストで全部開いている。** C の probe を
   `cc` でビルドして確認した (`net` の ABI probe と同じ流儀):

   | 呼び出し | 結果 |
   |---|---|
   | `clock_gettime(CLOCK_MONOTONIC)` | ok — `1345159.277795000` (起動からの秒) |
   | `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)` | ok — `0.002414000` |
   | `clock_gettime(CLOCK_REALTIME)` | ok — `1788363812.208365000` |
   | `clock_getres(CLOCK_MONOTONIC)` | ok — **1000 ns** と答える |
   | `nanosleep` | ok |

   Linux も同じ 4 つを持つ。**`mach_absolute_time` / `QueryPerformance
   Counter` のような OS ごとの分岐は要らない** — NET N0 が
   `#[cfg_attr(path)] mod sys` を要したのとは違い、時間は POSIX の
   同じ 1 本で足りる。

3. **extern 呼び出しは interpreter で高い。** 1,000,000 回のループに
   extern を 1 つ足す差分を測った:

   | レーン | ループのみ | + `io::random()` | 1 呼び出しあたり |
   |---|---|---|---|
   | interpreter (IR VM) | 6.30s | 13.05s | **+6.7 µs** |
   | AOT | 0.004s | 0.009s | **+5 ns** |

   同じループの 1 反復 (3 演算) が interpreter で約 6 µs なので、
   **extern 1 回 ≒ toylang の演算 3 個**。これは設計を 2 つ決める:
   **(a) 時計は反復ごとに読まない** (ベンチは N 回のループの外で 2 回
   読む)、**(b) 暦のような十数演算の計算は、toylang で書くより
   extern 1 本の方が interpreter では速い**。

4. **暦は既に Rust 側にあり、しかも 4 レーンで実装は 1 つ。**
   `strftime_utc` は `civil_from_days` / `day_of_year` を持つ
   (`toylang_rt/src/lib.rs`)。そして interpreter の extern registry は
   **同じ関数に委譲している** (`extern_io.rs:765` が
   `toylang_rt::strftime_utc` を呼ぶ)。つまり **extern に置いたものは
   tree-walker から AOT まで実装が 1 つ**で、`Hash for str` /
   `random` が同じ形。

5. **module の top-level `const` は使えない。** poll.t が
   `pub const` ではなく `pub fn interest_read()` を並べているのは、
   「module の const は他モジュールにも**自分の body にも**届かない」
   から (todo MODULE-CONST)。`NS_PER_SEC` の類はすべて `pub fn` にする。

## 既存の決定から引く制約

1. **決定性** (`docs/language.md` の Output 規約)。`strftime` を
   **UTC 固定**にしたのと同じ理由でローカル時刻は入れない。時計の値は
   pin できないので、**pin するのは性質だけ** (単調性・順序・
   round trip)。
2. **文法の判定は toylang 側** — `parse::to_f64` が「変換は extern、
   受理集合の判定は toylang」にした先例 (RUNTIME-LIB P0-B)。理由も
   同じで、`strptime` に投げると受理集合が OS ごとに割れる。
3. **失敗の運び方** (ERROR_MODEL) — 失敗は module ごとの enum、
   共通 trait は置かない。payload を運ぶ extern は「値 + 直後に status
   を読む」ペア形 (RUNTIME-IO)。時間はこの形が要る場面が **1 つも無い**
   (パースは純 toylang、時計は失敗しない)。
4. **契約から時計は読めない** (COMPILE-TIME-EVAL C4、`E0018`)。extern は
   「追えない呼び出し」なので `requires` / `ensures` に書くと警告が出る。
   **これは正しい**ので、doc comment にそう書く。
5. **`never_allocates`** — 時計の読みと sleep は確保しないので宣言できる。
   受信ループが `never_allocates` を名乗れるようにした NET の判断と同じ。

## 1. 単調時計と sleep

```
pub fn now_mono_ns() -> u64      # 単調非減少。起点は未規定
pub fn mono_res_ns() -> u64      # 粒度 (clock_getres。このホストは 1000)
pub fn cpu_time_ns() -> u64      # プロセスの CPU 時間 (user + sys)
pub fn now_unix_ns() -> i64      # 壁時計。UTC、閏秒なし
pub fn sleep_ns(ns: u64)
pub fn sleep_ms(ms: u64)
```

**決めること 4 つ:**

- **起点は未規定**。実測 2 の通り macOS では起動からの経過で、Linux でも
  同様。**差だけが意味を持つ**と doc comment に書く。値を保存したり
  プロセスをまたいで比べたりしてはいけない。
- **単調は「非減少」であって「厳密増加」ではない**。粒度より短い間隔で
  2 回読むと同じ値が返る。テストが `a < b` を要求すると偽陽性で落ちる
  ので、pin するのは `a <= b`。
- **サスペンド中は進まない** (両 OS の `CLOCK_MONOTONIC` がそう)。
  「経過した実時間」が要るなら壁時計を使え、と書く。
- **`sleep` は EINTR で早く戻りうる**。runtime 側で残り時間を見て
  ループする (呼び出し側に再試行を書かせない)。**戻り値は無い** —
  「何 ns 寝たか」を返すと呼び出し側がそれを時計代わりに使い始める。

`io::now()` は残す (壊さない)。`time::now_unix_ns() / 1_000_000_000` と
同じ値を返すことをテストで pin し、doc comment で新しい方へ誘導する。

## 2. 性能カウンタ — 何を数えるか

**採用: 時間 (2 種) + CPU 時間 + 既にある確保カウンタ。ハードウェアの
PMU は数えない。**

| 数える | どこから |
|---|---|
| 実時間 | `now_mono_ns()` |
| CPU 時間 (user+sys) | `cpu_time_ns()` |
| 確保バイト / 回数 / live / peak | **既にある** (`__builtin_*` 6 種) |

**数えない: instructions retired / cache miss / branch miss などの
ハードウェアカウンタ。** 理由は 3 つあって、どれも単独で決定的:

- Linux は `perf_event_open`、macOS は非公開の kperf。**同じコードで
  両方に届く口が無い** — SIMD を 128bit に限ったのと同じ「ホスト依存
  ゼロ」の既定に反する。
- macOS では権限が要る (署名 / root)。ビルドしたバイナリが**環境に
  よって動いたり動かなかったりする** API を stdlib に入れない。
- 数えられても**プロセスの外から測れる** (`perf stat` / `instruments`)。
  言語の中に要るのは「自分のコードの一部分を測る」ことで、それは
  時間と確保で足りる。

## 3. `Stopwatch` と `bench` — 純 toylang

実測 3 (a) の帰結として、**時計を読むのは測る区間の外側だけ**:

```
struct Stopwatch { start_ns: u64 }
impl Stopwatch {
    fn start() -> Self
    fn elapsed_ns(&self) -> u64
    fn restart(&mut self) -> u64     # 経過を返して 0 に戻す
}

struct Bench { iters: u64, total_ns: u64, cpu_ns: u64, bytes: u64 }
pub fn bench(iters: u64, f: fn () -> ()) -> Bench
```

- `bench` は **時計を 2 回、確保カウンタを 2 回**読むだけ。1 反復あたりの
  値は割り算で出す (`Bench` に `per_iter_ns()` を置く)。
- **最小値は取らない。** 最小を取るには反復ごとに時計を読む必要があり、
  interpreter ではその読み (6.7 µs) が測る対象より大きくなる。
  合計と反復数を返して、**外れ値の扱いは呼び出し側に任せる**。
- **`f` は capture しない `fn () -> ()`。** AOT の closure は scalar
  しか capture できないので、capture する closure を渡すと
  「interpreter で動いて AOT で落ちる」形になる。doc comment で明示し、
  状態は `var` を外に置いて `f` の中から触らない書き方を例示する。
- **`black_box` は無い。** 最適化で消えうる測定は消える。今のところ
  cranelift の opt level はテストで `none`、AOT の既定でも副作用のない
  ループが丸ごと消えることは確認していないが、**保証はしない**と書く。

## 4. 暦 — 実装を 1 つに保つ

実測 4 より、暦は Rust 側にあり全レーンで共有されている。ここに
2 つ目の実装を作らない:

```
extern fn __extern_time_civil_from_days(days: i64) -> i64 from "toylang_rt" as "toy_time_civil_from_days"
extern fn __extern_time_days_from_civil(packed: i64) -> i64 from "toylang_rt" as "toy_time_days_from_civil"
```

- **y/m/d は 1 つの `i64` に詰める**: `y * 65536 + m * 256 + d`。
  toylang 側は `y = p >> 16` (i64 の `>>` は算術シフト)、
  `m = (p >> 8) & 0xFF`、`d = p & 0xFF` で取り出す。負の年でも
  下位 16 bit が非負なので算術シフトが正しく `y` を戻す。
- **なぜ toylang で書き直さないか**: 実測 3 (b)。civil ↔ days は
  十数演算あり、interpreter では extern 1 回 (6.7 µs) より遅くなる。
  加えて `strftime` と**同じ暦が 2 つ**になる — COLLECTIONS が `Set` と
  `Dict` の「同じ算術が 2 箇所にある」を反復順の pin で縛ったのと同じ
  問題を、わざわざ作ることになる。
- **なぜ全部 extern にしないか**: 文法 (パース) は toylang に置く
  (制約 2)。extern に置くのは**暦の算術だけ**。

その上に純 toylang で:

```
struct DateTime { year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32, nanos: u32 }
impl DateTime {
    fn from_unix(secs: i64) -> DateTime
    fn to_unix(&self) -> i64
    fn weekday(&self) -> u32          # 0 = Sunday (strftime の %w と同じ)
    fn day_of_year(&self) -> u32
    fn is_valid(&self) -> bool
}
```

**閏秒は無い** (Unix 時間の定義どおり)。**タイムゾーンは持たない** —
`DateTime` は常に UTC。オフセットはパースで畳む (§5)。

## 5. 日付のパース — ISO 8601 だけ

```
fn parse_iso8601(s: str) -> Result<DateTime, TimeError>
```

**受理する形 (これで全部):**

```
YYYY-MM-DD
YYYY-MM-DDTHH:MM:SS
YYYY-MM-DDTHH:MM:SS.fff      (小数部は 1〜9 桁、ns に丸める)
… に続けて  Z  |  +HH:MM  |  -HH:MM
```

**`strptime` 形式 (`%Y-%m-%d` を実行時に解釈する) は入れない。**
`parse::to_f64` と同じ判断で、format 文字列の解釈を libc に渡すと
受理集合が OS ごとに割れる。加えて `strftime` が**出力**専用の
format を既に持っているので、入力と出力で 2 つの format 言語を
持つことになる。ISO 8601 は 1 つの固定文法で、**曖昧さが無い**。

**厳しい側に倒す** (`parse::to_u64` の既定と同じ):

- 前後の空白を trim しない。
- 桁数は固定 (`2026-9-3` は `Invalid`)。
- 範囲を検査する (`13` 月、`32` 日、`25` 時は `Invalid`)。
  **閏日は暦で検査** (`2025-02-29` は `Invalid`) — §4 の
  `days_from_civil` に通して往復が一致するかを見る。
- オフセットは**その場で UTC に畳む** (`+09:00` なら 9 時間引く)。
  畳んだ後にゾーンの情報は残らない。
- 秒 `60` (閏秒) は `Invalid`。Unix 時間に置き場所が無い。

```
enum TimeError { Empty, Invalid, OutOfRange }
```

`ParseError` (`Empty` / `Invalid` / `Overflow`) と**語彙を揃える**。
ERROR_MODEL の決定どおり共通 trait は置かず、`Display` で
`"invalid date"` 等を出す。

## 6. 整形

```
impl Display for DateTime { fn to_str(&self) -> str }   # ISO 8601 (Z 付き)
fn format(&self, fmt: str) -> String                    # strftime に委譲
```

`to_str` が ISO 8601 を出すので **`parse_iso8601` との round trip が
そのままテストになる** (§8)。任意 format は既にある `strftime` に
委譲する — 2 つ目の format エンジンを書かない。

## 7. 何を `io` に残し、何を `time` に置くか

- **`time.t` (新設)**: 単調時計 / CPU 時間 / sleep / `Stopwatch` /
  `bench` / `DateTime` / パース / 整形。
- **`io.t` (現状維持)**: `now()` / `strftime()` は動くまま。doc comment で
  `time::` へ誘導する。**移さない** — `io::now` を使っている example と
  テストを壊す価値が無く、`f64.t` が `math::fabs` を呼んでいるように
  **stdlib の module 間呼び出しは動く**ので、必要なら薄い転送で済む。

## 8. 非決定をどうテストするか

**pin するもの (4 レーン一致):**

- `DateTime` の全部 — `from_unix` / `to_unix` / パース / 整形。
  **round trip**: `parse_iso8601(dt.to_str()) == dt` を境界値
  (エポック 0 / 閏年の 2/29 / 1970 より前の負の秒 / 年またぎ) で。
- `strftime` との**相互検算**: 同じ秒に対して
  `strftime("%Y-%m-%d", t)` と `DateTime::from_unix(t).to_str()` の
  日付部分が一致すること。§4 が「実装を 1 つに保つ」と言っている
  ことを、テストの側から縛る。
- `TimeError` の網羅 match。

**pin するもの (性質だけ):**

- `now_mono_ns()` を 2 回読んで `a <= b` (§1 の「非減少」)。
- `sleep_ms(50)` の前後で経過が **45 ms 以上** (粒度とスケジューラの
  ぶれに余裕を取る。上限は見ない — CI は止まる)。
- `cpu_time_ns()` が非減少。

**pin しないもの:** 値そのもの、`sleep` の精度の上限、
`mono_res_ns()` の値 (OS が答えるものをそのまま返す)。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **TM0** | `now_mono_ns` / `mono_res_ns` / `sleep_ns` / `sleep_ms` (extern 4 本) | 単調性と sleep 下限の性質 pin。4 レーンで型が通ること |
| **TM1** | `cpu_time_ns` / `now_unix_ns` + `io::now()` との一致 pin | 同上 |
| **TM2** | `Stopwatch` / `bench` (純 toylang) | 3 レーン一致 (値ではなく形)。`bench` の doc に capture 制約 |
| **TM3** | 暦の extern 2 本 + `DateTime` / `from_unix` / `to_unix` | 4 レーン一致 + `strftime` との相互検算 |
| **TM4** | `parse_iso8601` + `TimeError` | 4 レーン一致 + round trip + 不正入力の表 |
| **TM5** | `Display` / `format` / `weekday` / `day_of_year` | 4 レーン一致 |

TM0 と TM1 は同じ 4 箇所 (`.t` の宣言 / `toylang_rt` / `jit.rs` の
シンボル登録 / interpreter の `extern_io.rs`) を触るので 1 セット。
TM3 が TM4 の前提 (閏日の検査に暦が要る)。

## 非目標

- **ハードウェア性能カウンタ** — §2。
- **タイムゾーンデータベース** (`America/New_York` の解釈) — tzdata を
  読む必要があり、`core/std` を純 toylang で書く既定と衝突する。
  オフセット付き ISO 8601 のパース (§5) で、外から来た時刻は扱える。
- **閏秒** — Unix 時間に置き場所が無い (§4)。
- **`strptime` / 実行時 format のパース** — §5。
- **高精度 sleep の保証** — OS のスケジューラ次第。`sleep_ms(1)` が
  1 ms で戻る保証はしない。
- **タイマー / 定期実行** — イベントループの話で、`Poller` の
  `wait(timeout_ms)` が既にその口を持っている (EVENT_POLLING)。
- **`Duration` / `Instant` の型** — `u64` のナノ秒で足りる。型を作ると
  演算子オーバーロードと `Display` と単位変換が付いてきて、得るのは
  「ns と ms を取り違えない」ことだけ。**関数名に単位を入れる**方
  (`sleep_ms` / `elapsed_ns`) で同じ効果を 0 行で得る。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — P2 (時間 / ロギング)
- [`STDLIB_TEXT.md`](STDLIB_TEXT.md) — §5 のパーサが載るバイト走査と
  `AsciiClass` (`digit_value`)
- [`ERROR_MODEL.md`](ERROR_MODEL.md) — `TimeError` の置き方
- [`NETWORK_IO.md`](NETWORK_IO.md) — extern を足す 4 箇所と ABI probe の
  流儀 (実測 2 はその縮小版)
- [`EVENT_POLLING.md`](EVENT_POLLING.md) — timeout はこちらが持つ
- [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md) — `bench` が読む確保カウンタ
