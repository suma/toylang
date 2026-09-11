# logsearch

ログを読み、圧縮アーカイブにし、そこへクエリを投げるサービス。**toylang で
書かれている** — 処理系の実アプリケーションのモデルケースであり、
「今日の toylang で本物のプログラムを 1 本書くと何が起きるか」を確かめる場所。

設計と、その過程で見つけた言語側の穴は [`design-docs/`](design-docs/README.md) に。

## 動かす

リポジトリルート (`~/dev/lang`) から:

```bash
# 1. 処理系 (初回のみ / HEAD が進んだら再実行)
cargo build --release -p toy

# 2. ビルド -> poc/logsearch/build/release/logsearch
./target/release/toy build poc/logsearch --release

# 3. 取り込み + 圧縮
./poc/logsearch/build/release/logsearch archive poc/logsearch/log/apache2 /tmp/arc

# 4. 検索
./poc/logsearch/build/release/logsearch query /tmp/arc "status=404 path=/wp-login.php limit=5"
```

`toy` はパッケージを**引数のパスから上に歩いて**見つけ (`main.t` か
`src/` を持つ最初のディレクトリ)、モジュール根を stdlib →
`poc/logsearch/src` の順に並べる。**以前ここにあった `refresh.sh` は
消えた** — symlink でモジュール根を手作りしていたのは
`--core-modules` が「置き換え」だったからで、繰り返し指定できるように
なって理由ごと無くなった ([`../../design-docs/BUILD_TOOL.md`](../../design-docs/BUILD_TOOL.md) B0)。

**`--release` を外すと `requires` 契約が検査される。**
`lsz` / `crc` / `bytes` の境界条件がその場で捕まるので、開発中はこちら
(出力は `build/debug/logsearch`)。配布時は付ける (契約が消え、境界検査も落ちる)。

ビルドは **0.22 秒** (5,067 行 + stdlib、warm)。`toy` はリンクキャッシュを
`build/.link/` に置くので、2 回目以降は `cc` の呼び出しも消える。

### そのほかの `toy`

```bash
toy check poc/logsearch          # 型検査だけ (コード生成をしない)
toy test  poc/logsearch -j4      # test ブロックを走らせる (77 件)
toy clean poc/logsearch --all    # build/ とリンクキャッシュを消す
```

`toy run` もあるが、このプログラムは**ビルドして出来たものをパス指定で
呼ぶ**方が扱いやすい (実行のたびにビルド判定を挟まない、引数が
`--` の後ろに埋もれない)。以下の例は
`poc/logsearch/build/release/logsearch` を `logsearch` と略記する。

**道具を使わない経路も等価に動く。** `toy build -v` が実際の呼び出しを
出すので、そのままコピーすれば `compiler` を直接叩ける:

```bash
./target/release/compiler --core-modules core --core-modules poc/logsearch/src \
    poc/logsearch/main.t --release -o /tmp/logsearch
```

### bare 名の衝突を避ける命名

bare な関数名は**全モジュールで 1 つの名前空間**を共有する。`toy` は
根を組み立てる時点で重複を見つけて先に言う (処理系は呼び出しに到達して
から `[E0010]` を出す):

```
warning: `decode` is defined in core/std/base64.t and core/std/hex.t and
         poc/logsearch/src/lsz.t
  a bare call takes the last one; qualify it to be explicit
```

**後の根が勝つ**ので `lsz::decode` は自分の実装に解決されるが、
呼び出し側が修飾していても警告は消えない (衝突は定義側にあるため)。
この POC は**改名で回避する**方針を取っており、2026-09-05 に最後の
2 件を潰した — `lsz::decode` → `lsz::decode_frame`、
`record::parse` → `record::parse_line`。`toy check` は無警告。

この警告が出る仕組みごと消す設計が
[`MODULE_IMPORTS.md`](../../design-docs/MODULE_IMPORTS.md) (明示 import)
で、その先取りとして**各ファイルは自分が `mod::` で呼ぶモジュールを
`import` 行で宣言してある** (2026-09-05)。今の `import` は
**何も禁じない** — 書かなくても auto-load で呼べる — ので、これは
依存を読めるようにするためのもの。宣言した依存は
[`design-docs/ARCHITECTURE.md`](design-docs/ARCHITECTURE.md) §3 に
一覧がある。

## サブコマンド

すべて `logsearch <コマンド> <引数...>`。**引数は位置指定で、フラグは無い** —
省略すると既定値が入り、途中だけ省くことはできない (後ろを指定するなら
前も書く)。終了コードは成功が `0`、失敗が `1`。

