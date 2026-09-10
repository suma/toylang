# TEST TOOL — toylang のテストを書く道具

> **状態: T0〜T5 landing 済み (2026-09-05)。** 残るのは
> T4 の後半 (`--backend all` = レーン間の食い違い報告) だけ。
> 実装: `interpreter/src/module_integration.rs` (T0)、
> `compiler_lower::install_test_driver` + `compiler --test` (T1)、
> [`toy/src/test_runner.rs`](../toy/src/test_runner.rs) (T2)。
> **`toy test` の既定は AOT** — 出荷するレーンが検査対象になる。
> 対象: **toylang で書かれたプログラム**のテスト。処理系自身の Rust
> テストは [`TEST_PLAN.md`](TEST_PLAN.md) が正本で、本文書は別の層。
> 既にあるもの: `test "..." { }` + `--test` (LLM-LOOP P4)、
> `assert_eq` / `assert_ne` / `assert`、`--check` の契約プロパティ
> テスト (P5)、`--all-backends`。設計は
> [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) §P4/§P5。
> 実装サイト: [`interpreter/src/lib.rs`](../interpreter/src/lib.rs)
> (`run_tests`)、[`interpreter/src/property.rs`](../interpreter/src/property.rs)、
> [`frontend/src/parser/expr/macros.rs`](../frontend/src/parser/expr/macros.rs)
> (`assert_eq` の desugar)。
> 関連: [`BUILD_TOOL.md`](BUILD_TOOL.md) (`toy test` はその 1 サブコマンド)。

## 1. なぜ — 5,000 行書いてテストが 0 だった

`poc/logsearch` は toylang で書かれた 5,000 行のプログラムで、
**テストが 1 つも無い**。回帰はすべて目視と `grep` / `awk` / Python の
オラクルとの突き合わせで見つけている。書き手が怠けたのではなく、
**置く場所が無かった**。以下は全部 2026-09-05 に実測した。

### 穴 1 — `test` ブロックはモジュールの中に置けない ★★★

```rust
# src/mathx.t (auto-load されるモジュール)
pub fn triple(n: u64) -> u64 { n * 3u64 }

test "triple works" {
    assert_eq(mathx::triple(3u64), 9u64)
}
```

```
[E0010] Core module `mylib.mathx` integration error:
        Unsupported expression type for remapping:
        BuiltinMethodCall(ExprRef(10), StrConcat, [ExprRef(12)])
```

`assert_eq` は**パーサが文字列連結に desugar する**マクロで、
モジュール統合の remapper が `BuiltinMethodCall` を知らない。
結果として:

> **テストはエントリファイルにしか書けない。エントリは、この言語で
> 唯一モジュールではないファイルである。**

12 モジュールのプログラムのテストが 1 つのファイルに集まることになり、
テスト対象の private 関数には触れない。**これが 0 件だった一番の理由**。

### 穴 2 — compiled レーンで `test` が走らない ★★★

```
$ compiler --core-modules <root> with_test.t -o out
compile error: assert requires a string literal message in this compiler MVP
```

`test` ブロックの中の `assert_eq` は AOT が拒否する。つまり
**`--test` はインタプリタ専用**で、出荷するレーン (AOT) は
`test` ブロックで検査できない。`poc/logsearch` が踏んだ不具合は
`&mut` の書き戻し・SoA・register 予算と、**どれもバックエンド固有**
だった。一番テストが要る場所に届いていない。

### 穴 3 — 走らせる単位が「1 ファイル 1 プロセス」

`--test` は 1 ファイルを取る。ディレクトリを渡せない、名前で絞れない、
一覧が出せない。50 個のテストファイルは 50 回の起動になり、
プロセス固定費 ~30 ms × 50 = 1.5 秒が乗る。

### 穴 4 — 語彙が 3 つしかない

`assert` / `assert_eq` / `assert_ne`。無いもので、この POC が実際に
書きたかったもの:

