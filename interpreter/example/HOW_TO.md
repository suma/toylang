# example の動かし方

このディレクトリのプログラムは、原則としてどれも 1 コマンドで走る。

```bash
# インタプリタ
cargo run -q -p interpreter -- interpreter/example/<name>.t

# AOT コンパイル → 実行
cargo run -q -p compiler -- interpreter/example/<name>.t -o /tmp/<name>
/tmp/<name>
```

`compiler/tests/example_consistency.rs` がこのディレクトリの全プログラムを
インタプリタ / JIT / AOT で突き合わせているので、**大半の example は
「置いてあるものは動く」**と考えてよい。例外は同ファイルのリストに理由付きで
挙がっているものだけ — 診断を見せるために**失敗するのが正しい**もの
(`ERROR_EXAMPLES`)、AOT がまだ組めないもの (`AOT_UNSUPPORTED`)、
バックエンドを落とすもの (`KNOWN_CRASHES`、現在は空)、そして
**peer を待つもの** (`NEEDS_A_PEER`)。下の `net_echo_server.t` が最後の
ものにあたる。

はじめの 2 つは**両方向に検査される**ので、直ったのにリストへ残っていても
失敗する。リストは黙らせる場所ではなく、残っている作業の台帳になっている。

---

## `net_echo_server.t` — イベントポーラで駆動する echo サーバ

サーバ**だけ**のプログラム。クライアントは同梱していないので、別の端末から
繋いで動かす。

ブロックするのは `Poller::wait` の 1 箇所だけで、accept も read もそこで
待つ。下にいるのは Linux なら `epoll`、macOS / BSD なら `kqueue` だが、
**ソースにはどちらの名前も出てこない** (`design-docs/EVENT_POLLING.md`)。

### ビルドと実行 (AOT)

```bash
cargo run -q -p compiler -- interpreter/example/net_echo_server.t -o /tmp/net_echo_server
/tmp/net_echo_server
```

最適化して組むなら `--release` を足す。

```bash
cargo run -q -p compiler -- interpreter/example/net_echo_server.t --release -o /tmp/net_echo_server
```

インタプリタで直接動かしてもよい (挙動は同じ)。

```bash
cargo run -q -p interpreter -- interpreter/example/net_echo_server.t
```

### 繋ぐ

起動すると **stderr** に待ち受けポートが出る。

```
listening on 127.0.0.1:55329
waiting up to 30 seconds for a client
```

**ポートは毎回変わる。** OS に空きポート (port 0) を要求して番号を読み戻して
いるので、番号はどこにも埋め込まれていないし、同時に何個立てても衝突しない。
その代わり接続のたびに読み取ること。

別の端末から:

```bash
printf 'hello' | nc 127.0.0.1 55329     # hello がそのまま返る
printf 'quit'  | nc 127.0.0.1 55329     # サーバが止まる
```

サーバ側の **stdout** は、何を返したかの記録になる。

```
served: hello
served: quit
connections served: 2
```

port が stderr で記録が stdout なのは意図的で、**stdout だけ見れば
実行ごとに同じ**になる。`> log.txt` で記録だけ取るのが楽。

### 止め方

3 通りある。

| 方法 | 挙動 |
|---|---|
| `quit` を送る | その接続を返してから正常終了 |
| 何もしない | アイドル予算 (既定 30 秒) を使い切って正常終了 |
| `Ctrl-C` | 即座に終了 |

アイドル予算は引数で変えられる。手で試すなら長め、様子を見るだけなら短めに。

```bash
/tmp/net_echo_server 5      # 5 秒誰も来なければ終了
```

### なぜ自動テストに載っていないか

`example_consistency` はこの example を飛ばす (`NEEDS_A_PEER`)。peer を待つ
プログラムなので、テストで走らせてもアイドル予算を全レーンに足したうえで
「誰も来なかった」ことを確認するだけになる。バックエンドを直せば載る類の
制限ではないので、`ERROR_EXAMPLES` / `AOT_UNSUPPORTED` と違って
**縮んでいくべき負債ではない** — 両方向の検査も掛けていない。

API 自体は覆われている。`compiler/tests/consistency/net.rs` が同じ
`bind` / `accept` / `Poller` の面を 4 レーンで pin していて、そちらは
決定的にするためにクライアントを同じプロセスに置いている。
**test 側が「正しさ」を、この example が「書き方」を持っている**という分担。

### 読みどころ

コード中に「入れ子で書けず束縛した」というコメントが 4 箇所ある。

- 引数位置の associated function call (`String::from_str(...)`)
- 式位置の struct を返す呼び出し (`serve(...)`)
- `match` の scrutinee に置いた呼び出し
- 既存の `String` 束縛への再代入

いずれも**インタプリタでは書ける形が compiled レーンで落ちる**もので、
`design-docs/todo.md` の ENUM-VARIANT-ARG / ENUM-ARG-NEST と同じ族。
実際のコードを書くとどこで刺さるかの記録も兼ねている。
