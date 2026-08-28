# フロントエンドの並列化 検討メモ (PARALLEL-FRONTEND)

**日付**: 2026-08-28
**問い**: 字句・構文解析と意味解析 (型検査) をマルチスレッドで並列化できるか。

**結論**: **できる。しかし今のワークロードでは取り分がほぼ無いので着手しない。**

- **stdlib の pre-parse は既に並列** (rayon 4 スレッド)。実測の取り分は
  **warm cache で 0.2ms / cold で 1.5ms** (1 実行あたり 10〜14ms 中)。
- **AOT の codegen も既に関数単位で並列** (`compiler/src/codegen/mod.rs:164`)。
  5.5k 行のプログラムで wall 212ms / user 323ms。
- 残る候補は (a) **1 ファイル内の並列 parse**、(b) **関数単位の並列型検査**。
  (a) は 22k 行のファイルで **36ms → 9.2ms (3.9x)** を実験で確認したが、
  **マージ費用 (~0.5µs/行) を含めていない**うえ、今の実プログラムは
  数百行なので削れるのは 1ms に満たない。(b) は取り分 (22k 行で 10.7ms)
  より先に**型検査器が AST を書き換えるのをやめる**という大きな前提が要る。

先に効くのは並列化ではなく **1 実行あたりの固定費** の方で、これは
[`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) と
[`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md) が既に扱っている
問題である。本文書はそれを別角度 (並列化) から確認した記録。

その固定費を「ビルドサーバを常駐させて消せないか」という別の提案も
測ったので **§5** に足した。**結論はサーバを作らないこと** — 消したい
7.2ms は常駐プロセスではなく**統合済みスナップショット 1 ファイル**
(cold プロセスで 0.95ms ロード) で落ちるうえ、そちらは 1 テスト 1
プロセスのテスト群にも効く。

---

## 0. 既にある並列化

| 箇所 | 実装 | 粒度 |
|---|---|---|
| stdlib 21 モジュールの pre-parse / キャッシュ読み | `interpreter/src/module_integration.rs::preparse_core_modules` | モジュール |
| AOT の cranelift codegen | `compiler/src/codegen/mod.rs:164` | 関数 |

どちらも rayon で、**プロセスごとに専用の小さいプール**を持つ
(`preparse_pool` / `compiler/src/small_pool.rs`)。プールを小さくしてあるのは
測定の結果で、`preparse_pool` のコメントにあるとおり **20 コアの既定プールは
CPU 46.6ms、4 スレッドなら 36.7ms で wall は同じ**。nextest は 1 テスト 1
プロセスなので、**並列化を足すたびにテスト 1 本あたりの固定費が乗る**。

AST は `Rc` を持つので `!Send`。今は「1 ワーカで作って丸ごと move する」形に
限定し、`unsafe impl Send for SendFile`
(`interpreter/src/module_integration.rs:1532`) で通している。**共有はできない。**

## 1. 測定 (2026-08-28、release、macOS / 20 コア)

フェーズ時間は `check_typing_diagnostics` と `run_source` に一時的な
`Instant` マークを入れて採った (計測コードは revert 済み。再現するなら
`integrate_modules` / `resolve_type_aliases` / `setup_type_checker` /
impl block / ユーザ関数本体 / `check_moves` の各境界に置けばよい)。

### 1a. 小さいプログラム (`fn main() -> u64 { 0u64 }`、warm cache、wall 9.9ms)

| 内訳 | 時間 |
|---|---|
| プロセス起動 (`--explain E0003` の実測がフロア) | 3.7 ms |
| stdlib の preparse + 統合 | 2.5 ms (うち preparse 0.8) |
| impl block の型検査 (stdlib 込み) | 0.86 ms |
| 全プログラム walk (alias / recursive / moves / never_allocates / const fn) | 0.6 ms |
| ユーザ関数の本体検査 | 0.005 ms |

**フロントエンドの 9 割が「ユーザのプログラムと無関係な固定費」**である。
`--core-modules` を空ディレクトリにすると 9.9ms → 4.4ms。

### 1b. stdlib pre-parse の並列 / 逐次 (21 モジュール, 3,379 行)

| | preparse だけ | 実行全体 (wall) |
|---|---|---|
| warm cache・並列 | 0.81 ms | 9.9 ms |
| warm cache・逐次 | 1.55 ms | 10.1 ms |
| cold cache・並列 | 3.3 ms | 12.5 ms |
| cold cache・逐次 | 4.5 ms | 14.0 ms |