| コマンド | 引数 | すること |
|---|---|---|
| `scan` | `<logdir> [limit]` | 何があるか、どう framing されたかを見るだけ |
| `archive` | `<logdir> <spec> [limit]` | 読んで圧縮し、セグメントを置いてカタログへ記録する |
| `query` | `<spec> "<query>"` | 検索・集計・traversal |
| `fields` | `<spec> <field> [limit] [scan]` | 1 フィールドの値分布 |
| `object` | `<spec> "<key>=<value>"` | 1 つの値の件数と初出 / 最終 |
| `verify` | `<spec>` | 全セグメントを読み戻して CRC を照合 |
| `catalog` | `<spec> [list\|repair\|compact]` | カタログの中身 / 作り直し / 世代交代 |
| `retain` | `<spec> [days]` | 保持期限を過ぎたセグメントを消す |
| `serve` | `<spec> [port] [idle]` | HTTP で答える (Web UI つき) |

サブコマンドを書かずにパスだけ渡すと `scan` として扱う
(`logsearch /var/log` = `logsearch scan /var/log`)。

### `<spec>` — どこに置き、どこを読むか

`archive` 以降のコマンドが取る `<spec>` は 2 通りある。

**ディレクトリ 1 つ** — それを 1 マウントとして扱う。ふだんはこれでよい。

```bash
logsearch archive /var/log /tmp/arc
logsearch query   /tmp/arc "status=404 limit=5"
```

**`.conf` で終わるパス** — マウント設定。容量の違うディスクへ分散する
ときに使う ([`design-docs/DATA_MODEL.md`](design-docs/DATA_MODEL.md) §6)。

```
# /etc/logsearch.conf
mount /var/log/logsearch/a  quota=100G
mount /mnt/disk2/logsearch  quota=400G
mount /mnt/disk3/logsearch  quota=400G  readonly
```

- `quota` は**宣言値**であって実際の空きではない (`statfs` が無い)。
  1024 倍の接尾辞 `K` `M` `G` `T` が使える
- 新しいセグメントは**使用率 (`used / quota`) が最小**のマウントへ置く。
  ラウンドロビンではないのは、容量の違うディスクを混ぜるのが普通だから
- `readonly` は読むだけ。`retain` も触らない
- **理解できない行はマウントを作らない**。quota を取り違えたマウントは、
  無いマウントより高くつく

### `archive` — 読んで、圧縮して、置く

```bash
logsearch archive <logdir> <spec> [limit]
```

`limit` は読むファイル数の上限 (既定 1,000,000)。`<logdir>` は再帰的に
歩き、`.log` / `.log.<数字>` だけを拾う (`.gz` は飛ばす)。

```console
$ logsearch archive poc/logsearch/log/apache2 /tmp/arc
archiving poc/logsearch/log/apache2 -> /tmp/arc
  mount /tmp/arc  0 / 1099511627776 B (0 permille)
  /tmp/arc/seg/2015/08/08/000000000001.seg  54241 records  8388680 B -> 2850940 B (33%)
  /tmp/arc/seg/2026/08/04/000000000002.seg  52202 records  8388846 B -> 2658913 B (31%)
  ...
segments         4 (0 failed)
records          181519
arena bytes      30559782
archive bytes    9331872
ratio            30% of raw
elapsed          780 ms
```

セグメントは `<mount>/seg/YYYY/MM/DD/<12 桁>.seg` に置かれ、同時に
そのマウントのカタログへ 1 行追記される。**日付はそのセグメントの
`ts_min`** で、日付を 1 つも持たない行ばかりのセグメントだけが
「書いた日」に入る。

### `query` — 検索

```bash
logsearch query <spec> "<query>"
```

空白区切り。**索引が知っているキーはフィルタ、それ以外は本文の部分一致**。

```bash
logsearch query /tmp/arc "status=404 limit=10"       # 完全一致
logsearch query /tmp/arc "path~/wp- limit=10"        # 値の部分一致
logsearch query /tmp/arc "path^/blog/ limit=10"      # 値の前方一致
logsearch query /tmp/arc "timeout from=-6h"          # 本文の部分一致 + 時刻
logsearch query /tmp/arc "top=status"                # 値の分布
logsearch query /tmp/arc "ip=10.0.0.1 top=path"      # traversal
```

| 書き方 | 意味 |
|---|---|
| `key=value` | **値の全体**に一致 |
| `key~needle` | 値のどこかに `needle` を含む |
| `key^prefix` | 値が `prefix` で始まる |
| `word` | 本文 (元の行) の部分一致 |

