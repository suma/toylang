# logsearch の設計文書

なぜこの形なのか、何を採らなかったのか、そして書きながら見つけた言語側の穴。

> 動かし方・サブコマンド・ディレクトリ構成は 1 つ上の
> [`poc/logsearch/README.md`](../README.md) にある。ここは**設計**だけを扱う。

## 文書

| 文書 | 何が書いてあるか |
|---|---|
| [`OVERVIEW.md`](OVERVIEW.md) | 目的・スコープ・非目標・用語 |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | プロセス構成、モジュール分割、ビルド手順 |
| [`DATA_MODEL.md`](DATA_MODEL.md) | レコード / セグメント / マウント / カタログ |
| [`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) | オンディスク形式をバイト単位で。圧縮 `LSZ1` |
| [`ONTOLOGY.md`](ONTOLOGY.md) | 行から実体へ。型付き索引とリンク |
| [`QUERY.md`](QUERY.md) | クエリの構文と実行 |
| [`HTTP_API.md`](HTTP_API.md) | サーバ API と Web UI。冒頭に実装状況 |
| [`MEMORY.md`](MEMORY.md) | **確保したメモリは返ってこない**前提での定常状態設計 |
| [`SIMD.md`](SIMD.md) | どこをベクトル化し、どこをしないか |
| [`RUNTIME_GAPS.md`](RUNTIME_GAPS.md) | 足りない言語機能と、踏んだ不具合 |
| [`ROADMAP.md`](ROADMAP.md) | 段階と完了条件、テスト戦略 |

急ぐなら OVERVIEW → ARCHITECTURE → RUNTIME_GAPS の 3 本で全体像が掴める。

## この設計を貫いている制約

1. **メモリは返ってこない** — ランタイムの heap は bump で、`free` は番地を
   再利用しない。**プロセス生涯の確保総量がそのまま RSS になる**ので、
   「リクエストごとに確保しない」書き方が強制される ([`MEMORY.md`](MEMORY.md))
2. ~~**ファイルは全体しか読めない**~~ — 2026-09-05 に `fs::File` で
   解消した。索引を別ファイルに分けていた理由も、セグメントを 8 MiB に
   抑えていた理由も**これだった**ので、保存層は 1 ファイル (`.seg`) の
   v3 に書き直してある。**`fsync` も同じ日に `File::sync()` で入った**
   ので、公開の `rename` の前に呼んでいる — 「電源断に弱い」も消えた
3. **スレッドが無い** — 収集も索引作成も検索も 1 つの流れに載る。
   長いクエリはループを止めるので、**検索は刻んで走らせる**設計にしてある
4. **`Vec::sort` が挿入ソート** — 数十万件を並べる場面では使えない。
   postings もリンクも**計数ソート**で作る

制約はどれも「今日の toylang の姿」であって永久のものではない。
何を足せば何が楽になるかは [`RUNTIME_GAPS.md`](RUNTIME_GAPS.md)。
**この POC が登録したバグが実際に直った例もある** (同 §Z)。
