# HTTP_API — サーバインターフェースと Web UI

> **実装状況 (2026-09-11)。** `src/http.t` (プロトコル) と
> `src/server.t` (イベントループ) が入り、`logsearch serve <spec>
> [port]` で上がる。動くのは `GET /healthz` / `GET /v1/query`
> (`format=ndjson|json|text`) / `GET /v1/stats` と、管理系の
> `repair` / `gc` / `flush` / `shutdown`、`POST /v1/ingest`、そして
> `GET /v1/labels`、`GET /` の Web UI (`src/ui.t`、1 ページ 4.2 KB)。
> `GET /v1/streams` は 2026-09-18 に入った (セグメントの
> **ストリーム表 kind 9** を 1 本読むだけで、フレームは展開しない。
> 実ログ 444,549 レコード・12 セグメントに対して **19 ms**)。
> `/v1/labels` は**セグメントの語彙セクションを読んで**答えており、
> §2 が言う「カタログのラベル辞書から答えるのでセグメントを開かない」
> 形ではない (その辞書はまだ無い)。1 セグメントにつき 1 セクション
> 読むので、43,813 語を持つ 4 セグメントで 43 ms。10 万セグメントでは
> 成り立たない。UI の「もっと読む」は
> `next_cursor` がまだ無いので、次のページではなく `limit` を
> 4 倍にして引き直す (上限 1000 で止まる)。
>
> `/v1/query` の応答は**組み上げてから送る**。§2 が ndjson を
> 「段階的に流せる」と書いているのに対し、こちらは本文全体を先に
> 作って `content-length` を必ず書く形にした — keep-alive が
> 長さの正しさに乗っているので、そこを崩さない方を取っている。
> `limit` の上限 1000 はそのぶんの予算で、1000 件で約 280 KB。
>
> **取り込みはマウントのディレクトリが既にある場合だけ有効**になる。
> サーバに自分のマウントを作らせると、spec の打ち間違いが黙って
> 新しい空アーカイブになり、「読めない」と「まだ何も無い」の区別も
> 一緒に失われるため。無い場合は `/v1/ingest` が `503` を返し、
> 起動時に stderr へ 1 行出る。
>
> **同時接続は 2026-09-19 に 1 本から 128 本になった。** 接続表は
> 当初 `Vec<i32>` (番号) だったが、**2026-09-20 に
> `Vec<Option<TcpStream>>` になった** — 空きスロットは `None`、
> 使うときは `borrow` して `&TcpStream` のまま読み書きし、閉じるときは
> `Vec::replace` で所有を取り戻す。ハンドルの容器が持てるように
> なった経緯は [`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) G16 と本体の
> `design-docs/ELEMENT_BORROW.md` にある。

## 1. 何を話すか

**HTTP/1.1 の部分集合**を話す。仕様の全部は要らないし、書けば書くほど
壊れる面が増える。受け入れるのは次だけ:

| 受け入れる | 受け入れない |
|---|---|
| `GET` / `POST` | それ以外は `405` |
| ヘッダ行 (`Name: value`)、最大 32 本、各 1 KiB | 折り返し継続行 (obs-fold) |
| `Content-Length` によるボディ | `Transfer-Encoding: chunked` は `411` |
| `Connection: keep-alive` (既定) / `close` | パイプライン (1 接続 1 要求ずつ処理) |
| `?a=b&c=d` と `%XX` / `+` のデコード | multipart、cookie、認証、圧縮 (`Accept-Encoding` は無視) |

**リクエストの JSON は読まない。** リーダは `core/std/json.t` に
**ある** (2026-09-03、[`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) §Z の G6) が、
この API に JSON が要る場面が無い — 検索条件はクエリ文字列、取り込みは
行の並びである。**応答は JSON を書く** (書き手は `Display` の上に
素直に書ける)。

要求全体の上限は **1 MiB**。超えたら `413` を返して接続を閉じる。
これは礼儀ではなくメモリ規律で、接続バッファは起動時に確保した
固定サイズだからである ([`MEMORY.md`](MEMORY.md) §3)。

## 2. エンドポイント

### `GET /healthz`

```
200 OK
text/plain

ok
```

依存を一切見ない。プロセスが応答できることだけを表す。

### `GET /v1/query`

パラメータと意味は [`QUERY.md`](QUERY.md) §1。

**`format=ndjson` (既定)** — 1 行 1 レコード。ストリーミングに向き、
`limit` が大きくても応答バッファを段階的に流せる。