キーは `status` / `method` / `path` / `ip` / `vhost` / `ua` / `host` / `tag`。
制御は `from` / `to` / `limit` / `order` / `kind` / `top`。

`from` / `to` は**半開区間**で、4 通りの書き方がある。

```bash
from=2030-01-01              # 日付だけ
from=2030-01-01T00:00:00Z    # ISO 8601 (オフセット可)
from=1893456000              # UNIX 秒
from=-6h                     # 今から遡って (`m` / `h` / `d`)
```

**読めなかった値は「指定なし」になる** (`from=yesterday` や
`from=2030-1-1` は境界を作らない)。落ちた境界は空振りではなく
**全件が返る**ので、時刻で絞ったつもりの件数が合わないときは
まず書き方を疑うこと。`tests/query.t` がこの区別を固定している。

**`=` は値の全体に一致する。** `ua=MJ12bot` は 0 件になる — user agent
文字列の全体ではないため。部分で探すなら `ua~MJ12bot` と書く。
詳しくは [`design-docs/QUERY.md`](design-docs/QUERY.md)。

答えの下に**なぜその時間だったか**が出る。読んだセグメント数・枝刈りの
内訳・展開したフレーム数までが答えの一部である。

```console
segments         4 opened of 12 (0 pruned by time, 8 by the index)
records          53689 examined, 53689 matched
bytes read       9433183 off the disk, 28573696 expanded
frames           109 expanded of 132
shown            3 (limit 3)
elapsed          362 ms
```

### `fields` / `object` — 分布と、1 つの値

```console
$ logsearch fields /tmp/arc status 5
field status (index)
  77578  200
  53689  404
  34810  301
   6105  400
   4979  304

segments         4 with an index
terms            43813 in the dictionary
distinct values  16
elapsed          24 ms
```

第 4 引数に `scan` を書くと索引を使わず全レコードを走査する
(`logsearch fields /tmp/arc status 5 scan`)。索引が出す答えを、
索引が置き換えたものと突き合わせるためにある。

`object` は 1 つの値だけを見る。**`key=value` をちょうど 1 つ**取り、
それ以外のトークンがあると断る (落とした条件は答えに見えてしまうため)。

```console
$ logsearch object /tmp/arc "status=404"
object status:404
  records        53689
  segments       4
  first seen     2015-08-08T23:17:04Z
  last seen      2026-09-04T13:22:59Z
  span           349452355 s
```

### `catalog` / `retain` — 台帳と保持期限

```console
$ logsearch catalog /tmp/arc
/tmp/arc  uuid d2853d3236801a1847bbde6b56a82538
  generation 2, 4 segment(s), 0 journal record(s)
  181519 records, 9331872 bytes
  2015-08-08T21:35:02Z .. 2026-09-04T13:23:27Z
```

| 第 2 引数 | すること |
|---|---|
| (省略) / `list` | 何を持っているか |
| `repair` | `seg/` を歩いてカタログを作り直す |
| `compact` | ジャーナルを畳んで新しい世代を公開する |

**カタログはキャッシュである。** 消しても `repair` が作り直すし、無ければ
どのコマンドもディレクトリ走査に落ちて動く。`repair` は 1 セグメント
あたり 320 バイトしか読まないので、迷ったら走らせてよい。

```console
$ logsearch retain /tmp/arc 0
dropping segments whose last record is before 2026-09-11T10:14:28Z

segments dropped 4
segments kept    0
bytes freed      9331872
```

`days` は既定 14。**セグメント単位でしか消さない** — 1 行でも期限内の
レコードがあればそのセグメントは残る (行単位の削除は無い)。
先にカタログから外し、次にファイルを消し、空になった日 / 月 / 年の
ディレクトリを片付ける。`readonly` のマウントは触らない。

### `serve` — HTTP

```bash
logsearch serve <spec> [port] [idle]
```

`port` は既定 8080 (`0` を渡すと OS が選び、選ばれた番号を stderr に
出す)。`idle` は「誰も繋いでこない時間がこれだけ続いたら終わる」秒数で、
既定の `0` は「止めろと言われるまで」。待ち受けは `127.0.0.1` のみ。

```console
$ logsearch serve /tmp/arc 8080
listening on 127.0.0.1:8080
```

| | |
|---|---|
| `GET /` | Web UI (1 ページ。外部から何も読み込まない) |
| `GET /healthz` | `ok` |
| `GET /v1/query?q=&limit=&format=` | `format` は `ndjson` (既定) / `json` / `text`、`limit` は 1〜1000 |
| `GET /v1/stats` | 稼働時間・マウント・確保カウンタ |
| `POST /v1/admin/repair` | カタログを作り直す |
| `POST /v1/admin/gc?days=N` | 保持期限の掃除を 1 巡 |
| `POST /v1/admin/shutdown` | 止める |