| 欲しかった検査 | 今どう書くか |
|---|---|
| `f64` の許容誤差つき比較 | 手で `if (a - b).abs() > eps { panic(...) }` |
| バイト列 (`Span<u8>`) の一致と**最初に違う位置** | 手でループ。失敗しても「どこが」が出ない |
| **panic すること** (境界外 `Vec::get` など) | **書けない**。panic はプロセスを終わらせる |
| 確保バイト数が増えないこと | カウンタはあるが (`__builtin_live_bytes`)、比較は手書き |
| 前回と同じバイト列 (ゴールデン) | 無い。`.seg` の形式安定はこれで縛りたい |

**`panic` を期待するテストが書けない**のは効きが大きい。
VEC-CONTRACTS で `Vec` の境界が `requires` になった直後に、
**その契約が破れることを確かめる術が無い**。

## 2. 設計

**2 層に分ける。** 言語の中に置く**ライブラリ**と、外から回す
**ランナー**。ランナーは [`BUILD_TOOL.md`](BUILD_TOOL.md) の
`toy test` で、独立した実行ファイルは作らない。

### D1. `core/std/testing.t` — 失敗したときに何が起きたか言う関数群

```rust
# Every assertion takes the two values and panics with both of them
# in the message: the point of an assertion library is that a failure
# reads like a report, not like a boolean that came out false.
pub fn assert_close(a: f64, b: f64, eps: f64)
pub fn assert_str_eq(a: str, b: str)
pub fn assert_bytes_eq(a: Span<u8>, b: Span<u8>)   # first differing offset
pub fn assert_some<T>(v: Option<T>) -> T           # unwrap or fail
pub fn assert_ok<T, E>(v: Result<T, E>) -> T
pub fn assert_in_range(v: i64, lo: i64, hi: i64)
```

**`assert_bytes_eq` が最初に違うオフセットを出すこと**が要点で、
4 MB の `.seg` が 1 バイト違うときに「違います」だけ言われても
使えない。`Span::bytes_eq` は既にある (MEMORY-ACCESS M3) ので、
不一致時の走査だけを足す形になる。

**確保の検査**は既存のカウンタに載せる:

```rust
test "reading a segment does not grow the heap" {
    val before = __builtin_live_bytes()
    read_one_segment()
    assert_no_growth(before)      # message says how many bytes it grew
}
```

`ensures allocates(N)` (ALLOC-CONTRACT-SUGAR) が**関数の契約**なのに対し、
こちらは**テストの中の区間**に効く。両方要る。

### D2. `test "..." panics { }` — panic を期待する

```rust
test "index past the end panics" panics {
    val v: Vec<u64> = Vec::new()
    val x = v.get(0u64)
}
```

`panics` は contextual keyword (`test` と同じ流儀)。実装:

- **インタプリタ**: panic は Rust 側の `RuntimeError` なので、
  ランナーが捕まえて成功と数える。追加コストゼロ
- **compiled レーン**: panic はプロセスを終わらせるので、
  **このテストだけ別プロセスで実行**し、終了コードと stderr を見る。
  数が少ない前提でよい (`poc/logsearch` なら 5 個程度)

**メッセージの一致も見られるようにする** (`panics "index out of bounds"`)。
契約違反のテストはこれが無いと「何かが落ちた」しか言えない。

### D3. ランナー — `toy test`

```
toy test                      # パッケージ全部
toy test lsz                  # 名前の部分一致で絞る
toy test --list               # 走らせずに一覧
toy test --backend all        # 4 レーンで走らせ、食い違いを報告
toy test --check              # 契約プロパティテスト (P5) も回す
toy test --format=json        # 機械向け (--diagnostics=json と同じ流儀)
```

- **探索**: `tests/*.t` と `src/**/*.t` と `main.t` の `test` ブロック。
  穴 1 が塞がると `src/` が本命になる
- **出力は failure-first** (P4 で決めた形をそのまま):

```
FAILED  lsz roundtrip of repeated bytes (src/lsz.t:212)
    Error at src/lsz.t:214:5:
    214 |     assert_bytes_eq(out.span(), input)
        |     ^^^^^^^^^^^^^^^ panic: byte 4097 differs: left 0x41, right 0x00
17 passed, 1 failed, 2 skipped (aot: assert in test block)   0.42 s
```

