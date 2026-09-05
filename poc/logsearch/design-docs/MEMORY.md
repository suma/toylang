# MEMORY — 返ってこないヒープの上で常駐する

## 1. 事実

`toylang_rt` のヒープは **bump アロケータ**で、`free` は番地を再利用しない。
`toy_dispatched_free` のコメントがそう明言している:

> No libc free: the bump region never reuses an address, so a later
> drop-glue visit of this block reads its original contents.

これは drop glue の冪等性 (別名・コピー・共有された箱を二重解放しない) を
買うための設計で、**バッチ実行のプログラムには正しい**取引である。
しかし常駐サーバにとっては次を意味する:

> **プロセス生涯の確保総量が、そのまま最大 RSS になる。**

`Arena::reset()` も `FixedBuffer::reset()` も**この事実を変えない** —
どちらも中で `__builtin_heap_free` を呼ぶだけで、番地は戻ってこない
(`core/std/allocator.t`)。`FixedBuffer` は「割り当ててよい総量の上限」
を課す**クォータ**であって、再利用する領域ではない。

したがって設計上の要件はこうなる。

> **定常状態では、1 リクエスト / 1 レコードあたりの確保回数が 0 でなければ
> ならない。** 起動時に確保し、以後は同じバッファを使い回す。

1 レコードあたり 1 回の確保 (32 バイト) でも、5,000 行/秒なら 1 日で
13 GB になる。「小さいから大丈夫」は成り立たない。

## 2. 何を起動時に確保するか

```
接続テーブル       128 × (64 KiB 受信 + 256 KiB 送信)      = 40 MiB
アクティブセグメント 本文アリーナ 8 MiB + レコード列 2 MiB   = 10 MiB
セグメント書き出し   圧縮出力 1 MiB + ハッシュ表 128 KiB     =  1.2 MiB
セグメント読み出し   1 フレーム 256 KiB + 展開先アリーナ 8 MiB  =  8.3 MiB
索引構築            語辞書 4 MiB + 転置ビルダ 8 MiB          = 12 MiB
クエリ (4 本同時)   top-k 1000 件 × 4 + 作業領域            =  4 MiB
カタログ            10 万セグメント × 64 B                   =  6.4 MiB
──────────────────────────────────────────────────────────────
合計                                                          ≈ 91 MiB
```

目標の 256 MiB に対して余裕がある。**この表が予算表であり、
超えるものを足すときは何かを削る**。

> **読み出しの行が縮んだのは 2026-09-05** (`fs::File`)。それまでは
> セグメントを**丸ごと**読むしかなく、走査は `.dat` と `.idx` に
> 32 MiB ずつのバッファを先に取り、その上で**セグメントごとに**展開先の
> アリーナを新しく確保していた — この処理系は解放したバイトを再利用
> しないので、それはコーパスに比例して伸びる常駐だった。今はどちらも
> 走査全体で 1 組で、`status=404` (4 セグメント / 30.5 MB 展開) の
> peak live は **22.7 MB** (`TOY_PROFILE_MEM=1` の実測)。

## 3. 定常状態の規律 (5 か条)

### D1. バッファは所有者が 1 つ、起動時に確保、以後 `clear()`

`Vec::clear` は長さを 0 にするだけで**バッファを保つ** (`core/std/collections/vec.t`)。
`Vec::with_capacity` を起動時に呼び、以後は `clear()` → 詰め直しを繰り返す。
容量を超えないよう、詰める側が上限を検査する。

