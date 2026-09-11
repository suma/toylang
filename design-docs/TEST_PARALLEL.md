# テストの並列実行 (TEST-PARALLEL)

> **状態: P0〜P3 landing 済み (2026-09-11)。** 残るのは P4 (所要時間の
> 記録と longest-first)、P5 (`serial`)、P6 (`--backend all` との合成)。
> 実測 (release `toy`、20 コア): `poc/logsearch` の VM レーンが
> **4.83 s → 0.80 s**、合成 8 ファイルの AOT が cold **0.88 → 0.23 s** /
> warm **0.44 → 0.13 s**、`panics` 4 本が **0.18 → 0.08 s**。
> `-j1` は今までどおり (スレッドを 1 本も立てない)。
> 対象は `toy test` の**ランナー**。テストの書き方 (`test` ブロック /
> `panics` / `core/std/testing.t`) は [`TEST_TOOL.md`](TEST_TOOL.md) が
> 正本で、本文書はその D3 (ランナー) の続き。
> **処理系自身**の並列化は [`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md)、
> **言語**の並行性は todo.md の CONCURRENCY。3 つは別の話である。
> 実装サイト: [`toy/src/test_runner.rs`](../toy/src/test_runner.rs)、
> [`toy/src/package.rs`](../toy/src/package.rs)、
> `compiler_lower::install_test_driver`、
> [`interpreter/src/lib.rs`](../interpreter/src/lib.rs) (`run_tests`)。

## 1. TEST_TOOL は「要らない」と書いた

[`TEST_TOOL.md`](TEST_TOOL.md) §5 の非目標:

> **並列実行** — スレッドが無い (CONCURRENCY)。順次で 0.4 秒なら要らない

**2 つとも取り下げる。**

1. **「スレッドが無い」は言語の話で、ランナーの話ではない。** `toy` は
   ホストの Rust で書かれた普通のプロセスであり、テストを並べるのに
   toylang のスレッドは要らない。同じ取り違えをしていないのが
   `interpreter/src/output.rs` で、そこには「OS レベルのリダイレクトは
   **並列テストスレッド**と競合するから thread_local の sink を使う」と
   **既に書いてある**。下地は先にできていた。
2. **「0.4 秒」は 1 レーンの、今日の本数での値。** 同じ `poc/logsearch`
   のスイートは `--backend vm` で **4.63 秒**かかる (§2)。そして 0.4 秒の
   ほうも**テスト 14 本 / ファイル 3 本**での値で、費用は
   **ファイル数と `panics` テストの本数に線形**に伸びる。テストを書かせる
   ための道具が、書くほど遅くなる形をしている。

## 2. 測定 (2026-09-11、release `toy`、macOS / 20 コア)

| 対象 | レーン | wall | 内訳 |
|---|---|---|---|
| `poc/logsearch` (14 tests / 3 files) | AOT (既定) | **0.41 s** | front-end 0.12 + compile/link ~0.25 + 実行 ~0.01 |
| 同上 | `--backend vm` | **4.63 s** | front-end 0.12 + **実行 4.5** |
| 同上 | `--list` (front-end のみ) | 0.118 s | |
| 合成 8 files × 2 tests | AOT warm | 0.42 s | front-end 0.19 + driver 8 本 ~0.23 |
| 同上 | AOT cold (`build/` 削除) | 0.89 s | link cache miss で driver 1 本 ~85 ms |
| 同上 | `--backend vm` | 0.19 s | ほぼ front-end |
| `panics` テスト 4 本 (1 file) | AOT | 0.40 s | **テスト 1 本 = バイナリ 1 本** |
| コンパイル済みテストバイナリの起動 | — | **~3 ms** | 20 回ループの実測 |

読み取れること:

- **レーンでボトルネックが違う。** AOT は **compile + link** (driver 1 本
  あたり warm ~29 ms / cold ~85 ms)、VM は**テストの実行そのもの** (4.5 s)。
  どちらもジョブ間に依存が無い。
- **`panics` テストが一番高い。** 1 本ごとに 1 バイナリなので、`Vec` の
  契約違反を 20 本書くと **20 回のコンパイル**になる。VEC-CONTRACTS が
  契約を増やすほどここが伸びる。
- **プロセス起動は compile の 1/10 以下 (3 ms 対 29〜85 ms)。**
  §5 の D1 はこの数字の上に立っている。
- 余談だが、AOT レーンは**同じファイルの front-end を 2 回**通している
  (`plan()` が列挙のために parse + 型検査し、`compiler::compile_file` が
  もう一度やる)。並列化とは別件だが、per-file 固定費のうち無視できない
  割合がこれである。

**着手条件** ([`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md) §4 の流儀で):
**1 パッケージの `toy test` が 2 秒を超えたら**着手する。`poc/logsearch`
は VM レーンで既に超えている。AOT レーンの 0.41 秒は今日は縮まないので、
これは「今すぐやる理由」ではなく「今設計しておく理由」である。