cold で 4 スレッドなのに 1.4x しか出ないのは、**モジュールの大きさが揃って
いない**ため。逐次での 1 モジュールあたり: `std.allocator` 1.20ms /
`std.string` 0.73 / `std.collections.vec` 0.62 / …合計 5.07ms。
**最大の 1 本が並列の下限**なので、スレッドを増やしても 1.2ms 以下にはならない。

### 1c. 大きい単一ファイル (自動生成 2000 関数 / 22,004 行)

| フェーズ | 時間 |
|---|---|
| parse (ユーザファイル) | **47.5 ms** |
| └ うち lexing のみ | 7.0 ms (15〜17%) |
| 型検査 合計 | 25.3 ms |
| └ ユーザ関数の本体 | 10.7 ms |
| └ stdlib 統合 | 3.7 ms |
| └ `check_moves` | 2.6 ms |
| └ impl block | 0.87 ms |

parse は行数に対して線形 (1.1k / 5.5k / 22k 行で 2.2 / 2.1 / 2.2 µs/行)。
2026-08-20 に `offset_to_line_col` と `finalize_number_types` の O(n²) を
潰した後の状態 ([`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md) §2)。

**lexer は parse の 1/6** なので、字句解析だけを並列化しても意味が無い。

### 1d. ファイル内 chunk 分割の実験

トップレベル `fn` 境界で分割し、chunk ごとに独立した `ParserWithInterner` で
parse した (`frontend/examples/lexbench.rs` に一時的に置いて計測、削除済み)。
22k 行:

| 分割数 | 逐次 | 並列 (rayon 既定プール) |
|---|---|---|
| 2001 (関数ごと) | 49 ms | **163 ms** |
| 4 | 37 ms | 17 ms |
| 8 | 37 ms | 13 ms |
| 20 | 36 ms | **9.2 ms (3.9x)** |

**細粒度が壊滅する**のは parser インスタンスの固定費 **7.5µs/個**
(interner を新規に作り builtin 名を intern する) と、スレッドを跨いだ
アロケータ競合。粗い分割 (16〜32) なら素直に効く。

**ただしこの数字はマージを含んでいない。** 別々に parse した AST を 1 つの
プログラムに合流させる作業は既に実装があり (`AstIntegrationContext`)、
その実測費用は **~0.5µs/行** (stdlib 3,379 行の統合が ~1.7ms)。22k 行なら
~11ms で、稼いだ 27ms の 4 割が消える。マージが全ノードを deep-copy して
symbol と `ExprRef` / `StmtRef` を貼り替える設計だからで、これは
**並列 parse のために作られたものではない**。

## 2. 何が邪魔をしているか

| 障害 | 場所 | 内容 |
|---|---|---|
| AST が `!Send` | `frontend/src/ast/` (`Rc`) | 丸ごと move はできる (`SendFile` の unsafe wrapper)。共有は不可 |
| interner が 1 個の `&mut` | `DefaultStringInterner` を全経路が引き回す | symbol は添字なので、ワーカごとに interner を持つと**必ず remap が要る** |
| 型検査が AST を書き換える | `type_checker/core.rs:12` が `&mut ExprPool` / `&mut StmtPool` | `?` の脱糖 (`expression.rs:2327`)、`Display` の `to_str` 挿入 (`method_call.rs:221`)、リテラルの具体型書き戻し (`type_conversion.rs:269`)、struct update の展開 (`struct_literal.rs:808`)、closure の共有判定 (`closure_escape.rs:58`) |
| 関数間に依存がある | `expression.rs:1252 type_check_forward_ref` | 呼び出し位置が callee の本体検査を引き起こす。純粋な関数単位の仕事ではない |
| 共有レジストリを検査中も更新する | `type_checker/context.rs` (struct_methods / enum_definitions / trait 表) | 本体検査の前に凍結できるかは未検証 |
| 決定性が pin されている | `compiler/tests/reproducible_build.rs` | 並列化しても診断順・ID 採番・コード生成は決定的でなければならない (link cache が全ミスになる) |
| プールの生成費がプロセスごと | `preparse_pool` / `small_pool.rs` | nextest は 1 テスト 1 プロセス。並列化を足すほどテスト全体の CPU が増える |

**「意味解析 = 読み取り専用の解析」ではない**、というのがいちばん大きい。
型検査器は脱糖器でもあるので、関数単位に割るには先に「検査」と「書き換え」を
分ける必要がある。これは並列化と無関係にも価値のある整理だが、大仕事である。

## 3. やるとしたらこの順序

1. **共有 concurrent interner** (または symbol ID 空間の事前分割)。
   これが無いと 1 ファイル内の並列 parse は必ず remap 費用を払う。
   `.toycache` の schema (`FULL_AST_CACHE_SCHEMA_VERSION`) が intern 順に
   依存している点に注意 — 順序が変わると古いキャッシュが別の意味に化ける。
2. **粗いチャンク分割での並列 parse**。分割は字句レベルの前走査が要る
   (`/* */` / 文字列 / 補間 `{...}` の中の `fn` に騙されないこと)。
   関数ごとの細粒度は 7.5µs の固定費で負ける。
3. **マージを deep-copy ではなく ref の shift だけにする**。1 と組み合わせて
   はじめて 2 の利得が残る。
4. **型検査の並列化は最後**。前提は「検査と書き換えの分離」+「宣言表の凍結」。
   取り分は 22k 行で 10.7ms。

## 4. 着手条件

- 実プログラムで **単一ファイルが 5,000 行を超える**こと (今の最大は
  `core/std/string.t` の 632 行、生成物を除けば全部それ以下)。
- かつ、1 実行あたりの固定費 (§1a の 5.4ms) が先に片付いていること。
  固定費が支配的なうちは、parse を 4x にしても体感は変わらない。

## 5. ビルドサーバ案 (常駐プロセス) — 測って却下

**問い** (2026-08-28): クライアントがビルドリクエストを送る常駐サーバに
すれば、stdlib の構築費用は消えるか。

**結論**: **作らない。** 消したいものは消えるが、同じものが常駐なしで
落ちる。サーバが正当化されるのは LSP / watch のように**同じプロセスが
何度もコンパイルする**用途になってから。

### 5a. 測定 (release, 200 回ループ平均 / プロセス内は 5 回)

| プロセスあたり | ms/run |
|---|---|
| `/usr/bin/true` (spawn の床) | 3.0 |
| 最小 Rust バイナリ (466 KB) | 3.8 |
| `interpreter --explain` (stdlib ロード無し) | 4.2 |
| `interpreter trivial.t` (core dir 空) | 5.0 |
| `interpreter trivial.t` (stdlib あり・warm cache) | **12.2** |

→ **stdlib の固定費は 1 プロセスあたり ~7.2ms**。これがサーバ化の上限。

> §1 の 9.9ms とこの 12.2ms は同じプログラムである。§1 は python から
> 25 回起動した**最小値**、ここは shell ループ 200 回の**平均**で、
> 差は測り方 (外れ値の扱いと起動元) の違い。**比較は同じ表の中でのみ
> 行うこと。**

| 同一プロセス内 (2 回目以降) | ms |
|---|---|
| フロントエンド全体 (stdlib 込み) | 3.4 |
| 同 (core dir 空 = ユーザ分だけ) | 0.12 |
| **統合済み `File` の clone** | **0.15** |
| interner の clone | 0.001 |
| 統合済み状態の bincode 保存 / ロード | 0.70 / 0.85 |
| **同 ロード (cold プロセスで最初の仕事として)** | **0.95** (319 KB) |

**同じプロセスの 2 回目は 12.2ms ではなく 3.4ms** である。7.2ms の半分近くは
「プロセスが冷たいこと」(page cache / malloc の暖機 / rayon プール生成) で、
stdlib の計算そのものではない。

| AOT (trivial program) | ms/run |
|---|---|
| `--emit=ir` | 18.7 |
| `--emit=obj` | 16.4 |
| exe (link cache ヒット) | 26.3 |
| exe (実 `cc`) | **75.9** |

### 5b. サーバが消せるもの / 消せないもの

- **消せる**: stdlib の再構築 7.2ms。サーバ側の 1 リクエストは
  clone 0.15ms + ユーザ分 0.12ms ≈ **0.3ms** になりうる。
- **消せない**: **クライアントのプロセス起動 3〜4ms**。`/usr/bin/true` が
  3.0ms なので、薄いクライアントを書いても下がらない。加えて IPC 往復。
- **消せない**: AOT の `cc` リンク ~50ms。exe ビルドの 2/3 がここで、
  サーバが触れるのは 10% だけ。既存の `TOY_LINK_CACHE_DIR` の方が効く。
- **消せない**: cargo のビルド時間 (クリーン 23.7s / 増分 2.35s、BUILD-PERF)。
  これは sccache の領域で、toylang のビルドサーバとは無関係。

差し引き、CLI 1 回の体感は **12.2ms → ~5ms**。常駐プロセス・プロトコル・
無効化・クラッシュ回復を持ち込む対価としては薄い。

### 5c. 代わりに: 統合済みスナップショット

**stdlib を merge し終えた `File` + interner を 1 ファイルに保存する**と、
cold プロセスで **0.95ms でロードできる** (319 KB、実測)。7.2ms → ~1ms で、
サーバ版の 0.3ms とほぼ同じ土俵に立つ。しかも:

- **テストに効く。** nextest は 1 テスト 1 プロセスなので**サーバは使えない**
  (テストは CLI を経由せずライブラリを直接呼ぶ)。スナップショットは各
  プロセスが読むだけなので効く。todo.md の TEST-PERF が挙げている
  「core module ロードが 1 プロセス 27ms、interpreter の 984 テストが各々
  払う」に直接当たる。
- 無効化は「stdlib 全ソースのハッシュ + schema version」だけ。todo.md が
  モジュール束ね (bundle) を見送った理由 —「invalidation が全モジュール
  単位になる」— は、**対象を stdlib に限れば実質デメリットにならない**。
  stdlib は滅多に変わらず、変わったら全部作り直して構わない。

### 5d. どちらをやるにせよ先に解く点

1. **統合の向きが逆**。今は stdlib を**ユーザプログラムの pool に merge**し、
   先頭 `user_func_count` 個をユーザ分として切り出している
   (`interpreter/src/lib.rs::check_typing_diagnostics`)。スナップショットも
   サーバも「stdlib ベースにユーザを足す」向きに直す必要がある。
2. **型検査器の派生状態**も持ち越さないと impl block の検査 0.86ms と
   `setup_type_checker` が残る。
3. **`Rc` で `!Send`** (§2 と同じ制約)。サーバをマルチスレッドにするなら
   リクエストごとにスレッド固定か、シングルスレッド + キュー。
4. **キャッシュ健全性**。`FULL_AST_CACHE_SCHEMA_VERSION` と intern 順への
   依存はスナップショットでも同じ罠で、面積は広がる。バージョン不一致は
   必ず「黙ってミス扱い」にすること。

### 5e. 推奨順序

1. **統合済みスナップショット** (7.2ms → ~1ms、常駐不要、テストにも効く)。
2. **サーバは LSP / watch とセットで**。同一プロセスの 2 回目以降が
   3.4ms → clone 0.15ms + 差分検査になるので、そこでは桁で効く。
3. **AOT の体感を上げたいなら `cc` から** — link cache の既定 on、
   `-fuse-ld=lld` ([`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md) §4 の
   残件 4)。

## 非目標

- **言語レベルの並行性** (`spawn` / チャネル) — 別の話。todo.md の
  CONCURRENCY を見ること。本文書は**コンパイラ自身の並列化**に限る。
- **`Rc` → `Arc` の全面置換** — 単独では遅くなるだけで、上の 1〜3 を
  やらない限り取り分が無い。
- **lexer 単独の並列化** — parse の 15〜17% しかない。
- **ビルドサーバの常駐化** — §5 のとおり、CLI 用途では取り分が薄い。
  LSP / watch を作るときに、その中身として再検討する。

## 関連

- [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) — 固定費を
  キャッシュで削る側。preparse の deserialize ~1.5ms が残件
- [`SEPARATE_COMPILATION.md`](SEPARATE_COMPILATION.md) — 同じ固定費を
  中間オブジェクトで削る案 (見送り)。frontend の O(n²) 修正の記録も
- [`todo.md`](todo.md) の TEST-PERF — 「1 テスト 1 プロセス」の費用構造。
  並列化の判断はここと切り離せない
- `interpreter/src/module_integration.rs::preparse_core_modules` — 既存の
  並列 pre-parse とプールのサイズ決定の根拠
