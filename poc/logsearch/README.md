# logsearch

ログを読み、圧縮アーカイブにし、そこへクエリを投げるサービス。**toylang で
書かれている** — 処理系の実アプリケーションのモデルケースであり、
「今日の toylang で本物のプログラムを 1 本書くと何が起きるか」を確かめる場所。

設計と、その過程で見つけた言語側の穴は [`design-docs/`](design-docs/README.md) に。

## 動かす

リポジトリルート (`~/dev/lang`) から:

```bash
# 1. 処理系 (初回のみ / HEAD が進んだら再実行)
cargo build --release -p compiler -p interpreter

# 2. モジュール根 — std とこのプログラムの src を並べた symlink 2 本
#    (初回のみ。src に .t を足しても再実行は要らない)
./poc/logsearch/refresh.sh

# 3. AOT でビルド
./target/release/compiler --core-modules poc/logsearch/build/root \
    poc/logsearch/main.t --release -o /tmp/logread

# 4. 取り込み + 圧縮
/tmp/logread archive poc/logsearch/log/apache2 /tmp/arc

# 5. 検索
/tmp/logread query /tmp/arc "status=404 path=/wp-login.php limit=5"
```

**手順 3 の `--release` を外すと `requires` 契約が検査される。**
`lsz` / `crc` / `bytes` の境界条件がその場で捕まるので、開発中はこちら。
配布時は付ける (契約が消え、境界検査も落ちる)。

## サブコマンド

| | |
|---|---|
| `archive <logdir> <out> [limit]` | ログを読んでセグメントに圧縮する |
| `query <out> "<query>"` | 検索・集計・traversal |
| `fields <out> <field> [limit] [scan]` | 1 フィールドの値分布 (`scan` で索引を使わず全走査) |
| `verify <out>` | 全セグメントを読み戻して CRC を照合 |
| `scan <logdir> [limit]` | 何があるか、どう framing されたかを見るだけ |

### クエリの書き方

空白区切り。**索引が知っているキーはフィルタ、それ以外は本文の部分一致**。

```bash
/tmp/logread query /tmp/arc "status=404 limit=10"          # フィールド指定
/tmp/logread query /tmp/arc "timeout from=-6h"             # 部分一致 + 時刻
/tmp/logread query /tmp/arc "top=status"                   # 値の分布
/tmp/logread query /tmp/arc "ip=127.0.0.1 top=path"        # traversal
```

キーは `status` / `method` / `path` / `ip` / `vhost` / `ua` / `host` / `tag`、
制御は `from` / `to` / `limit` / `order` / `kind` / `top`。
**フィールドは値の全体に一致する** (`ua=MJ12bot` は 0 件 — 部分一致で探すなら
`MJ12bot` と書く)。詳しくは [`design-docs/QUERY.md`](design-docs/QUERY.md)。

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
    query.t           クエリのパースと実行
  design-docs/        設計文書 11 本
  refresh.sh          モジュール根を作る (出力は build/、git 管理外)
  log/                読ませる実ログ (git 管理外)
```

**`main.t` が `src/` の外にあるのは構成上の要件である。** エントリは
コンパイラに「プログラム」として渡すので、同じファイルがモジュール根の下にも
あると **auto-load でもう一度取り込まれ**、その複製は自分の top-level `const` を
持たないまま型検査される (`Identifier 'BUF_BYTES' not found`)。1 ファイルに
1 つの役割を持たせる。

## 今どこまで動くか

| | 状態 |
|---|---|
| 読む (探索 / 読み込み / 行分割 / 5 形式の framing) | 動く |
| 圧縮 (LSZ1 / CRC-32 / フレーム) | 動く |
| 保存 (1 ファイル `.seg` v3、読み戻し検証) | 動く |
| 索引 (型付き語彙索引 / 共起リンク) | 動く |
| 検索 (時刻 / フィールド / 部分一致 / 集計 / traversal) | 動く |
| カタログ・マウント・保持期限 | 設計のみ |
| HTTP サーバと Web UI | 設計のみ |
| **テスト** | **無い** — 次にやること |

実測 (`log/apache2` の 181,519 行 / 30.5 MB、AOT `--release`、2026-09-05):

```
archive   9.12 MB (29%)   785 ms          verify   4 セグメント OK    251 ms
query "status=404"        53,689 件  370 ms   (8.6 MB 読んで 30.5 MB 展開)
query "wp-login.php"         546 件  183 ms
query "top=status"            16 種   24 ms   (語彙セクションだけ読む)
query "ip=X top=path"      1,908 種   35 ms   (フレームを 1 つも展開しない)
query "from=2030-01-01"        0 件    0 ms   (1,280 バイトしか読まない)
```

サイズは本体 4.19 MB + 索引 4.93 MB の合計。v2 は 2 ファイルに
分けていたので、この表の 9.12 MB と v2 の「4.19 MB」は同じものの
数え方が違うだけである ([`design-docs/STORAGE_FORMAT.md`](design-docs/STORAGE_FORMAT.md) §0)。

答えは `grep` / `awk` / Python の厳密パーサと突き合わせて一致を確認している。

## git には入らない

`poc/` はルートの `.gitignore` に入れてある。処理系の履歴に、試作の設計文書と
ソースを混ぜないため。**ここで見つけた言語側の不具合だけが
`design-docs/todo.md` に上がる** — 実際に直った例もある
([`design-docs/RUNTIME_GAPS.md`](design-docs/RUNTIME_GAPS.md) §Z)。