```
200 OK
content-type: application/x-ndjson
x-logsearch-next-cursor: 1756900000.4821

{"ts":"2026-09-03T11:59:58Z","seq":4819,"labels":{"host":"web01","app":"api","level":"error"},"body":"request timed out after 30s"}
{"ts":"2026-09-03T11:59:57Z","seq":4818,...}
```

**`format=json`** — 統計を含む 1 つのオブジェクト ([`QUERY.md`](QUERY.md) §7)。
UI が使うのはこちら。

**`format=text`** — 元の行に近い形。`grep` に食わせるための出力。

エラー:

| 状況 | コード | 本文 |
|---|---|---|
| パラメータが解釈できない | `400` | `{"error":"bad parameter","detail":"from: not a timestamp"}` |
| `limit > 1000` | `400` | 上限を示す |
| 予算で打ち切り | `200` | `stats.truncated = true` (エラーにはしない) |
| マウントが全部読めない | `503` | `{"error":"no readable mount"}` |

### `POST /v1/ingest`

ボディは**改行区切りの行**。1 行 = 1 レコード。`Content-Type` は見ない。

```
POST /v1/ingest?host=web01&app=api
content-length: 84

2026-09-03T12:00:01Z level=error request timed out after 30s
level=info served 200 in 4ms
```

- クエリ文字列で渡したラベルは**既定値**として全行に付く。行の中の
  `key=value` が勝つ
- 行頭が `YYYY-MM-DDTHH:MM:SSZ` か UNIX 秒なら `ts` として使い、
  そうでなければ受信時刻
- 応答は取り込んだ件数

```
200 OK
{"accepted":2,"rejected":0,"seq_first":4821,"seq_last":4822}
```

**部分成功を返す。** 100 行のうち 3 行が壊れていても 97 行は取り込み、
`rejected` で報告する。全体を失敗にすると、送り手は同じ 100 行を
再送し続けることになる。

### `GET /v1/streams`

観測されているストリーム (ラベル集合) の一覧と件数。UI の補完に使う。
`?limit=` は既定 200、上限 1000 (他のエンドポイントと同じ)。

```
{"streams":[{"labels":{"host":"web01","app":"api","level":"error"},"records":11208,
             "ts_min":"2026-09-03T12:00:01Z","ts_max":"2026-09-04T13:23:34Z"}, ...],
 "distinct":7,"shown":7,"segments":12}
```

**ラベルを持たない行も 1 つのストリーム** (`"labels":{}`) である。
落とすと件数の合計がレコード数と合わなくなり、「取りこぼしたのか、
ラベルが無いのか」が区別できない。件数の多い順に並ぶ。

答えの出どころは**書き出し時に作るストリーム表** (kind 9) で、
語彙索引では代わりにならない — 索引はキーと値の組ごとなので、
「`app=api` と `level=error` を**同時に**持つ行が何件か」は
そこに書かれていない。

### `GET /v1/labels` / `GET /v1/labels?name=host`