## 3. 並列にする前に直すもの

**すべて逐次実行では無害で、並列にした瞬間に牙をむく。** 単独でコミット
できるので、フェーズ P0 としてまとめて先に入れる。

| # | 症状 | 直す場所 | なぜ並列の前提か |
|---|---|---|---|
| **X0** | **テストバイナリの名前が衝突する。** `tests/a/x.t` と `tests/b/x.t` は**同じ** `build/debug/tests/x` を書く (実測)。`panics` は `sanitise(テスト名)` なので、別ファイルの同名テストも衝突する | `toy/src/package.rs::test_exe_path` | 逐次なら「上書きして順に走る」で無害。並列だと**他方が書いている最中のバイナリを実行する**。しかも中間ファイル名が出力名から作られる (`driver.rs::sibling_temp_path` → `.toy_compile_x.o`) ので、`.o` も同時に踏む。パッケージ相対パスをキーに含めて一意にする |
| **X1** | **`.toycache` の書き込みが非アトミック** (`File::create` + `write_all`) | `frontend/src/cache.rs::save_full_module` / `save_interface` | 同じ module を同時にコンパイルすると同じ hash ファイルへ同時に書く。読み側は失敗すれば miss に落ちるので**安全側だが遅くなる**。link cache は既に tmp + `rename` でやっている (`compiler/src/driver.rs::populate_link_cache`) ので、同じ手口を写すだけ |
| **X2** | **`--backend vm` の filter が「走らせてから捨てて」いる。** `toy test "a skipped frame" --backend vm` は **4.71 秒** — 全部走らせるのと同じ | `toy/src/test_runner.rs::run_one_vm` (`run_tests_from_source` が全テストを実行してから `.filter()`) | 今の VM レーンには**「1 本だけ走らせる」経路が無い**。per-test ジョブを作るとは、その経路を作ることそのもの。AOT 側は `plan()` で先に絞っているので既に正しい |
| **X3** | **debug ビルドはオブジェクト破棄のたびにグローバル `Mutex` を取る** (`DESTRUCTION_LOG`、`cfg(debug_assertions)`) | `interpreter/src/object.rs` | VM レーンを並列にすると全ワーカーが drop ごとに 1 本の Mutex に並ぶ。release では no-op なので**開発ビルドだけが遅くなる**という一番たちの悪い形 (かつ log は際限なく伸びる)。thread_local にするか、既定 off にする |
| **X4** | **`TOY_BLESS` を `std::env::set_var` で渡している** | `toy/src/test_runner.rs::run_in_package` | Rust 2024 で `set_var` が unsafe なのは「他スレッドが走っている最中はダメ」だから。スレッドを立てる前に 1 回だけ、が構造的に守れる位置 (`main` の入口) に移す |

## 4. 状態の棚卸し — 何が並列にできるか

並列化の可否は、結局「プロセスに 1 つしか無いものを触るか」で決まる。

**per-thread になっている (そのまま並べられる)**

`RuntimeState::RT` / `OUTPUT_SINK` / `ERROR_SINK` / heap の `PROFILE`・
`PROFILE_SITES`・`PROFILE_ALLOCATORS` / `extern_io` の `RANDOM_STATE` と
各 status / `FFI_LIBS` / JIT のキャッシュ群 / `compiler_lower::templates`
の `IN_PROGRESS`。**インタプリタの実行時状態は全部 thread_local である。**
`random_seed` の決定性も、確保カウンタ (`__builtin_live_bytes`) も、
ワーカーごとに独立して正しい。

