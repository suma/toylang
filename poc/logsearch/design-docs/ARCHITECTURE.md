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

**太字は実装済み。**

```
poc/logsearch/
  main.t                  # **エントリ**。今は「読んで報告する」だけ
  design-docs/            # この設計文書一式
  log/                    # 読ませる実ログ (git 管理外)
  src/
    line.t                # **行分割** (Line / LineScan、Span<u8> の上)
    logdir.t              # **ログファイルの再帰探索** (.gz などは除外)
    reader.t              # **1 ファイルを使い回しバッファへ読む**
    record.t              # **行の framing** (時刻・host/tag・ラベル・本文)
    bytes.t               # **ByteWriter / ByteReader** (LE・varint・SIMD コピー)
    crc.t                 # **CRC-32** (表は起動時に作る)
    lsz.t                 # **LSZ1 圧縮** (LZ77、SIMD 化済み)
    segfile.t             # **`.seg` のファイル層** (ヘッダ / セクション表 /
                          #   read_at / write_at / フレーム展開)
    archive.t             # **セグメントの書き出し・検証**、索引の意味
    extract.t             # **フィールド抽出** (apache 2 書式 / KEY=value)
    #   索引 (語彙・postings・リンク) は archive.t が持つ
    search.t              # **部分一致検索** (SIMD、スカラー参照つき)
    query.t               # **クエリのパースと実行**
    config.t              # 設定ファイルの読み取りと検証
    bytes.t               # ByteReader / ByteWriter (LE の詰め書き / 読み出し)
    crc.t                 # CRC-32 (起動時に表を作る)
    lsz.t                 # 圧縮コーデック LSZ1 (encode / decode)
    record.t              # レコードの表現と行のパース
    labels.t              # ラベル辞書 (str → u32 code)
    index.t               # 語彙索引の構築と参照
    catalog.t             # カタログのスナップショット / ジャーナル / --repair
    store.t               # マウント横断の目録。枝刈りと配置
    mount.t               # マウントの宣言・選択・容量計上
    query.t               # クエリの表現・計画・実行状態機械
    http.t                # HTTP/1.1 の最小パーサとレスポンス組み立て
    json.t                # JSON の**書き手**だけ (読み手は持たない)
    ui.t                  # 検索 UI の HTML (const str)
    server.t              # poller、接続テーブル、ディスパッチ
    stats.t               # カウンタと /v1/stats
```

**分割の基準は「どのバッファを所有するか」**である。`segbuild` は書き込み
バッファを、`server` は接続バッファを、`query` は結果バッファを持ち、
互いのバッファには触らない。[`MEMORY.md`](MEMORY.md) の定常状態は、
所有者が 1 つずつであることに完全に依存している。

### 依存の向き

```
main → config, server
server → http, query, segbuild, catalog, stats
query → catalog, segread, index, json
segbuild → lsz, crc, bytes, labels, index
segread → lsz, crc, bytes
catalog → bytes, crc, fs, path
store → catalog, mount
```

循環は無い。toylang のモジュール解決は循環を検出して落とす
(`Circular dependency detected`) ので、これは守らないと動かない規約でもある。

## 4. ビルド

`toy` が組む ([`../../../design-docs/BUILD_TOOL.md`](../../../design-docs/BUILD_TOOL.md))。
パッケージは `main.t` か `src/` を持つディレクトリで、モジュール根は
**stdlib → このパッケージの `src/`** の順に並ぶ (後の根が勝つ)。

```bash
cargo build --release -p toy                      # 処理系 (初回のみ)
./target/release/toy build poc/logsearch --release
./poc/logsearch/build/release/logsearch poc/logsearch/log   # 第 2 引数でファイル数を絞れる
```

**以前はここに symlink 2 本のモジュール根を作る `refresh.sh` があった。**
`--core-modules` が「追加」ではなく「置き換え」で、自分のモジュールを
指すと stdlib が消えたためで、2026-09-05 に**繰り返し指定可能**になって
理由ごと消えた。道具を使わない形も等価に動く:

```bash
./target/release/compiler --core-modules core --core-modules poc/logsearch/src \
    poc/logsearch/main.t --release -o /tmp/logread

# インタプリタ (オラクル。同じ答えを返すが桁で遅い)
./target/release/interpreter --core-modules core --core-modules poc/logsearch/src \
    poc/logsearch/main.t poc/logsearch/log 5
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
1 つも要らない。**bare 名は全部で 1 つの名前空間**なので、`src/lsz.t` の
`decode` と `std/hex.t` の `decode` は衝突する — 後の根が勝つ規則で
自分の実装に解決されるが、`toy` はビルド前に警告を出す。

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