ラベルのキー一覧、あるいは指定キーの値一覧。マウントごとのラベル辞書
(`meta/labels.dict`、[`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §6) から
答えるので、セグメントは開かない — 応答の `segments` が 0 なのがその
印である。**辞書を持たないマウント** (辞書が入る前に書かれたもの、
消されたもの) では従来どおりセグメントの語彙セクションを歩く。
`catalog <spec> repair` が辞書を作り直す。

### `GET /v1/stats`

```json
{
  "uptime_s": 86400,
  "ingest": {"records": 4.31e8, "bytes": 8.6e10, "rejected": 12, "rate_1m": 4903},
  "segments": {"live": 411, "pending_delete": 3, "active_records": 20144, "active_bytes": 4194304},
  "mounts": [
    {"path":"/var/log/logsearch/a","state":"active","quota":107374182400,"used":41231237120,"segments":121},
    {"path":"/mnt/disk2/logsearch","state":"degraded","last_error":"permission denied","segments":0}
  ],
  "memory": {"live_bytes": 214958080, "cumulative_bytes": 981467136, "alloc_count": 41233},
  "queries": {"running": 2, "completed": 91204, "truncated": 31, "p50_ms": 22, "p99_ms": 810},
  "corrupt_frames": 0
}
```

> `queries` のレイテンシは `time::now_mono_ns()` で測る (単調時計と
> ナノ秒精度がある)。

`memory` は**確保カウンタ** (`__builtin_live_bytes()` など) をそのまま出す。
`live_bytes` が時間とともに増えていないことが、このサーバの健康の定義である
([`MEMORY.md`](MEMORY.md) §5)。`cumulative_bytes` との差が開き続けるなら、
どこかで「返らないメモリ」を使い続けている。

### 管理系

| エンドポイント | 意味 |
|---|---|
| `POST /v1/admin/flush` | アクティブセグメントを今すぐ書き出す |
| `POST /v1/admin/compact` | 冷えたセグメントの併合を**1 パスぶん**進める (2026-09-21)。答えは `{"merged":3,"records":75,"bytes_in":5023,"bytes_out":2997,"failed":0}` で、`merged` が 0 なら冷えたものが無かったということ。繰り返し呼べば進む |
| `POST /v1/admin/repair` | カタログを捨て、`seg/` を歩いて作り直す ([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §7) |
| `POST /v1/admin/gc` | 保持期限切れの削除を今すぐ 1 巡ぶん進める |
| `POST /v1/admin/shutdown` | きれいに停止する ([`ARCHITECTURE.md`](ARCHITECTURE.md) §5) |

**管理系は既定で `127.0.0.1` からの接続にしか答えない** (`peer_addr()` で
判定)。認証機構が無いので、これが唯一の防御である。設定で
`admin_from = any` にできるが、**その場合は前段に認証を置くこと**を
設定ファイルのコメントに書く。

## 3. Web UI

`GET /` は**1 枚の HTML** を返す。外部 CSS も JS も画像も読まない
(CDN に出ていく常駐サーバは、閉じたネットワークで動かないため)。

```
┌──────────────────────────────────────────────────────────────┐
│ [from ▾] [to ▾]  [label host = web01 ×] [+]   [q: timeout ]🔍│
├──────────────────────────────────────────────────────────────┤
│ 12:00:01  web01 api  ERROR  request timed out after 30s      │
│ 11:59:58  web02 api  WARN   retry 2/3 (upstream slow)        │
│ …                                                             │
│                                     [もっと読む]  412 件 / 180ms │
└──────────────────────────────────────────────────────────────┘
```

- `fetch('/v1/query?...&format=json')` を叩いて描くだけ。状態は URL に持つので、
  **検索結果はそのままリンクとして共有できる**
- 「もっと読む」は `next_cursor` を足して再度叩く
- ラベルの候補は `/v1/labels` から引く
- `stats` を下部に出す ([`QUERY.md`](QUERY.md) §7)。**速い / 遅いの理由が
  UI から見える**ことを設計目標にする

HTML は `src/ui.t` の `const` 文字列として持つ。ファイルから読む案は、
**インストール時にパスがずれると UI だけ 404 になる**ので採らない。
埋め込みなら壊れようが無い。

> **文字列補間との相性に注意**: HTML には `{` が大量に出るので、
> toylang の補間 (`"{x}"`) が誤爆する。UI の文字列に補間は使わず、
> `{{` / `}}` のエスケープが必要な箇所はコード側で連結する。

## 4. 接続の扱い

| 項目 | 値 | 理由 |
|---|---|---|
| 最大同時接続 | 128 (`max_conns`) | 接続テーブルを起動時に静的確保する。受信 8 MiB + 送信 32 MiB = **40 MiB** を起動時に取る (`/v1/stats` の `live_bytes` に出る)。減らす旋は `max_conns` |
| 受信バッファ / 接続 | 64 KiB | 1 レコードの上限と同じ |
| 送信バッファ / 接続 | 256 KiB | 部分書き込みを跨いで保持する |
| アイドルタイムアウト | 60 秒 | `io::now()` の秒精度で足りる粒度 |
| ヘッダ待ちタイムアウト | 10 秒 | slow-loris への最低限の応答 |
| keep-alive | 有効 | 取り込みクライアントが繋ぎっぱなしにするため |

接続テーブルが埋まったら**新規 accept をやめる** (登録解除ではなく、
listener を poller から一時的に外す)。`accept` して即座に閉じるより、
TCP のバックログに待たせる方が、送り手にとって扱いやすい。

**応答がスロットに入らないとき**は、共有の大きいバッファ 1 本を使う。
クエリは数 MB を返しうる一方、それを 128 本ぶん確保するのは無駄なので、
「スロットに入る応答はスロットから、入らない応答は共有バッファから、
ただし同時に 1 本だけ」という形にした。共有バッファが塞がっている間に
届いた要求は捨てずに保留し、次の tick で答える。

**書き込みは常に部分書き込みを想定する。** ソケットは非ブロッキングなので
`write` は `Ok(n < len)` を普通に返す。`WouldBlock` を受けたら
`interest_write()` を足して poller に戻り、書けるようになってから続きを送る。
`net_echo_server.t` は小さな応答しか返さないのでこの経路を踏まないが、
検索結果は数 MB になりうるので**ここは必ず踏む**。