**共有だが既に安全**

`discover_core_modules` のキャッシュ (`Mutex`)、link cache (tmp + rename)、
`small_pool` / `preparse_pool` (プロセスに 1 つ。`install` は複数スレッド
から呼べる — 内側は 4 スレッド上限のままなので、外側を並列にしても
スレッド総数は増えない。内側の取り分がほぼゼロになるだけ)。

**プロセスに 1 つしか無い (並列にしても分けられない)**

- **cwd** — パッケージ根に固定する規約 (golden のパスが書いたとおりの
  意味になるように、TEST-TOOL T5)。**これは動かさない**。したがって
  「テストごとに作業ディレクトリを分ける」形は採れない (§8)。
- **環境変数** — X4 のとおり、スレッドを立てる前に決める。
- **`io::exit`** — 呼んだテストはランナーごと落とす。今日と同じ。

**AST は `!Send`**

`File` は `Rc<Function>` を持つので、スレッドを跨げない (`Arc` 化は
[`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md) の非目標)。したがって
**設計の一番外側の制約はこれ**:

> **1 ジョブの front-end と実行は同じスレッドで完結させ、スレッド間を
> 渡るのは `Outcome` (String と u32 だけ) にする。**

`module_integration.rs` の `SendFile` のような `unsafe impl Send` は
**要らない**。ジョブ境界をそう引く限り、この設計に unsafe は出てこない。

## 5. 設計

### D1. ジョブ = 既に独立している単位

| レーン | ジョブ | 1 ジョブの費用 | 上限 |
|---|---|---|---|
| AOT | **driver 1 本のコンパイル** (ファイル単位) | warm ~29 ms / cold ~85 ms | ファイル数 |
| AOT | **テスト 1 本の実行** (driver を 1 プロセス起動) | ~3 ms + テスト本体 | テスト本数 |
| VM | **テスト 1 本** (front-end は per-thread memo、D3) | テスト本体 | テスト本数 |

AOT を **compile ジョブ / run ジョブに割る**のが、今の実装との一番大きな
違いである。今は「driver 1 本 = 1 コンパイル + 1 実行」で、`panics` テスト
だけが 1 本 = 1 バイナリになっている。§2 のとおり **compile は run の
10〜30 倍高い**ので、向きを逆にする:

> **ファイルごとに driver を 1 本だけ作り、テストは 1 本ずつ別プロセスで
> 走らせる。**

そのために driver に**走らせるテストを実行時に選ばせる**:
`compiler_lower::install_test_driver` が焼き込んでいる名前集合
(`CompilerOptions::test_only`) の代わりに、**index を実行時に読む**
入口を生成する (`__toy_test_index() -> u64` を `toylang_rt` に足し、
`TOY_TEST_INDEX` を読む。文字列比較を生成コードに持ち込まないため
名前ではなく index)。得られるもの:

- **`panics` テストがコンパイルを 1 回も増やさない。** 4 本の `panics` が
  0.40 秒 → driver 1 本 (~29 ms) + 4 プロセス (~12 ms) になる
- **AOT レーンが全部の失敗を 1 回で報告できる。** 今は「panic が
  プロセスを終わらせるので最初の失敗で止まる」(TEST_TOOL T1 の記録) が、
  1 テスト 1 プロセスなら止まらない。**`--backend vm` を選ぶ理由が
  1 つ消える**
- ジョブが細かくなるので並列の効きが良くなる

代償は 1 テストあたり ~3 ms のプロセス起動。14 本で 40 ms、うち並列で
消えるぶんを引けば実質もっと小さい。**`--jobs 1` で測って比べられる**
ようにしておくこと。

### D2. スケジューラ — 依存の無い動的割り当て

```rust
// One cursor, many workers: jobs are wildly uneven (one test runs for
// seconds while its neighbours take a millisecond), so a static split
// would leave workers idle behind the long one.
let cursor = AtomicUsize::new(0);
std::thread::scope(|s| {
    for _ in 0..jobs {
        s.spawn(|| while let Some(i) = next(&cursor, jobs_len) {
            results[i].set(run_job(&plan[i]));   // Outcome only
        });
    }
});
```

- **rayon を `toy` に足さない。** ジョブが粗くて work-stealing の細かさが
  要らない、compiler / interpreter の内部プールと入れ子にしたくない、
  `toy` の依存は今すべて自前クレートである、の 3 つ。`std::thread::scope`
  と `AtomicUsize` で足りる。
- **長いジョブから先に。** ジョブ長の分散が大きい (`poc/logsearch` は
  1 本が数秒、他は ms) ので、順序が speedup をほぼ決める。前回の所要時間を
  `build/<profile>/.testtimes` に記録して降順に並べ、無いものは末尾。
  ファイルではなく `(file, line, name)` をキーにする — テストを 1 本足した
  だけで履歴が全部捨てられないように。
- **`-j N` / `--jobs N`**、既定は `available_parallelism()`、**`-j1` は
  今日と同じ順序で同じ出力**になること (§7 の検査はこれを使う)。

### D3. VM レーン — per-thread front-end memo と新しい API

VM レーンのジョブは**テスト 1 本**にしたい (実行が支配的だから) が、
`File` は `!Send` なので「1 回 front-end を通して全ワーカーで共有」が
できない。**ワーカーごとに 1 回ずつ通す**:

```rust
thread_local! {
    // A worker parses a file the first time it steals a test from it.
    // With T workers and F files the front end runs at most T*F times
    // instead of once (0.04 s per file for logsearch) -- the price of
    // an AST that is not Send.
    static PREPARED: RefCell<HashMap<PathBuf, Rc<PreparedTests>>> = ...;
}
```

interpreter 側に「検査だけして持っておく」形を足す (今は
`run_tests_from_source` が検査と実行を一息でやるので、分けられない):

```rust
pub struct PreparedTests { /* !Send: File + interner + source */ }
pub fn prepare_tests(source: &str, filename: &str, options: &RunOptions)
    -> Result<PreparedTests, String>;
impl PreparedTests {
    pub fn cases(&self) -> &[TestCaseInfo];      // name / line / file / expect_panic
    pub fn run_one(&self, index: usize) -> TestOutcome;
}
```

既存の `run_tests_from_source` は `prepare_tests` + 全 index の `run_one`
に置き換える (呼び出し側は無変更)。**X2 はこの API が生えた時点で消える** —
フィルタは `cases()` に対して、走らせる前に効く。

`run_one` は**テストごとに確保カウンタをリセットする**
(`heap::reset_profile()`)。thread_local なカウンタは「そのスレッドで
先に走ったテストの累積」を持つので、リセットしないと
`__builtin_allocations()` を読むテストの答えがスケジュール依存になる
(今日は「全部が 1 スレッドで順に走る」ので偶然決定的なだけ)。
`live_bytes` の差分を見る `assert_no_growth` は元から影響を受けない。

### D4. 報告 — 順序は保ち、出力は捕まえる

- **完了順ではなくジョブ順に並べ替えてから報告する。** ジョブ順は
  discover 順 (ファイル名ソート → 宣言順) なので、**`-jN` の出力は
  `-j1` とバイト一致する**。今の `(file, line, name)` の重複畳み込みも
  ジョブ順に適用すれば結果は変わらない。
- **VM レーンはテストごとの stdout を `output::with_capture` で捕まえ、
  失敗したテストのぶんだけ出す。** 捕まえないと 20 本の `println` が
  混ざって出る。AOT レーンは `Command::output()` で既に捕まっている。
- **途中経過は出さない。** 並列に走っているものを 1 行ずつ流すと順序が
  実行順になり、決定性が壊れる。`-v` のときだけワーカー番号つきで
  開始/終了を出す (診断であって出力ではない)。

### D5. 直列でなければならないテスト

既定を並列にする以上、**同じファイル・固定ポート・cwd 相対の一時ファイル
を触るテストは壊れる**。`poc/logsearch` のテストは `.seg` を書くので、
受け入れ先で必ず要る。3 段で逃がす:

1. **`--jobs 1`** — 全体を今日の挙動に戻す。P1 ではこれだけ。
2. **`test "..." serial { }`** — contextual keyword。`panics` と同じ流儀で
   「テストの性質はテストの隣に書く」。`serial` なテストは全部集めて
   **最後に 1 本ずつ**走らせる (並列ジョブが終わってから、が一番単純で
   説明もしやすい)。
3. **`--bless` は暗黙に `-j1`** — golden を書く操作なので、2 本が同じ
   パスへ同時に書くと勝者が不定になる。記録は年に数回の操作で、
   速度を要求する場面ではない。

`serial` を `toy` 側の規約 (`tests/serial/` に置く等) にする案も考えたが、
**ファイルの置き場所でテストの意味論を決めるのは `panics` と不整合**に
なる。言語側に置く。

### D6. 見積り

| 対象 | 今 | 見積り (T=8) | 何で決まるか |
|---|---|---|---|
| `poc/logsearch` VM | 4.63 s | **~0.7 s** | 実行 4.5 s ÷ min(T, 14) + front-end T×0.04 s。テスト長の分散次第 |
| `poc/logsearch` AOT | 0.41 s | ~0.25 s | 固定費 (front-end 0.12) が支配的で、**あまり縮まない** |
| 合成 8 files AOT warm | 0.42 s | ~0.15 s | compile 8 本の並列 |
| 合成 8 files AOT cold | 0.89 s | ~0.25 s | 同上 (1 本 85 ms) |
| `panics` 4 本 | 0.40 s | **~0.05 s** | D1 で compile が 4 本 → 1 本になるため。**並列化より D1 の効きが大きい** |

**測っていないもの**: VM ワーカー 1 つが抱える統合済みプログラムの
常駐量。`-j20` で RSS がどうなるかは P1 の受け入れ条件に入れる (必要なら
既定の `-j` に上限をかける)。

## 6. フェーズ

| | 内容 | 完了条件 | 状態 |
|---|---|---|---|
| **P0** | §3 の X0〜X4 | 並列にしなくても正しい。X0 は今日でも「同名ファイルが 2 つあると片方のバイナリしか残らない」 | ✅ (X2 は P3 で消えた) |
| **P1** | ジョブ化 + `-j` | `-j1` と `-jN` の出力がバイト一致。合成 8 files が 0.42 → 0.2 s 未満 (実測 0.128 s) | ✅ |
| **P2** | D1 の driver index 化 | `panics` 4 本が 1 コンパイル。**AOT が全部の失敗を報告する** | 未 |
| **P3** | D3 の per-test ジョブ + `prepare_tests` | `poc/logsearch` VM が 2 s 未満 (実測 0.80 s)。X2 が消える | ✅ |
| **P4** | D2 の所要時間記録 + longest-first | 意図的に偏らせたスイートで wall が最長ジョブ + ε になる | 未 |
| **P5** | `serial` | `.seg` を書くテストが並列で緑のまま | 未 |
| **P6** | `--backend all` (TEST_TOOL T4 の残り) との合成 | レーン × テストがジョブになる | 未 |

P0 と P2 は**並列化と独立に価値がある** (前者は正しさ、後者は
「AOT で全部の失敗が出る」)。先に入れて損しない順に並べてある。

### P0〜P3 で分かったこと (2026-09-11)

- **plan 段階も front end である。** 「走らせる前に何があるか調べる」の
  実体は parse + 型検査で、ファイルあたり 20〜40 ms かかる。最初の実装は
  これを逐次でやってからテストだけを並列にしたので、**9 ファイルの
  小さなスイートが 0.22 s → 0.31 s と遅くなった** — 走っている 0.02 s を
  8 分割する間、0.19 s の plan は 1 スレッドのままだった。plan も
  同じカーソルで並列にして 0.14 s。**Amdahl の分母を先に見ること。**
- **`-j1` では plan の成果を捨てない。** VM レーンの実行は plan が作った
  検査済みプログラムそのものを要るので、ワーカーが 1 本 (= plan と同じ
  スレッド) のときは thread_local の memo に残して再利用する。
  `-jN` では捨てる — 別のスレッドが走らせるので使えず、
  検査済みプログラムは stdlib ごと抱えるから、持っているだけで高い
- **重複の畳み込みを plan に移したのは副産物として速い。** module の
  テストは取り込んだプログラムの数だけ現れるので、以前は**全部走らせて
  から**報告を 1 つに畳んでいた。plan で畳むと走らせる回数も 1 回になる
- **`assert_eq` の出力捕捉は失敗時だけ出す。** 並列に走る `println` を
  そのまま流すと混ざる。thread_local の sink (`output::with_capture`) が
  既にあったので、レーンをまたぐ追加実装は要らなかった
- **確保カウンタのリセットは既にあった。** D3 が心配していた
  「スレッドで先に走ったテストの累積が見える」は、`execute_entry` が
  実行ごとに `reset_profile()` を呼ぶ (MEMORY_PROFILING M4) ので
  元から起きない
- **`-j2` だけは小さいスイートで損をする** (合成 9 ファイルで
  0.22 → 0.32 s)。plan と実行の両方でファイルごとの front end を
  払い直すのに、並列度が足りない。既定はコア数なのでここには落ちないが、
  P4 の所要時間履歴が入れば「並べても取り分が無い」と分かって
  1 本に落とせる

## 7. 検証

`toy/tests/package_layout.rs` に足す (1・2・3・6 は landing 済み):

1. **同名ファイルが別バイナリになる** — `tests/a/x.t` と `tests/b/x.t` を
   置き、`build/*/tests/` に 2 本できること (X0 の回帰)
2. **`-j1` と `-j8` の出力がバイト一致** — 決定性を pin する一番安い形。
   text と `--format=json` の両方で、**両レーンで**、失敗と `panics` を
   含むスイートに対して。**所要時間の行だけは比較から外す** (2 回の
   実行が正当に食い違う唯一の値)
3. **失敗の出力が混ざらない** — 2 本落ちるスイートで、それぞれの
   診断が相手のテスト名を含まないこと
4. **`--bless` が `-j1` に落ちる**
5. **`-j8` で確保カウンタが決定的** — `__builtin_allocations()` を読む
   テストを 2 本置き、10 回走らせて同じ数字になること (D3 のリセット)
6. **`panics` テストが並列でも正しく落ちる** (P2 後: 1 driver から
   複数プロセス)

`-j1` と `-jN` の一致を pin しておけば、以後の変更で順序が漏れても
テストが落ちる。**これが並列化の唯一の安全弁**である。

## 8. 非目標

- **言語レベルの並行性 (CONCURRENCY)** — 別の話。`toy` はホストの Rust
  なので、toylang にスレッドが無いことは `toy test` の並列化を妨げない。
  TEST_TOOL.md の非目標はここを取り違えていた (§1)。
- **テストごとの sandbox (cwd / 一時ディレクトリの分離)** — cwd は
  プロセスに 1 つで、golden のパスがパッケージ根相対という規約
  (TEST-TOOL T5) が壊れる。AOT の子プロセスだけ `current_dir` を
  与えることは**できてしまう**が、レーンで意味論が割れるのでやらない。
- **タイムアウトで殺す** — AOT の子プロセスには効くが VM のスレッドは
  殺せない。これもレーンで割れるので入れない。ハングは `-v` で
  「どのジョブが返っていないか」が見えれば足りる。
- **分散実行 / CI シャーディング** — `--format=json` と `-j` があれば
  外で書ける。TEST_TOOL が JUnit XML について言ったのと同じ理由。
- **処理系自身の並列化** — [`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md)。
  ただし本文書の P1 が入ると**外側が並列になるので内側 (small_pool /
  preparse_pool) の取り分は消える**。両方を測るときは片方ずつ切ること。

## 関連

- [`TEST_TOOL.md`](TEST_TOOL.md) — テストの書き方とランナーの本体 (D3)
- [`BUILD_TOOL.md`](BUILD_TOOL.md) — `toy` のサブコマンドと build/ の配置
- [`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md) — 処理系自身の並列化と、
  `Rc` による `!Send` 制約の記録
- [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) §P4/§P5 — `test` ブロックと
  `--check` の設計