#### `/v1/query` に投げるクエリ

`q` の中身は**コマンドラインの `query` と同じ文字列**である。違うのは
URL エンコードが要ることと、`limit` と `format` を `q` の外から渡すこと
だけ。手で `%3D` を書かずに済むので、例は `--data-urlencode` で示す。

```bash
Q=http://127.0.0.1:8080/v1/query

# 完全一致。`format` の既定は ndjson (1 行 1 レコード)
curl -s --get $Q --data-urlencode 'q=status=404' -d limit=5

# 値の部分一致 / 前方一致
curl -s --get $Q --data-urlencode 'q=path~/wp-'   -d limit=5
curl -s --get $Q --data-urlencode 'q=path^/blog/' -d limit=5

# 本文 (元の行) の部分一致
curl -s --get $Q --data-urlencode 'q=wp-login.php' -d limit=5

# 条件は並べると AND
curl -s --get $Q --data-urlencode 'q=status=404 method=GET' -d limit=5

# 時刻で絞る (半開区間)。効けば `pruned_by_time` に出る
curl -s --get $Q --data-urlencode 'q=status=404 from=2030-01-01' -d limit=5
curl -s --get $Q --data-urlencode 'q=status=404 from=-6h to=-1h'  -d limit=5

# 古い順に。既定は新しい順
curl -s --get $Q --data-urlencode 'q=status=404 order=asc' -d limit=5

# 統計つきの 1 オブジェクト (Web UI が使う形)
curl -s --get $Q --data-urlencode 'q=status=404' -d limit=5 -d format=json

# 元の行に近い形。`grep` に渡すならこれ
curl -s --get $Q --data-urlencode 'q=status=404' -d limit=5 -d format=text
```

`limit` は **URL 側が勝つ** (`q` の中に `limit=99` と書いても、
`-d limit=5` があれば 5)。範囲は 1〜1000 で、外れると `400` が返る。

`format=json` の答えは `records` と `stats` の 2 つを持つ:

```console
$ curl -s --get $Q --data-urlencode 'q=status=404 method=GET' -d limit=2 -d format=json
{"records":[{"ts":"2026-09-04T13:22:59Z","body":"..."}, ...],
 "stats":{"segments_opened":4,"segments_considered":4,
          "pruned_by_time":0,"pruned_by_index":0,
          "records_examined":49923,"records_matched":49923,
          "bytes_read":8592081,"bytes_expanded":28462198,
          "frames_expanded":109,"frames_total":120,
          "shown":2,"truncated":true,"elapsed_ms":364}}
```

**`top=` は HTTP では使えない** — 分布を出す経路がまだコマンドライン側に
しか無いので、`400` で断る。黙って落とすと「全レコードが返ってきた」形に
なり、答えに見えてしまうため。分布が要るなら
`logsearch fields <spec> <field>` を使う。

| 返る `400` | いつ |
|---|---|
| `q: missing` | `q` が無い |
| `limit: a number from 1 to 1000` | 範囲外、または数でない |
| `format: json, ndjson or text` | 知らない形式 |
| `top=: distributions are command-line only for now` | `top=` が入っている |

読めるマウントが 1 つも無いときは `503` (`no readable mount`)。
**要求は正しいのに答えられない**ので `400` ではない。「読めない」は
ディレクトリが存在しない (打ち間違い) か、`.conf` が読めない場合を指す。

**中身が空のアーカイブは 503 ではない。** マウントは開けていて、
セグメントを 1 本も持っていないだけなので `200` と空の結果を返し、
`segments_considered` が `0` になる。Web UI はそれを見て
「まだ何も archive されていない」と書く。引数なしの `serve` は
既定で空の `/tmp/logarchive` を指すので、最初に見るのはたいていこの形。

**管理系は loopback からの接続にしか答えない。** 認証機構が無いので、
これが唯一の防御である。**同時接続は 1 本** — 理由と、それが設計の
どこから外れているかは [`design-docs/HTTP_API.md`](design-docs/HTTP_API.md)
の冒頭にある。

## 構成