- **`--backend all` は `assert_consistent` のユーザ版**。
  `compiler/tests/consistency/` が処理系開発者に与えているものを、
  toylang を書く人にも与える。**食い違いだけを報告する**
- **1 プロセスで全部走らせる** (`toy` が compiler / interpreter を
  crate として持つ)。穴 3 の 1.5 秒が消える
- **各テストは独立したコンテキスト** — P4 の既定を維持

### D4. ゴールデンファイル

```rust
test "the segment format did not change" {
    val bytes = build_one_segment()
    assert_golden("tests/golden/one.seg", bytes)
}
```

`toy test --bless` で書き直す。**形式のバージョニング約束
([`STORAGE_FORMAT.md`](../poc/logsearch/design-docs/STORAGE_FORMAT.md) §9)
は、これが無いと守れているか分からない** — 「既存データを読めなく
する変更を入れない」は、バイト列を固定して初めて検査になる。

`--bless` の差分は「何バイト目から違う」を出す (D1 と同じ関数)。

## 3. 先に直すもの (すべて再現を確認済み)

| # | 症状 | 直す場所 | 効き |
|---|---|---|---|
| **T0** | module 内の `test` が統合で落ちる (`Unsupported expression type for remapping: BuiltinMethodCall`) | [`interpreter/src/module_integration.rs`](../interpreter/src/module_integration.rs) の remapper | **テストがコードの隣に置ける**。他は全部これの後 |
| **T1** | AOT が `test` 内の `assert_eq` を拒否 (`assert requires a string literal message in this compiler MVP`) | compiler の assert lowering | **出荷レーンがテストできる** |
| **T2** | `main` の無いファイルの AOT が無関係なエラーを出す (`log::level_from_rank is neither a variant ...`) | compiler のエントリ解決 | テストファイルは `main` を持たない |
| **T3** | `--test` が 1 ファイル固定 | interpreter CLI / `toy` | 探索と絞り込み |

T0 が最優先。**残り 3 つは T0 の後でないと価値が出ない** — テストが
1 ファイルに閉じ込められている限り、走らせ方を良くしても書く量が
増えないため。

## 4. フェーズ

| | 内容 | 完了条件 | 状態 |
|---|---|---|---|
| **T0** | module 内の `test` ブロック | `src/lsz.t` に書いた `test` が `--test` で走る | ✅ 2026-09-05 |
| **T1** | compiled レーンでの `test` | 同じテストが `--backend aot` で走る | ✅ 2026-09-05 |
| **T2** | `toy test` (探索・絞り込み・一覧・JSON) | `poc/logsearch` のテストが 1 コマンドで全部走る | ✅ 2026-09-05 |

### T1 で分かったこと

- **直したのは `assert` のメッセージ制限** — `panic` は既に非リテラルを
  `PanicStr` で受けていた (ERROR_MODEL E3) のに `assert` は literal 縛りの
  ままだった。`assert_eq` は 2 値からメッセージを組むので、
  **`assert_eq` を含む `test` はそもそもコンパイルできなかった**。
  メッセージは fail ブロックの中で lower する (false のときだけ評価する
  規約を守るため、かつ通ったテストが報告の費用を払わないため)
- **compiled レーンの entry を合成する** —
  `compiler_lower::install_test_driver` が、各テストの前に stderr へ
  マーカーを出して呼ぶだけの `main` を作る。ユーザの `main` は
  `toy_program_main` に改名して残す (テストが呼びうる)。
  **最初の失敗で止まる** — assertion 失敗は panic でプロセスが終わる。
  全部報告するにはテストごとに 1 プロセスが要る (T4 の形)
- **T2 (main の無いファイル) も一緒に消えた** — `test` ブロックを
  entry として数えるようにしたので、`tests/*.t` が
  「全部 lower する」フォールバックに落ちて無関係な stdlib の body で
  死ぬことが無くなった
- **`--test` が IR VM で全部 pass していた** (既存バグ) —
  `execute_entry` は entry 関数を受け取るのに、IR VM の fast path は
  **`main` を走らせていた**。`assert_eq` が lower できなかったおかげで
  ineligible になり tree-walker に落ちていたので露見していなかった。
  T1 で lower できるようにした瞬間に**全テストが黙って緑になる**。
  fast path を「entry が本当に `main` のときだけ」に絞った