> **`push` は `never_allocates` を通らない。** 検査は**到達可能性**なので、
> 容量が足りていて実際には伸びない呼び出しでも拒否される。実測:
> ```
> [E0016] `poke` is declared `never_allocates`, but it can reach the
>         allocator: poke -> push -> __builtin_heap_realloc
> ```
> 同じ関数を `v.set(i, b)` で書けば通る。したがって**確保しないと約束する層は
> `Vec::set` / `Span<u8>` 経由で書き、`push` は確保してよい層でしか使わない**。
> これは窮屈だが、D1 が求める「容量は事前に決まっている」という性質と
> 一致してもいる ([`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) の G11)。

```rust
# The one place a request body is staged. Allocated once at startup;
# every request clears and refills it. `push` past `cap` would
# realloc — which on this runtime means a permanent leak — so the
# caller checks first and answers 413 instead.
struct RequestBuf { bytes: Vec<u8>, cap: u64 }
```

### D2. リクエストごとに `String` を作らない

文字列補間 (`"{a}={b}"`)、`concat`、`substring`、`split` は**すべて確保する**。
応答の組み立ては `ByteWriter` (固定 `Vec<u8>` への追記) で行う。

```rust
# Everything the response needs to say goes through here. No String,
# no interpolation: those allocate, and on this runtime allocation is
# forever.
impl ByteWriter {
    fn put_str(&mut self, s: str)
    fn put_u64(&mut self, v: u64)
    fn put_ts(&mut self, secs: u64)      # writes YYYY-MM-DDTHH:MM:SSZ
    fn put_json_escaped(&mut self, src: Span<u8>)
}
```

`io::strftime` は `str` を返す = 確保する。**時刻の整形は自前で書く**
(UTC 固定なので、閏秒を無視すれば単純な除算で足りる)。

### D3. レコードは文字列を持たない

[`DATA_MODEL.md`](DATA_MODEL.md) §1 の通り、ラベルは辞書の符号、本文は
アリーナ内のオフセット。`Dict<String, Vec<u64>>` のような構造を
**1 レコードごとに触らない** (辞書はセグメントごとに 1 回だけ作る)。

### D4. 可変長には必ず上限がある

| 対象 | 上限 | 超えたら |
|---|---|---|
| 1 レコード | 64 KiB | 切って `truncated` ラベル |
| 1 要求 | 1 MiB | `413` |
| 1 応答 | 256 KiB × ページ | ページングに倒す |
| セグメント | 8 MiB | フラッシュ |
| 語彙 | 65,536 語 / セグメント | 以降は索引に入れない (本文走査には残る) |
| 同時クエリ | 4 | `503` + `Retry-After` |

**上限の無い入力は、返らないヒープでは即座に致命傷になる。**

### D4b. `fs` / `time` / `json` の確保する API を hot path に置かない

`fs` / `time` / `json` は便利だが、**確保するものが多い**:
`fs::list_dir` は `Vec<String>` を丸ごと作り、`path::join` /
`fs::realpath` / `time::format` は `String` を返し、`json` のリーダは
木を作る。**どれも起動・フラッシュ・GC の側に置く** — 取り込みと
クエリの内側では使わない。`time::now_mono_ns` / `now_unix_ns` /
`sleep_ms` はスカラーを返すだけなので hot path でも安全。

### D5. 確保は「起動時」と「セグメント境界」にだけ許す

例外は 2 つだけ:

1. 起動時の一括確保
2. カタログの成長 (セグメント 1 本につき 64 バイト。1 日 400 本で 25 KB/日)。
   **保持期限で頭打ちになる** (`fs::remove_file` があるので、
   カタログから外した分は実際に消える)

2 は原理的に増え続けるが、**増加率がデータ量に比例して十分小さい**ので
許容する。100 万セグメント (数年ぶん) で 64 MB。

## 4. 契約で縛る

toylang には**確保の契約**がある。設計をコメントではなくコードで守れる。

```rust
# The hot path must not allocate. `never_allocates` makes that a
# compile-time property: if a callee anywhere below can allocate,
# this refuses to build ([E0016] names the path that reaches it).
never_allocates fn append_record(seg: &mut ActiveSegment, rec: &ParsedLine) -> bool {
    ...
}

# The flush path may allocate, but a bounded amount, and it must not
# retain any of it past the call.
fn flush_segment(seg: &mut ActiveSegment, m: &mut Mount) -> bool
    ensures retains(0u64)
{
    ...
}
```

- **`never_allocates`** — 取り込みの内側 (行分割、ラベル抽出、
  アクティブセグメントへの追記)、HTTP のパース、クエリの述語評価
- **`ensures retains(0u64)`** — フラッシュ、索引構築、クエリ 1 ステップ。
  作業領域を使ってよいが、呼び出しの前後で live が増えないこと
- **`ensures allocates(N)`** — 起動時の確保に予算を書く

`never_allocates` は**クロージャ・`dyn`・`extern` 経由を追えないので拒否する**。
取り込みの内側にこれらを置けないという制約になるが、逆に言えば
**「ここに間接呼び出しを持ち込むな」を型検査に言わせられる**。

## 5. 測る

### 開発中

```bash
# 1 リクエストあたりの確保をゼロにできたかを見る
./target/release/compiler --core-modules poc/logsearch/build/root \
    poc/logsearch/main.t --profile=mem --profile-format=json -o /tmp/logsearchd
TOY_PROFILE_MEM=json /tmp/logsearchd --config bench.conf
```

`leaks` が空でも安心しない。**ここで見るのは `cumulative_bytes`** —
「確保して free した」も RSS には効いてしまうため、リークではなく
**累計**が定常負荷で伸びないことを確認する。

### 実行中

`/v1/stats` の `memory` セクションが同じカウンタを出す
(`__builtin_live_bytes()` などはプロファイルフラグ無しで読める)。
**運用中のサーバが自分の定常性を報告する**のが、この設計での「健康」の定義。

### 回帰テストで縛る

```rust
test "steady state: 10k requests allocate nothing after warmup" {
    val server = harness::start()
    harness::drive(server, 1000u64)             # warm up
    val before = __builtin_cumulative_bytes()
    harness::drive(server, 10000u64)
    val after = __builtin_cumulative_bytes()
    assert_eq(after, before)
}
```

**これが ROADMAP M2 の完了条件そのものである。** 落ちないことではなく、
増えないことを CI が見る。

## 6. この制約が消えたら

`design-docs/todo.md` に「アドレスを再利用するアロケータ」の項目は無い
(bump + 冪等 free は drop glue の前提になっている)。仮に将来
**再利用するアロケータ**が入っても、上の規律はほぼそのまま残す価値がある —
リクエストごとに確保しないサーバは、どのランタイムでも速い。

変わるのは D4 の厳しさと、`Arena::reset()` が本当に効くようになること
(セグメント構築のような「作って捨てる」単位が素直に書けるようになる)。
そのときに書き直す箇所を減らすため、**確保する場所は
`segbuild` / `server` / `query` の 3 モジュールの入口に閉じ込めておく**。
