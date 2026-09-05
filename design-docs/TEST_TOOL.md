# TEST TOOL — toylang のテストを書く道具

> **状態: 提案 (未実装)。** 2026-09-05。
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

| | 内容 | 完了条件 |
|---|---|---|
| **T0** | module 内の `test` ブロック | `src/lsz.t` に書いた `test` が `--test` で走る |
| **T1** | compiled レーンでの `test` | 同じテストが `--backend aot` で走る |
| **T2** | `toy test` (探索・絞り込み・一覧・JSON) | `poc/logsearch` のテストが 1 コマンドで全部走る |
| **T3** | `core/std/testing.t` | 上の表の 5 つが 1 行で書ける |
| **T4** | `panics` テストと `--backend all` | 契約違反が検査でき、レーンの食い違いが出る |
| **T5** | ゴールデンと `--bless` | `.seg` の形式が固定される |

**最初の受け入れ先は `poc/logsearch`** である。5,000 行・12 モジュール・
4 レーン・バイト列の形式を持つプログラムが既にあり、そこで書けない
テストは設計が足りていない。ROADMAP は「次の 1 コミットはテスト」と
書いたまま止まっている — 止めているのは意志ではなく T0 である。

## 5. 非目標

- **モック / スタブのフレームワーク** — 依存注入の語彙が言語に無く、
  無いまま足すと `dyn Trait` の上に別の型システムを作ることになる
- **並列実行** — スレッドが無い (CONCURRENCY)。順次で 0.4 秒なら要らない
- **カバレッジ計測** — 計装が要る。`--profile=mem` と同じ層に置ける
  設計余地はあるが、テストがまず 1 つも無い段階の話ではない
- **JUnit XML などの CI 形式** — `--format=json` があれば変換は外で書ける
