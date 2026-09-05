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
toy test  poc/logsearch          # test ブロックを走らせる (まだ 0 件)
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
    poc/logsearch/main.t --release -o /tmp/logread
```

### ビルド時に出る警告

```
warning: `decode` is defined in core/std/base64.t and core/std/hex.t and
         poc/logsearch/src/lsz.t
  a bare call takes the last one; qualify it to be explicit
```

bare な関数名は**全モジュールで 1 つの名前空間**を共有する。`toy` は
根を組み立てる時点で重複を見つけて先に言う (処理系は呼び出しに到達して
から `[E0010]` を出す)。**後の根が勝つ**ので `lsz::decode` は自分の
実装に解決されるが、曖昧なままにせず修飾するのが正しい。

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
logsearch query /tmp/arc "status=404 limit=10"          # フィールド指定
logsearch query /tmp/arc "timeout from=-6h"             # 部分一致 + 時刻
logsearch query /tmp/arc "top=status"                   # 値の分布
logsearch query /tmp/arc "ip=127.0.0.1 top=path"        # traversal
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