| **T3** | `core/std/testing.t` | 上の表の 5 つが 1 行で書ける | ✅ 2026-09-05 |
| **T4** | `panics` テストと `--backend all` | 契約違反が検査でき、レーンの食い違いが出る | `panics` ✅ / `--backend all` 未 |
| **T5** | ゴールデンと `--bless` | `.seg` の形式が固定される | ✅ 2026-09-05 |

### T3〜T5 で決めたこと

- **`assert_bytes_eq` の走査は失敗時にしか走らない** — `Span::bytes_eq`
  が yes/no を 1 呼び出しで答える (MEMORY-ACCESS M3) ので、
  通ったアサーションの費用は `bytes_eq` そのもの。オフセットを探す
  ループはその後
- **確保の検査は `live_bytes`** — cumulative ではない。
  バッファを確保して解放したヘルパは heap を「増やして」いないので、
  確保そのものを禁じると実装を検査することになる
- **`panics` テストは AOT ではテスト 1 本 = バイナリ 1 本** —
  panic がプロセスを終わらせるので、後に走るものと driver を共有できない。
  そのため driver の filter は**名前の集合**である (1 本を除くとは
  他の全部を名指すこと)
- **golden が無いときは失敗**で、初回に黙って記録はしない。
  一度も見られていないテストが緑になるのを避ける。
  `--bless` は `TOY_BLESS` で伝える (AOT は子プロセスの環境、
  VM は `toy` 自身の環境)
- **テストはパッケージ根から走る** — テストに書く golden のパスが
  書いたとおりの意味になるように

### T0 で分かったこと

- **直したのは 1 つの match arm。** 統合の remapper に
  `Expr::BuiltinMethodCall` の腕が無かっただけで、`BuiltinMethod` は
  symbol を持たない普通の enum なので受け手と引数を写すだけだった。
  「5,000 行にテストが 0 件」の原因が 20 行の欠落だったことになる
- **`test` ブロックは function とは別に運ばれる。** `test "..." { }` は
  0 引数関数に lower され、その関数は元から統合で写っていた。
  写っていなかったのは**それを名指す `TestCase`** の方で、
  モジュールのテストは「死んだ関数」として存在し `--test` は
  「`test` ブロックが無い」と答えていた
- **名前とファイルを持たせた。** テスト名は `mathx::triple works` の
  ように module で修飾する (2 つの module が同じ "roundtrip" を
  持てるので)。`TestCase` に `file` を足して、失敗が**自分のファイル**を
  引くようにした (足すまでは entry の名前 + module の行番号という
  嘘の組み合わせを出していた)
- **同じ module のテストは 1 回だけ報告する。** `tests/a.t` も `main.t` も
  `src/` を取り込むので、素直に走らせると同じブロックが取り込んだ
  プログラムの数だけ出る。ブロックを同定するのは書かれた場所なので、
  `(file, line, name)` で畳む

**最初の受け入れ先は `poc/logsearch`** である。5,000 行・12 モジュール・
4 レーン・バイト列の形式を持つプログラムが既にあり、そこで書けない
テストは設計が足りていない。ROADMAP は「次の 1 コミットはテスト」と
書いたまま止まっている — 止めているのは意志ではなく T0 である。

## 5. 非目標

- **モック / スタブのフレームワーク** — 依存注入の語彙が言語に無く、
  無いまま足すと `dyn Trait` の上に別の型システムを作ることになる
- ~~**並列実行**~~ — **取り下げた (2026-09-11)**。「スレッドが無い
  (CONCURRENCY)」は**言語**の話で、`toy` はホストの Rust なので関係が
  無い。「順次で 0.4 秒」も AOT レーンの値で、同じスイートは
  `--backend vm` で 4.63 秒かかる。設計は
  [`TEST_PARALLEL.md`](TEST_PARALLEL.md)
- **カバレッジ計測** — 計装が要る。`--profile=mem` と同じ層に置ける
  設計余地はあるが、テストがまず 1 つも無い段階の話ではない
- **JUnit XML などの CI 形式** — `--format=json` があれば変換は外で書ける
