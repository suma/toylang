# ARCHITECTURE — プロセス構成とモジュール分割

## 1. 全体像

`logsearch` は **1 プロセス・1 スレッド**である。toylang にスレッドが無い
(`design-docs/todo.md` の CONCURRENCY は「検討中」) のが直接の理由だが、
制約から出発した割に筋は悪くない: 収集も索引作成も検索も同じデータ構造を
触るので、ロックを設計しなくて済む代わりに**時間を分け合う規律**だけを
設計すればよくなる。

```
                    ┌──────────────────────── logsearchd (1 process, 1 thread) ─────────────────────────┐
   syslog/TCP ──────▶ ingest listener ─┐                                                                 │
   HTTP POST  ──────▶ http listener ───┼─▶ Poller::wait ─▶ ready 事象を 1 つずつ処理                      │
   HTTP GET   ──────▶                  │        │                                                        │
                                       │        ├─ ingest conn  : 受信 → 行分割 → parse → active segment  │
                                       │        ├─ http conn    : 要求読み → ルーティング → 応答書き      │
                                       │        └─ tick (timeout): フラッシュ判定 / 圧縮 / タイムアウト回収 │
                                       │                                                                 │
                                       │   active segment (8 MiB, メモリ)                                │
                                       │        │ 満杯 or 60 秒                                           │
                                       │        ▼                                                        │
                                       │   segment builder ─▶ フレーム圧縮 + 索引作成                     │
                                       │        │                                                        │
                                       │        ▼  tmp/ へ File::write (フレームごと) + sync            │
                                       │   placement ─▶ mount を 1 つ選ぶ                                 │
                                       │        │                                                        │
                                       │        ▼  fs::rename 1 回 (.seg が公開の瞬間)                    │
                                       │   catalog ジャーナルへ追記 ─▶ メモリ上の目録を更新               │
                                       └─────────────────────────────────────────────────────────────────┘
                                                 │
   検索                                          ▼
   GET /v1/query ─▶ plan ─▶ catalog で枝刈り ─▶ 索引 ─▶ フレーム展開 ─▶ 突き合わせ ─▶ top-k ─▶ NDJSON
```