```
poc/logsearch/
  main.t              エントリ。サブコマンドの入口
  src/                モジュール (main.t は *入れない* — 下記)
    line.t            行分割 (Line / LineScan)
    logdir.t          ログファイルの再帰探索
    reader.t          1 ファイルを使い回しバッファへ読む
    record.t          行の framing (時刻 / host / tag / ラベル / 本文)
    extract.t         フィールド抽出 (apache 2 書式 / KEY=value)
    bytes.t           ByteWriter / ByteReader (LE・varint・SIMD コピー)
    crc.t             CRC-32
    lsz.t             LSZ1 圧縮 (LZ77、SIMD 化済み)
    segfile.t         .seg のファイル層 (ヘッダ / セクション表 / read_at)
    archive.t         セグメントの書き出し / 検証 / 索引 / リンク
    search.t          部分一致検索 (SIMD、スカラー参照つき)
    query.t           クエリのパースと実行・3 形式の描画
    catalog.t         カタログ (スナップショット / ジャーナル / 再構築)
    mount.t           マウントの宣言・配置ポリシー・`meta/mount.json`
    http.t            話すと決めた HTTP/1.1 の部分集合
    server.t          イベントループと経路
    ui.t              Web UI (1 ページを埋め込みで持つ)
  tests/              `toy test` が走らせる test ブロック (77 件)
  design-docs/        設計文書 11 本 + 目次
  build/              toy の出力 (実行ファイル / リンクキャッシュ、git 管理外)
  log/                読ませる実ログ (git 管理外)
```

**`main.t` が `src/` の外にあるのは、もう要件ではない。** 以前は
エントリが根の下にもあると auto-load で二重に取り込まれ、複製が自分の
top-level `const` を失って `Identifier 'BUF_BYTES' not found` で落ちた。
2026-09-05 に auto-load が**コンパイル対象と同じファイルを飛ばす**ように
なって解消している (`src/main.t` を置いた最小パッケージで確認済み)。
ここで外に置いたままなのは好みの問題 — 1 ファイル 1 役割が読みやすい。

## 今どこまで動くか

| | 状態 |
|---|---|
| 読む (探索 / 読み込み / 行分割 / 5 形式の framing) | 動く |
| 圧縮 (LSZ1 / CRC-32 / フレーム) | 動く |
| 保存 (1 ファイル `.seg` v3、読み戻し検証) | 動く |
| 索引 (型付き語彙索引 / 共起リンク) | 動く |
| 検索 (時刻 / フィールド / 部分一致 / 集計 / traversal) | 動く |
| カタログ・マウント・保持期限 | 動く |
| HTTP サーバと Web UI | 動く (同時接続は 1 本、取り込みは未) |
| **テスト** | 77 件 (`toy test poc/logsearch -j4`) |

実測 (`log/apache2` の 181,519 行 / 30.5 MB、AOT `--release`、2026-09-11):

```
archive   8.90 MB (30%)   780 ms          verify   4 セグメント OK    211 ms
query "status=404"        53,689 件  366 ms   (8.6 MB 読んで 28.5 MB 展開)
query "wp-login.php"         546 件  178 ms
query "top=status"            16 種   24 ms   (語彙セクションだけ読む)
query "ip=<1 つの値> top=path"  1,908 種  35 ms   (フレームを 1 つも展開しない)
query "from=2030-01-01"        0 件    0 ms   (ヘッダだけ読んで 4 本とも捨てる)
```

> traversal の行のアドレスは伏せてある (実ログ由来で、プライベート
> アドレスではないため。CLAUDE.md の規約)。`logsearch fields <spec> ip 1`
> が出す 1 位の値をそのまま入れると再現する。

本体と索引は 1 ファイルに入っている。v2 は 2 ファイルに分けていたので、
この表の 8.90 MB と v2 の「4.19 MB」は同じものの数え方が違うだけである
([`design-docs/STORAGE_FORMAT.md`](design-docs/STORAGE_FORMAT.md) §0)。

答えは `grep` / `awk` / Python の厳密パーサと突き合わせて一致を確認している。

## git に入るもの / 入らないもの

**ソースと設計文書は追跡している。** この POC は、このリポジトリが持つ
**唯一の「言語の利用者」**であり、何を回避しなければならなかったかが
そのまま処理系の穴の一覧になるため。追跡しないのは 2 つだけで、
ルートの `.gitignore` にそう書いてある:

| | 理由 |
|---|---|
| `log/` | 読ませる実ログ (~137 MB)。処理系とは無関係 |
| `build/` | `toy` の出力 (実行ファイル / リンクキャッシュ) |

**言語側の不具合は `design-docs/todo.md` に上げる** (台帳を二重に持たない)。
実際に直った例もある
([`design-docs/RUNTIME_GAPS.md`](design-docs/RUNTIME_GAPS.md) §Z)。
