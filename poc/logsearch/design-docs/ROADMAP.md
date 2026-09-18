# ROADMAP — 何から書くか

## 1. 進め方の原則

- **各マイルストーンは「動くもの」で終わる。** 半端な層を積み上げない
- **完了条件は測れる形で書く。** 「だいたい動く」は完了ではない
- **純関数の層から書く。** `lsz` / `crc` / `bytes` / `record` は
  `--check` と `--all-backends` で最初から縛れる。サーバはそのあと
- 各マイルストーンで踏んだ言語側の穴は
  [`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) に足し、言語側の作業は
  `design-docs/todo.md` に足す (**台帳を二重に持たない**)

## 2. どこまで来たか

**設計した順ではなく、必要になった順に作った。** 最初のマイルストーンの
前に「読む側」が要り、サーバより先に保存と検索が動いた。以下は現状であって、
当初の計画表ではない。**M0〜M6 という番号はもう使っていない** — 他の文書に
残っていた参照は、番号ではなく完了条件の名前で書き直してある。

| | 状態 | 実測 |
|---|---|---|
| **読む** `logdir` / `reader` / `line` / `record` | 動く | 30 ファイル 17 MB を AOT 59 ms で framing |
| **土台** `bytes` / `crc` / `lsz` | 動く | LSZ1 は実ログで 13〜16%、ラウンドトリップは 4 レーン一致 |
| **保存** `archive` (`.seg` v3 / verify) | 動く | 181,519 レコードを 9.12 MB へ 785 ms、検証 251 ms |
| **索引** 語彙 (kind=6) / リンク (kind=7) | 動く | 索引の追加ぶん +20%、`top=status` が 24 ms |
| **検索** `query` / `search` | 動く | `grep` と件数一致、traversal はオラクルと一致 |
| **カタログ / マウント / 保持期限** | 動く | 2 マウント (8M / 32M) に 12 セグメント・444,549 レコードを配置。使用率で 2 本 / 10 本に分かれた |
| **HTTP サーバ / Web UI** | 動く | `/` `/v1/query` (3 形式) `/v1/ingest` `/v1/labels` `/v1/stats` `/healthz` と管理系。1000 件 280 KB の応答が部分書き込みを跨いで届く。同時接続は 1 |
| **テスト** | 140 件 + プロパティ 3 本 | `toy test poc/logsearch` が 0.6 秒 (AOT、キャッシュ有り)。内訳は下記 |

### 次にやるなら

1. **テスト** — 140 件。内訳は `server` 26 / `http` 23 / `lsz` 14 /
   `catalog` 11 / `search_query` 9 / `ontology_index` 9 / `mount` 8 /
   `segment_format` 7 / `query` 7 / `main` 6 / `ontology_extract` 5 /
   `steady` 5 / `streams` 4 / `index_scan` 3 / `catalog_rebuild` 3。
   **`main.t` のサブコマンドも通しで走る** (2026-09-18) — ログを読む →
   セグメントを書く → 検証する → 引く → 台帳を作り直す → 保持期限で
   捨てる、の 1 本道と、「ログが 1 つも無いディレクトリは失敗で返る」。
   加えて **`verify` が壊れたセグメントを見つける**こと (1 バイト
   反転した写しで終了コード 1)、`scan` / `fields` (索引版と全走査版) /
   `object` が通る形と断る形の両方で答えること。
   `test` ブロックが `main.t` に在るのは、サブコマンドが関数であり、
   `toy test` が entry も拾うため。**モジュールもサブコマンドも一通り
   覆えた**ので、次に薄いのは並行 (同時接続が 1 本なので書けない)。
   `tests/lsz.t` (2026-09-17) は LSZ1 のラウンドトリップ・壊れたフレーム・
   エンコーダ出力のゴールデン (`tests/golden/lsz-shape*.lsz`) と、
   `--check` にかけるプロパティ 2 本 (ラウンドトリップ、SIMD とスカラーの
   `match_len` の一致) を持つ。**書いた初日に、途中で切れたフレームを
   デコーダが長さの varint の途中から読み進めるバグを見つけた** —
   契約を切った `--release` ではフレームの外のバイトを読んでいた。
   `tests/index_scan.t` (同日) は §4-3 の「索引と全走査の一致」で、
   **これも初日に 2 件見つけた** — `proto` が索引に一度も書かれておらず
   `proto=HTTP/1.1` が実ログで約 15.7 万行あるのに 0 件だったことと、
   syslog の host と `host=` ラベルが同じ値の行を 2 件と数えていたこと。
   2026-09-18 に §4 の残り 3 つが入った — `.seg` のゴールデンと読み側の
   約束 (`segment_format`)、`search` / `query::search` の答え
   (`search_query`)、定常性 (`steady`)、そしてセグメントのファイルだけ
   から台帳を作り直す経路 (`catalog_rebuild`)。**`steady` は書いた日に
   言語側のバグを 1 つ出した** — tree-walker だけが `str::as_ptr` の
   受け皿を確保カウンタに載せ、解放もしていなかった
   (`design-docs/todo.md` の STR-PTR-UNCOUNTED、2026-09-18 に修正)。
   直ったので、定常性は全レーンで測る。
   その `host` の食い違い (クエリだけが syslog ヘッダと比べていた) も
   2026-09-18 に解消した。`host` は予約ラベルで「送信元ホスト」なので、
   クエリも索引側に揃えた — 実ログで `host=web01` が **0 件 558 ms →
   3 件 32 ms**、`tag=CRON` は件数そのままで 627 ms → 117 ms
   (どちらも索引が 11 セグメントを落とすようになった)。
   `/v1/streams` (2026-09-18) のストリーム表は `tests/streams.t` が
   **集合であること** (ラベルの順に依存しない / 同じ組を 2 回書いても
   1 つ / ラベル無しも 1 つのストリーム / セグメントを跨いで畳める) を
   固め、HTTP の形は `tests/server.t` が見る
2. ~~**語の部分一致 / 前方一致** (`ua~MJ12bot` / `path^/wp-`)~~ — 2026-09-10 に landing
3. ~~**カタログとマウント** — 複数ディレクトリへの配置と保持期限~~ —
   2026-09-11 に landing。`src/catalog.t` / `src/mount.t` と、
   `catalog` / `retain` サブコマンド。**カタログはキャッシュのまま**で、
   無ければディレクトリ走査に落ちるし、`catalog <spec> repair` が
   320 B/セグメントで作り直す。配置は使用率が最小のマウント。
   保持はセグメント単位 (1 行でも新しければ残る)
4. ~~**HTTP サーバ**~~ — 2026-09-11 に landing。`src/http.t` /
   `src/server.t` / `src/ui.t` と `serve`。取り込み (`/v1/ingest`、
   アクティブセグメント、`flush`、停止時の書き出し) も同日に入った。
   `/v1/labels` とラベルの索引も同日。**`/v1/streams` は 2026-09-18 に
   landing** — ラベル集合は語彙索引 (組ごと) では数えられないので、
   書き出し時に**ストリーム表 (kind 9)** を作る形にした。実ログ 12
   セグメントで **+1,102 バイト (0.004%)**、答えは **19 ms**。
   **残りは `/v1/labels` をカタログのラベル辞書から答える形にすること**
   (今はセグメントの語彙セクションを読んでいる)。
   同時接続が 1 なのは言語側の穴で、
   [`HTTP_API.md`](HTTP_API.md) の冒頭に理由を書いた
5. ~~**フレーム単位の選択読み**~~ — 2026-09-11 に landing。効くのは
   選択率ではなく**クラスタ性**だった (下表)

## 3. 依存関係

```
line ─┐
      ├─▶ reader ─▶ record ─▶ extract ─┐
logdir┘                                 ├─▶ archive ─▶ query
bytes ─▶ crc, lsz ─▶ segfile ───────────┘        search ─┘
```

**セクション表を最初に入れておいたのが効いた。** 索引は
「知らない kind は飛ばす」形なので、語彙索引 (kind=6) もリンク (kind=7) も
**形式を変えずに後から足せた**。2 ファイルを 1 ファイルに畳んだ v3
(2026-09-05) でも、変わったのは**セクションがどのファイルに居るか**
だけで、セクションの中身は 1 バイトも変わっていない。

## 4. テスト戦略

toylang の道具をそのまま使う。**新しいテスト基盤は作らない。**

| 層 | 道具 | 何を見るか |
|---|---|---|
| 純関数 | `--check` (プロパティ) | ラウンドトリップ、不変条件。`requires` が入力フィルタ、`ensures` がオラクル |
| 純関数 | `--all-backends` | interpreter / JIT / AOT の **3 レーン**で同じ答え。**圧縮と索引は決定的でなければならない** |
| 単体 | `test "..." { }` + `toy test` | 形式のエンコード/デコード、パース、クエリの述語。**既定は AOT** (出荷するレーンを検査する)、`--backend vm` は全部の失敗を 1 回で報告する |
| 契約 | `test "..." panics "text"` | `requires` 違反がその文言で落ちること |
| メモリ | `test` + `testing::heap_mark` / `assert_no_growth` | 定常性 (「10k 要求で確保が増えない」) |
| 結合 | 同一プロセス内クライアント | サーバを立て、同じプロセスから繋ぐ (`compiler/tests/consistency/net.rs` の流儀) |
| 形式 | `testing::assert_golden` + `toy test --bless` | `.seg` のヘッダ / セクション表 / フレームのバイト列を固定。**形式が黙って変わらないこと** |
| 回帰 | `interpreter/example/` に小さなドライバ | example_consistency が自動で拾う |

> **`--all-backends` の「interpreter」は tree-walker とは限らない。**
> 適格なプログラムでは IR VM が取られ、それは AOT / JIT と同じ
> `compiler_lower` を通る。lowering のバグは 3 レーン揃って通りうるので、
> 独立したオラクルが要る場面は tree-walker (`--check` / consistency
> harness) の側で見る。

### 特に厚くするところ

1. **`lsz` のラウンドトリップ** — 壊れると過去のログが全部読めなくなる。
   ランダム入力、繰り返しだけの入力、非圧縮になる入力を全部通す
   (2026-09-17、`tests/lsz.t`。実ログは使わない — ゴールデンに実在の
   アドレスが入るため)
2. **カタログの復旧** — 「末尾が壊れている」「丸ごと無い」
   (`tests/catalog.t`)、「ファイルだけある」(2026-09-18、
   `tests/catalog_rebuild.t`)
3. **索引と全走査の一致** — 2026-09-17、`tests/index_scan.t`。
   オラクルは `fields ... scan` ではなく**元の行を数え直したもの**で、
   索引と全走査の両方が同じ誤りを持つ形を避けている。**索引のバグは
   「答えが少ない」形で出るので、比較対象が無いと気づけない**
4. **定常性** — 2026-09-18、`tests/steady.t`。クエリ・本文走査・集計・
   セグメント展開・クエリの読み取りを 20〜100 周し、live バイトが戻ることを
   見る。4 レーンすべてで測れる (STR-PTR-UNCOUNTED を直したので)

### 形式のバージョニング

`.seg` のヘッダと `meta/mount.json` はバージョン番号を持つ
([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §3 / §1)。**形式を凍結すると宣言した
あとは、既存データを読めなくする変更は入れない** — 自分のログが
読めなくなるサービスは、そこで信用を失う。形式を変える必要が出たら、
バージョンを上げて**両方読める**ようにする。
(v2 → v3 でマジックを `LSD2` → `LSD3` に上げて互換を捨てたのは、
凍結前の POC だからできたことである。)

## 5. テストの回し方

`toy test` は**パッケージの `tests/*.t` とモジュール内の `test` ブロックの
両方**を拾う。結果を機械で読むときは JSON で出す:

```bash
./target/release/toy test poc/logsearch --format=json --diagnostics=json  # 全部 (既定は AOT)
./target/release/toy test poc/logsearch frame        # テスト名 (ファイル名ではない) で絞る
./target/release/toy test poc/logsearch --backend vm # 失敗を全部まとめて見る
./target/release/toy test poc/logsearch pinned --bless   # ゴールデンを記録し直す
```

**フィルタはテスト名に当たる**ので、`lsz` と書いても `tests/lsz.t` は
選ばれない (0 件で終わる)。

プロパティ (ランダムな形・長さ) は `--check` の担当なので、そちらは
interpreter を直接叩く (ループ予算の都合で入力は 3000 バイトまで、~40 秒):

```bash
./target/release/interpreter --core-modules core --core-modules poc/logsearch/src \
    --check --diagnostics=json poc/logsearch/tests/lsz.t
```

範囲は `requires` ではなく剰余で絞ること。生成器は u64 全域から引くので、
`requires len <= 3000` と書くと大半が捨てられて THIN になる。