**ブロックする場所は `Poller::wait` の 1 箇所だけ**である。ソケットは
生まれつき非ブロッキング (`core/std/net.t` の規約) で、ファイル I/O は
poller に載らないので、**ファイルを触る操作はループを止める**。これが
フレームのサイズ (256 KiB) を決めている: 1 回の読み書きが数十 ms を
超えないこと。**セグメントのサイズ (8 MiB) はもう I/O の単位ではない** —
`File::read_at` が入ってから、読みも書きもフレーム単位である
([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §0)。

## 2. イベントループ

```
loop {
    val budget_ms = 次のフラッシュ期限までの残り (最大 TICK_MS = 250ms)
    val n = poller.wait(budget_ms)?          # ここだけがブロックする
    for i in 0..n { dispatch(poller.event(i)) }
    tick(io::now())                          # 期限を過ぎたものだけが動く
}
```

`dispatch` は 1 事象あたりの仕事に上限を持つ。

| 事象 | 1 回でやること | 上限 |
|---|---|---|
| listener readable | `accept` を繰り返し、`WouldBlock` で止める | 32 接続 / 回 |
| ingest conn readable | 受信バッファへ 1 回 `read`、完全な行だけを取り込む | 64 KiB / 回 |
| http conn readable | 要求を組み立て、完成したらルーティング | 64 KiB / 回 |
| http conn writable | 応答バッファから 1 回 `write`。残りは次回 | 256 KiB / 回 |
| クエリ実行中の conn | **クエリ状態機械を 1 ステップ進める** | 1 フレーム / 回 |

最後の行が単一スレッド設計の要点である。100 セグメントを舐めるクエリを
1 回のディスパッチで最後まで走らせると、その間 ingest が全部詰まる。
クエリは「セグメント 1 つ・フレーム 1 つ進めては戻ってくる」再開可能な
状態機械にする ([`QUERY.md`](QUERY.md) §5)。

**`WouldBlock` は失敗ではない。** `net.t` が最初に言っていることであり、
このループの平常状態でもある。`NetError` を握り潰さないために、
接続を落とす判断は `dispatch` の 1 箇所に集める。

## 3. モジュール分割

toylang のモジュールは**ファイル階層から経路が決まり、別名は最後の
セグメント**になる (`docs/language.md` の Modules)。したがって
ファイル名がそのまま呼び出し側の綴りになる — `segment::flush(...)`。

2026-09-24 時点で**すべて実装済み** (設計だけの行は無い)。

```
poc/logsearch/
  main.t                  # エントリ。10 のサブコマンド (archive / query /
                          #   fields / object / verify / catalog / compact /
                          #   retain / serve / scan)
  design-docs/            # この設計文書一式
  log/                    # 読ませる実ログ (git 管理外)
  tests/                  # `toy test` が拾う結合テスト (+ golden/)
  src/
    line.t                # 行分割 (Line / LineScan、Span<u8> の上)
    logdir.t              # ログファイルの再帰探索 (.gz などは除外)
    reader.t              # 1 ファイルを使い回しバッファへ読む
    record.t              # 行の framing (形 `LineShape`・時刻・host/tag・
                          #   ラベル・本文)
    bytes.t               # ByteWriter / ByteReader (LE・varint・SIMD コピー)
    crc.t                 # CRC-32 (表は起動時に作る)
    lsz.t                 # LSZ1 圧縮 (LZ77、SIMD 化済み)
    segfile.t             # `.seg` のファイル層 (ヘッダ / セクション表
                          #   `Section` / read_at / write_at / フレーム展開)
    archive.t             # セグメントの書き出し・検証、索引 (語彙・
                          #   postings・リンク)
    extract.t             # フィールド抽出 (apache 2 書式 / KEY=value)
    search.t              # 部分一致検索 (SIMD、スカラー参照つき)
    query.t               # クエリのパースと実行 (`Field` / `OutputFormat`)
    labels.t              # ラベル辞書 (どのキー・値をマウントが持つか)
    catalog.t             # カタログのスナップショット / ジャーナル /
                          #   --repair (`RowKind` / `JournalOp` /
                          #   `RemovalReason`)
    compact.t             # コンパクション (冷えたセグメント群 → 1 アーカイブ)
    store.t               # マウント横断の目録。枝刈りと配置
    mount.t               # マウントの宣言・選択・容量計上 (`MountState`)
    http.t                # HTTP/1.1 の最小パーサとレスポンス組み立て
                          #   (`Method`)
    ui.t                  # 検索 UI の HTML (raw 文字列リテラル 1 つ)
    server.t              # poller、接続テーブル、ディスパッチ、統計
```

タグは enum、サイズ・上限・番兵は `const` で持つ (2026-09-24 に数を返す
0 引数関数 74 本から移した — RUNTIME_GAPS.md G19)。ディスクに出る番号は
明示の discriminant で固定し、読み戻しは `segfile::section_of` /
`catalog::journal_op_of` の 1 か所ずつが担う。

> 以前ここには `bytes.t` / `crc.t` / `lsz.t` / `record.t` / `query.t` が
> **実装済みの行と設計だけの行の両方に**並んでいた。書いた順に足して
> 消し忘れたもので、実体は上の 1 つずつしかない。`index.t` も同じ理由で
> 消した — 語彙・postings・リンクは `archive.t` が持っている。

**分割の基準は「どのバッファを所有するか」**である。`segbuild` は書き込み
バッファを、`server` は接続バッファを、`query` は結果バッファを持ち、
互いのバッファには触らない。[`MEMORY.md`](MEMORY.md) の定常状態は、
所有者が 1 つずつであることに完全に依存している。

### 依存の向き

**各ファイルの先頭に `import` 行で宣言してある** (2026-09-05)。今の形:

```
main    → std.fs std.io std.parse std.time | archive extract logdir query record segfile
archive → std.fs                           | extract record segfile
query   → std.parse std.time               | archive logdir search segfile
logdir  → std.fs std.path
reader  → std.io
extract →                                  | record
segfile →                                  | lsz
bytes crc line lsz record search →         (依存なし)
```

循環は無い。ただしそれは**設計上の選択**であって、処理系の制約では
ない — 相互に `import` し合う 2 モジュールは普通に動く (2026-09-05 に
確認。統合後は 1 つの `File` に畳まれるので初期化順という概念が無い)。
以前ここには「toylang は循環を検出して落とす」と書いてあったが、
それは `import` で個別に読み込む経路の話で、auto-load される
このパッケージには掛かっていない。

**`import` を書く基準は「`mod::` と修飾して呼ぶか」**である。toylang の
`import` が今束縛するのは**モジュールの別名だけ**で、型は
`Vec` / `String` / `Span` / `Dict` のようにグローバルに居る
(本体側 [`MODULE_IMPORTS.md`](../../../design-docs/MODULE_IMPORTS.md) D1)。
したがって `Dict` を使うだけのファイルに import 行は無い。
**同 P2 (import の推移閉包だけを読み込む) が入ったら、型を使うだけの
ファイルにも行が要る** — そのときこの POC が最初の移行対象になる。

`import` は今のところ**何も禁じない** (書かなくても auto-load で
呼べてしまう) ので、この宣言は**読む人と将来の検査のためのもの**である。
別名 (`import a.b as h`) は使っていない — ファイル名がそのまま
qualifier になる形が既に短い。

**行き先の形**は下で、今あるファイルへの対応は
`segbuild` / `segread` → `archive` + `segfile`、`index` → `archive`、
`json` の読み手 → `core/std/json.t` である。

```
main → config, server
server → http, query, segbuild, catalog, stats
query → catalog, segread, index, json
segbuild → lsz, crc, bytes, labels, index
segread → lsz, crc, bytes
catalog → bytes, crc, fs, path
store → catalog, mount
```

## 4. ビルド

`toy` が組む ([`../../../design-docs/BUILD_TOOL.md`](../../../design-docs/BUILD_TOOL.md))。
パッケージは `main.t` か `src/` を持つディレクトリで、モジュール根は
**stdlib → このパッケージの `src/`** の順に並ぶ (後の根が勝つ)。

```bash
cargo build --release -p toy                      # 処理系 (初回のみ)
./target/release/toy build poc/logsearch --release
./poc/logsearch/build/release/logsearch archive poc/logsearch/log/apache2 /tmp/arc
./poc/logsearch/build/release/logsearch query /tmp/arc "status=404 limit=5"
```

サブコマンドを書かなければ第 1 引数がログディレクトリの `scan` になる
(`logsearch poc/logsearch/log 5` は 5 ファイルだけ framing する)。
一覧は [`../README.md`](../README.md)。

**以前はここに symlink 2 本のモジュール根を作る `refresh.sh` があった。**
`--core-modules` が「追加」ではなく「置き換え」で、自分のモジュールを
指すと stdlib が消えたためで、2026-09-05 に**繰り返し指定可能**になって
理由ごと消えた。道具を使わない形も等価に動く:

```bash
./target/release/compiler --core-modules core --core-modules poc/logsearch/src \
    poc/logsearch/main.t --release -o /tmp/logsearch

# インタプリタ (オラクル。同じ答えを返すが桁で遅い)
./target/release/interpreter --core-modules core --core-modules poc/logsearch/src \
    poc/logsearch/main.t scan poc/logsearch/log 5
```

**エントリ `main.t` が `src/` の外にあるのは、もう要件ではない。**
以前は二重取り込みで複製が top-level `const` を失っていたが
(`Identifier 'BUF_BYTES' not found`)、auto-load が**コンパイル対象と
同じファイルを飛ばす**ようになって解消した。ここで外に置いたままなのは
1 ファイル 1 役割が読みやすいからで、構成上の制約ではない。

**実測**: `poc/logsearch/log` の 30 ファイル・17 MB・
68,557 行を **AOT で 59 ms**。同じ入力の 5 ファイル分で
**AOT 6 ms に対しインタプリタ 7,684 ms** (約 1,300 倍) — 1 バイトずつ
メソッドを呼ぶ形は IR VM では通らない、という目安になる。
両レーンの集計値は完全に一致する。

stdlib の根が `std::math` などの経路を、`src/` の根が `record::` などの
別名を与える。auto-load は根の下の **`.t` を全部**読むので、`import` 行は
1 つも要らない。**bare 名は全部で 1 つの名前空間**なので、`src/lsz.t` に
`decode` を置くと `std/hex.t` の `decode` と衝突する — 後の根が勝つ規則で
自分の実装に解決されるが、`toy` はビルド前に警告を出す。この POC は
stdlib と被る bare 名を**使わない**ことで避けている
(`lsz::decode_frame` / `record::parse_line`)。

> **確認済み**: `--core-modules <root>` 下に置いたユーザモジュールの
> `pub fn` が、インタプリタと AOT の両方から `util::double(21u64)` の形で
> 呼べること、戻り値が `String` の関数も両レーン一致することを実測した
> **プロジェクトルートに `modules/` を作る旧経路は使わない** —
> cwd に依存するのでビルドがどこから起動されたかで壊れる。

`--all-backends` は入力が 1 ファイルの実行を前提にしているので、
サーバ本体には使えない (peer を待つため)。**バックエンド間の一致は
サーバではなく、純関数の層 (`lsz` / `crc` / `bytes` / `record` / `query` の
述語評価) に対して取る** — `compiler/tests/consistency/` と同じ流儀で、
`interpreter/example/` に小さなドライバを置く ([`ROADMAP.md`](ROADMAP.md) §4)。

## 5. 起動と停止

### 起動

1. `--config <path>` を読む (`io::read_file`)。無ければ既定値
2. 各マウントの `logsearch.mount` を確認し、無ければ書く (初期化)
3. 各マウントのカタログ (スナップショット + ジャーナル) を読み、`tmp/` の
   残骸を消す ([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §7)
4. **全バッファをここで確保する** — 以降ヒープは伸びない ([`MEMORY.md`](MEMORY.md))
5. listener を bind して poller に登録し、ループへ入る

**ポート番号は設定に書くが、`0` を書ける。** その場合 `local_port()` で
読み戻して stderr に出す (`net_echo_server.t` と同じ規約)。テストは常に
`0` を使う — 番号を固定するとテストが同時に走れない。

### 停止

`POST /v1/admin/shutdown` か、収集ソケットが全部閉じたうえでの idle 期限切れ。
どちらも次の順で降りる。

1. 新規 accept をやめる
2. アクティブセグメントをフラッシュし、カタログへ追記する
3. 進行中の HTTP 応答を書き切る (最大 5 秒)
4. `io::exit(0)`

> **`io::exit` は `Drop` を走らせない。** ファイルはすべて
> `write_file_bytes` / `append_file_bytes` で閉じた状態なので、
> 落としてよいのは「メモリ上のバッファだけ」であることをこの順序で担保する。
> **シグナルは受け取れない** ので `SIGTERM` での停止は書けない
> ([`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) の G9)。当面は管理エンドポイントが
> 唯一のきれいな止め方になる。

## 6. 障害時のふるまい

| 起きたこと | ふるまい |
|---|---|
| マウントが書けない (`PermissionDenied` / `NotFound`) | そのマウントを **degraded** にして配置対象から外し、`/v1/stats` に出す。他のマウントで継続 |
| 全マウントが degraded | 取り込みを止め、収集ソケットの読み取りを停止して TCP のバックプレッシャに任せる。検索は続く |
| カタログのジャーナル末尾が壊れている | CRC が合う最後まで採用し、その場で新世代へコンパクションする。**カタログはキャッシュ**なので、最悪でも `--repair` で `seg/` から作り直せる |
| セグメントの CRC 不一致 | そのフレームを飛ばし、`/v1/stats` の `corrupt_frames` を増やす。クエリは残りを返す |
| カタログにあってファイルが無い | クエリが触った瞬間に `NotFound` で気づき、`REMOVE` をジャーナルへ書く。起動時に全件 `file_size` はしない (直近 64 個だけ) |
| 確保失敗 (null) | 今日の stdlib は確保失敗を検査していない ([`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) の G10)。**上限つきバッファしか使わない**ことで確保そのものを起こさないのが一次防御 |
| 1 レコードが 64 KiB を超える | その行を切り捨て、`truncated=1` ラベルを付けて取り込む。接続は落とさない |
