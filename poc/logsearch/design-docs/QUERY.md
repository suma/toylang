# QUERY — クエリの形と実行

## 1. クエリの形

クエリは**空白区切りの 1 本の文字列**である。コマンドラインと URL の
どちらもそれを渡すからで、`main.t` の `query` サブコマンドが受ける。

```
logsearch query <archive> "status=404 path=/wp-login.php limit=5"
logsearch query <archive> "tag=CRON session from=-6h order=asc"
logsearch query <archive> "ip=127.0.0.1 top=path"
```

トークンは 3 種類に分かれる。

| 種類 | 例 | 解決の仕方 |
|---|---|---|
| **索引の効くフィールド** | `status=404` `ip=1.2.3.4` `path=/x` `method=GET` `vhost=v` `ua=…` `host=h` `tag=CRON` | 語彙索引の postings。**語が無いセグメントは展開しない** ([`ONTOLOGY.md`](ONTOLOGY.md) §4) |
| **クエリの制御** | `from=` `to=` `limit=` `order=` `kind=` `top=` | 下記 |
| **それ以外すべて** | `timeout` `level=error` `SRC=1.2.3.4` | **本文の部分一致** (AND)。キーに見えても索引が知らなければ文字列として探す — 打った人の意図がそれだから |

| 制御 | 意味 |
|---|---|
| `from=` / `to=` | 時刻範囲 (半開)。ISO 8601 / UNIX 秒 / 相対 (`-1h` `-30m` `-2d`)。**UTC 固定** |
| `kind=` | 行の形 (`syslog` / `apache` / `datetime` / `epoch` / `plain`) |
| `limit=` | 表示件数 (既定 20、`limit=0` で件数だけ) |
| `order=` | `desc` (既定) / `asc` |
| `top=<field>` | **行ではなく値を返す**。フィルタと組み合わせると traversal になる |

**条件は「種類を跨いで AND、同じ種類の中は OR」**である。括弧つきの
任意ブール式は採らない — 式パーサと計画器の費用に対し、ログ検索で実際に
使われる形はこの積和に収まる。

**フィールドは値の全体に一致する。** `ua=MJ12bot` は 0 件になり
(索引の語は user agent の全体)、その旨を出力に書く。部分一致で探すなら
`MJ12bot` と書けばよい。

### まだ無いもの

bloom フィルタ、カーソルによるページング (§6 は設計のみ)、NDJSON 出力、
否定と OR、語の前方一致 (`path=/blog*`)。並べ替えは全一致を集めてから
行うので、`max_hits` (20,000) を超えると打ち切りを報告する。

## 2. 実行計画

```
1. 時刻範囲 [from, to) でカタログを枝刈り     → 候補セグメント
2. ラベル条件でさらに枝刈り                     → (セグメントのラベル辞書に無い値は落とす)
3. 語条件で bloom を叩く                        → 語を持たないセグメントを落とす
4. 残ったセグメントを、order の向きに並べる
5. 各セグメントを 1 つずつ開き:
   a. セクション表と語彙索引を読む (数十 KiB、`read_at`)
   b. 語索引で候補レコード ordinal 集合を作る
   c. record table でラベル・時刻・カーソルを判定
   d. 本文が要る条件 (contains) があるフレームだけを展開して判定
   e. 通ったレコードを top-k に入れる
6. k 個埋まり、かつ残りセグメントの ts_max/ts_min が
   top-k の境界を超えられないと分かったら打ち切る
```

**6 の早期打ち切りが効くのは、セグメントを時刻順に処理するからである。**
`order=desc` なら新しいセグメントから開き、`limit` 個埋まった時点で、
それより古いセグメントは「最良でも境界に届かない」ことが `ts_max` から
分かる。ログ検索の 9 割は「最近の N 件」なので、実際にはこの打ち切りが
ほとんどの仕事をする。

## 3. 語索引を使う条件と使わない条件

| 条件 | 索引 | 実装 |
|---|---|---|
| `term=timeout` | ○ | 転置リストを引く |
| `term=time*` | ○ | 語は辞書順なので二分探索 + 前方走査、リストを OR |
| `contains=after 30s` | × | フレームを展開して本文を走査 |
| `label=app:api` | ○ | セグメントのラベル辞書 + ストリーム表 |
| `level>=warn` | ○ | ラベルと同じ (`level` は予約ラベル) |

`contains` の走査は**先頭 + 末尾バイトの 2 マスクでベクトル化する**
(16 MiB あたり 35.7ms → 1.4ms、[`SIMD.md`](SIMD.md) §5)。それでも
`contains` は**索引で候補を絞ったあとの絞り込みにしか使わない**。
`contains` 単独のクエリは全フレーム展開になるので、応答に
`"scanned_bytes"` を載せて、遅い理由が見えるようにする。

**語の切り出しは取り込み時と検索時で同じ関数を使う** (`index::tokenize`)。
別実装にすると「入れたのに引けない」が起き、これはログストアで最も
発見が遅れる種類のバグである。同じ関数であることは、
`--all-backends` の一致検査ではなく**同一性**で担保する。

## 4. 並べ替えと top-k

セグメント内はストリームごとに `(ts, seq)` 昇順のランになっている
([`DATA_MODEL.md`](DATA_MODEL.md) §3) ので、時刻順の出力は
**k-way merge** で作る。`PriorityQueue<T: Ord>` が
そのまま使える。

```rust
# The heap is a *min*-heap and `Ord` carries only `lt`, so a
# descending merge feeds it keys whose `lt` is reversed. The wrapper
# type is the comparator: there is no second type parameter and no
# comparator field to pass around.
struct DescKey { ts: u64, seq: u64 }

impl Ord for DescKey {
    fn lt(&self, other: &Self) -> bool {
        if self.ts != other.ts { return self.ts > other.ts }
        self.seq > other.seq
    }
}
```

top-k も同じ道具で、**k 件の「最悪」を根に持つヒープ**を維持し、
新しい候補が根より良ければ差し替える。`limit=1000` なら常駐 1,000 件で、
ソート対象がいくら増えてもメモリは動かない — [`MEMORY.md`](MEMORY.md) の
「上限つきバッファしか使わない」規律に、並べ替えも従わせる。

> `Vec::sort_by` は comparator を `fn (T, T) -> bool` で取るが、
> **トップレベル関数の名前を値として渡せない** (todo の FN-NAME-AS-VALUE)。
> クロージャリテラルを `val` に束縛してから渡す必要があり、しかも AOT の
> クロージャは**スカラーしか捕捉できない**。`Ord` の `impl` を書く方が
> 制約が少ないので、この設計は**比較器を型で表す**方に倒している。

## 5. 刻んで走らせる — クエリ状態機械

スレッドが無く、`Poller::wait` が唯一のブロック点なので、**クエリを
一息に走らせるとその間サーバが止まる**。100 セグメントを開くクエリは
数秒かかりうるので、これは許容できない。

そこでクエリは**再開可能な状態機械**として持つ。

```rust
enum QueryPhase {
    Plan,                    # catalog を枝刈りして候補列を作る
    OpenSegment(u64),        # ヘッダとセクションを読む
    ScanFrames(u64, u32),    # セグメント seg のフレーム f を処理する
    Finish,                  # top-k を整列して応答バッファへ書く
}
```

1 回のディスパッチで進めるのは**1 フレームぶん**か、フレームを開かない
判定なら**1 セグメントぶん**。進んだら poller に戻る。

これで得られるもの:

- 長いクエリの間も取り込みが動き続ける
- クエリのタイムアウトが**中断できる**もの (状態を捨てるだけ) になる
- 同時に走る複数クエリが自然に交互実行される
- 「1 クエリが使ってよい CPU 予算」を**フレーム数**という観測できる単位で
  設定できる (`max_frames_per_query`、既定 4096)

代償は、クエリの途中に新しいセグメントがフラッシュされうること。
**カーソル `(ts, seq)` を境界にしているので、結果は「その時点までの
一貫したスナップショット」ではなく「境界を跨がない範囲では一貫」**になる。
ログ検索としてはこれで足りる (追記しか起きないので、後から現れるのは
常に新しいレコードで、`order=desc` の 1 ページ目にしか影響しない)。

## 6. ページング

`limit` 件を返し、最後のレコードの `(ts, seq)` を `next_cursor` として返す。
次のページはそれを `cursor` に渡す。

```json
{"records": [...], "next_cursor": "1756900000.4821", "stats": {...}}
```

オフセット方式を採らないのは [`DATA_MODEL.md`](DATA_MODEL.md) §1 の通りで、
**取り込みが進むとオフセットは意味を失う**が `(ts, seq)` は不変だからである。
カーソルは不透明な文字列として扱ってよいが、**中身は読める形**にしておく
(デバッグのとき、カーソルが何を指しているか見えることに価値がある)。

## 7. 応答に載せる統計

```json
"stats": {
  "segments_considered": 412,
  "segments_opened": 7,
  "frames_expanded": 23,
  "records_examined": 190244,
  "records_matched": 200,
  "bytes_scanned": 6029312,
  "elapsed_ms": 180,
  "truncated": false
}
```

**なぜ遅いかがクエリ自身から分かる**ようにする。`segments_opened` が
`considered` に近ければ枝刈りが効いていない (時刻範囲が広すぎるか、
bloom が飽和している)。`frames_expanded` が大きければ `contains` を
`term` に書き換えられないか考える。

`truncated` は予算 (`max_frames_per_query`) で打ち切ったことを表す。
**黙って少ない結果を返さない** — ログ検索で「無い」と「見ていない」を
混同させるのは、この種のサービスで最もまずい嘘である。
