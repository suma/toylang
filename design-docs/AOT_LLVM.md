# AOT_LLVM — AOT の 2 つ目のバックエンドとしての LLVM

> 状態: **L0〜L5 すべて landing (2026-10-08)**。LLVM バックエンドは IR の
> 全命令を扱い、`interpreter/example/` 158 本中 157 本が cranelift と
> stdout・stderr・終了コードまで一致 (残る 1 本はポート番号)。
> **`--release` は LLVM `-O2`** で、feature 無しの `--release` はエラー。
> `--features llvm` のテストで consistency / example_consistency の AOT
> レーンを LLVM でも突き合わせる

## 0. 決めたこと

| 論点 | 決定 |
|---|---|
| 形 | **1 つの AOT コンパイラに 2 つ目のバックエンドとして足す** (別コンパイラは作らない) |
| 選び方 | `--codegen=cranelift\|llvm`。既定は debug = cranelift、**`--release` = LLVM** |
| 最適化 | release は **`-O2` 相当** (新パスマネージャの `default<O2>`) |
| feature | `llvm` (既定 off)。`toy` の `llvm` が `compiler/llvm` を有効にする |
| feature 無しの `--release` | **エラー** (直し方 2 つを言う。`--release --codegen=cranelift` は通る) |
| バインディング / 版 | **inkwell 0.10、`llvm22-1` のみ** (LLVM 22) |

## 1. どこを差し替えるか

`codegen::emit_object` は「`compiler_lower` で IR を作る →
`build_object_module` で object のバイト列にする」の 2 段で、その後の
リンク (`driver::link_executable`、`toylang_rt` の staticlib、
`TOY_LINK_CACHE_DIR`) は object の出どころを知らない。LLVM は
`build_object_module` の**横に並ぶ**: `compiler/src/llvm/` が IR の
`Module` を受けて object のバイト列を返す。lowering・テスト driver の
差し込み・リンク・ランタイム・CLI は共有。

LLVM 側が守る約束は cranelift 側と同じ IR の ABI である:

- 引数は**葉ごと** (`compiler_ir::layout::flatten_compound_leaf_types`)、
  compound の戻りは複数値 (LLVM では無名 struct で返す)
- 幅の広い参照 struct はポインタ (`Function::ptr_params`)
- ランタイムは `toylang_rt` の `toy_*` を同じ名前・同じ C ABI で呼ぶ
- エントリは IR の `main` (Export) を C の `main` から呼ぶ形を cranelift
  側と揃える

## 2. 段取り

| | 中身 |
|---|---|
| **L0** | `--codegen` (compiler / toy)、`CompilerOptions::codegen`、`llvm` feature、`toy` のスタンプにバックエンドを入れる。**この段では LLVM は `--codegen=llvm` と明示したときだけ** |
| **L1** | スカラーの部分集合: 関数・局所変数・整数 / 浮動小数 / bool の演算と比較・cast・分岐・ループ・直接呼び出し・`Print`・`Const`・panic 系の終端。未対応の命令は**名前を出してエラー** (黙って誤ったコードを作らない) |
| **L2** | compound (複数値の戻り、`CallStruct` / `CallEnum` / `CallTuple`、`PtrRead` / `PtrWrite`)、ヒープ (`toy_dispatched_*`)、文字列、配列、間接呼び出し・closure・`dyn` の vtable |
| **L3** | 観測性と検査: shadow stack (backtrace)、`--heap-check` の計装、確保のサイト情報、contract のメッセージ |
| **L4** | SIMD、`ParFor` / `TaskSpawn`、残りの命令。consistency harness に LLVM のレーン (feature 有効時のみ)、`example_consistency` |
| **L5** | `default<O2>`、**`--release` = LLVM の既定と、feature 無しの `--release` のエラーをここで有効にする**、計測 (cranelift `speed` との比較) |

**`--release` の既定を LLVM に切り替えたのは L5 (最後)。** LLVM が
全命令を扱えるようになる前に切り替えると、既存の `--release` ビルド
(POC の release ビルド、テスト) が「未対応の命令」で落ちる — feature
無しなら全部エラーになる。L4 までの `--release` は従来どおり
cranelift で、契約を外すだけだった。

L4 までは debug のまま `--codegen=llvm` で答えを突き合わせる。
`--release` は契約も外すので、release だけで比べると「LLVM の誤り」と
「契約が外れた違い」が区別できない。

## 2.5 実装で決まったこと

- **局所変数はすべて `alloca` で、入口で 0 に初期化する。** cranelift は
  書かれていない変数を 0 と読み、lowering はそれに頼っている —
  `var out: Vec<T>` の宣言より前に `return` する関数も、出口で `out` の
  drop を走らせ、0 のバッファの free (no-op) になる。初期化しないと
  スタックのゴミを free して落ちた (`try_compound.t`)。mem2reg が
  レジスタに戻すので `-O2` では残らない
- **ポインタは `i64` のまま運び、load / store の直前で `inttoptr` する**
  (cranelift と同じ表現)。compound の葉は詰めて置かれているので、
  計算した番地への load / store は align 1
- **シフト量はビット幅でマスクする。** cranelift の `ishl` は幅の剰余を
  取るが、LLVM の `shl` は幅以上で poison
- 比較の結果は `i1` を `i8` に zero-extend して bool にする (IR の bool は
  8 bit)。浮動小数の `!=` は unordered (`UNE`)、ほかは ordered
- float → int の cast は `llvm.fpto[su]i.sat` (cranelift の
  `fcvt_to_*_sat` と同じく NaN は 0)
- 幅の狭い整数の引数と戻り値には `signext` / `zeroext` を付ける
  (cranelift の `sext` / `uext`)。宣言にも呼び出し側にも
- 診断文字列のプール (`codegen::diag_pool::DiagPool`) は cranelift 側と
  共有する — ランタイムが読む record 形式がそのまま揃う
- `main` が `str` を返すプログラムの終了コードは文字列の番地の下位
  8 bit なので、バックエンドどうしで比べない
- **shadow stack は cranelift と同じ形**: 関数の入口で `toy_shadow_ctx()`
  を 1 回呼び、slot と深さを先に計算しておき、呼び出しの前後は store
  2 つと 1 つ。`main` は自分のフレームを積む。backtrace が cranelift と
  1 バイトも違わない
- **SIMD**: float の `min` / `max` は `llvm.minimum` / `llvm.maximum`
  (cranelift の `fmin` / `fmax` と同じく NaN を伝播し `-0 < +0`)、
  比較は `<N x i1>` を lane 幅に sign-extend したマスク、シフト量は
  lane 幅でマスク、`__simd_reduce_*` は lane 0 から順の逐次畳み込み
  (仕様)、`swizzle` は lane ごとに `idx < 16 ? t[idx] : 0`
- 間接呼び出しにも `signext` / `zeroext` を付ける — LLVM は呼び出し側と
  受け側の属性が揃っていることを前提にする (cranelift は付けない)
- **テストのレーン**: `--features llvm` のとき、consistency harness の
  `compile_file` / `checked_compiler_run` と example_consistency の
  `run_compiled_with` が、同じプログラムを LLVM でも作って走らせ、
  cranelift のバイナリと stdout (example は stderr も) と終了コード
  (example を除く) を突き合わせる。LLVM の出力をわざと変えると
  「the LLVM backend disagrees with cranelift」で落ちることを確かめた

## 2.6 L5 の実測 (2026-10-08、aarch64 Apple Silicon)

release どうしの比較 (cranelift `speed` / LLVM `default<O2>`、どちらも
ホストの CPU 向け):

| | cranelift | LLVM -O2 | |
|---|---:|---:|---|
| `fib(40)` | 0.30 s | 0.14 s | 2.1x |
| 整数演算のループ (`seq4`) | 0.65 s | 0.40 s | 1.6x |
| 確保 3,000 万回 (`allocbench`) | 0.65 s | 0.58 s | 1.1x |
| POC `verify` (65 万件 / 5 セグメント) | 0.150 s | 0.091 s | 1.65x |
| POC `query` 全走査 (`host=web3`) | 0.220 s | 0.115 s | 1.9x |
| POC `fields host` (索引) | 0.010 s | 0.007 s | |
| POC バイナリ | 586 KB | 475 KB | -19% |
| POC の release ビルド時間 | 0.08 s | 1.95 s | |

POC の 3 コマンドは出力が一致した。確保が多い処理の伸びが小さいのは、
時間の大半がランタイム (`toylang_rt`、Rust で書かれ同じもの) にあるため。
ビルド時間は 25 倍になるが、それは release の対価で、debug は
cranelift のまま速い (LLVM の `-O0` は 0.37 s)。

## 2.7 poc/logsearch での実行時間の比較 (2026-10-08)

コンパイル時間は除き、**実行時間だけ**を比べた。同じソース (HEAD
`c52c7ea0`) から `toy build poc/logsearch --release --codegen=cranelift|llvm`
で作った 2 つのバイナリ (cranelift `speed` / LLVM `default<O2>`、どちらも
ホストの CPU 向け、契約は外れる)。機械は Apple M5 Ultra (30 コア)、macOS 27.0.1。

入力は `poc/logsearch/log` の実ログ (562 ファイル / 137 MB、そのうち
apache2 が 33 MB)。`archive` で作った 12 セグメント / 444,549 レコード /
25 MB のアーカイブを読み取り系の全コマンドで共有し、書き込み系
(`archive` / `compact`) は毎回新しいディレクトリ・アーカイブの写しで
走らせた。各コマンドは 2 つのバイナリを交互に 5 回ずつ走らせた
**中央値**。出力 (stdout / stderr / 終了コード、所要時間の行と一時
ディレクトリのパスを除く) は**全コマンドで一致**した。

| コマンド | cranelift | LLVM -O2 | 速さ |
|---|---:|---:|---:|
| `scan` (全 562 ファイルの framing) | 150.3 ms | 58.8 ms | 2.56x |
| `archive` (読む → 索引 → 圧縮 → 書く) | 4,070.9 ms | 2,161.5 ms | 1.88x |
| `verify` (全セグメントを展開して CRC 照合) | 371.5 ms | 232.1 ms | 1.60x |
| `query status=404` (索引の完全一致) | 196.2 ms | 106.2 ms | 1.85x |
| `query path~/wp-` (値の部分一致) | 164.2 ms | 88.2 ms | 1.86x |
| `query path^/blog/` (値の前方一致) | 127.8 ms | 77.3 ms | 1.65x |
| `query error` (本文の全走査) | 304.5 ms | 210.5 ms | 1.45x |
| `query top=status` (分布、語彙だけ読む) | 19.3 ms | 12.9 ms | 1.49x |
| `query ip=<最多の値> top=path` (traversal) | 25.0 ms | 15.8 ms | 1.58x |
| `fields path` (索引) | 78.4 ms | 34.9 ms | 2.24x |
| `fields path ... scan` (全走査) | 393.7 ms | 242.9 ms | 1.62x |
| `object status=404` | 20.2 ms | 13.8 ms | 1.47x |
| `compact` (セグメントの併合) | 6,647.7 ms | 3,495.6 ms | 1.90x |
| サーバ: 60 KB × 600 回の `/v1/ingest` | 0.35 s | 0.17 s | 2.0x |

サーバの行は合成ログ (実在の値を含まない) で 3 回ずつ測ったもの。取り込み中の
`/v1/stats` の応答時間 (p99 0.7〜2.2 ms、最大 2.1〜6.2 ms) は両者で差が
見えない — 書き出しは既に背景の task に出ている (CONCURRENCY B、POC)。

**どこで差が出るか**: 伸びが大きいのは `scan` の行の切り出し、`fields` の
索引の読み取り、`archive` / `compact` の LSZ 圧縮と索引構築で 1.9〜2.6x、
小さいのは本文の全走査 (`query error`) の 1.45x と、数十 ms で終わる索引だけの
問い合わせの 1.5x 前後。

> **訂正 (同日)**: 当初ここには「伸びが小さいのは時間がランタイム側
> (Rust の `toylang_rt`) にあるもの」と書いたが、§2.8 のプロファイルで
> **誤り**と分かった。`query error` の時間は 98% が toylang の側
> (`query::search` と LSZ の展開) にあり、ランタイムは 0.2% だった。
> 最初の推測はフレームポインタが無いせいでスタックの欠けたサンプルを
> 数えたもので、ランタイム側に偏っていた。伸びの差がどこから来るかは
> まだ説明できていない。

再現: 測定スクリプトはリポジトリに置いていない (コマンド列は上表のとおり)。
traversal の値は `fields <arc> ip 1` の最多の値を使い、ここには書かない
(実ログ由来の値であるため — CLAUDE.md の規約)。

## 2.8 プロファイル (2026-10-08)

§2.7 と同じ機械・同じアーカイブで、(1) LLVM `-O2` で作った `poc/logsearch` の
実行時間と、(2) それを作るコンパイラ自身のコンパイル時間を測った。サンプラは
Instruments の Time Profiler (`xctrace record --template 'Time Profiler'`、
1 ms 間隔)。表を `xctrace export` で XML にし、スクリプトで葉のフレーム (self) と
スタック上の関数 (inclusive) に集計した。

### フレームポインタ (直したもの)

最初の計測では、LLVM 版の **56〜97% のサンプルにスタックが無かった**
(`verify` 97%、`scan` 70%、`archive` 57%。cranelift 版は 0%)。サンプラは
フレームポインタの鎖をたどるが、LLVM の関数にはフレームポインタを残せという
属性が付いておらず、`-O2` が消していた。clang / rustc が Apple のターゲットで
付けるのと同じ `"frame-pointer"="non-leaf"` を全関数に付けて、欠けは 0〜8% に
なった。実行時間の差は雑音の範囲 (−1〜+1%)。

この欠けのせいで、直す前の集計は「toylang の処理 6〜9%、ランタイムと
システム 90%」と出ていた。欠けたサンプルは捨てられ、スタックを持つ
ランタイム (Rust) と libSystem の分だけが残っていたためで、**数字の向きが
逆だった**。

### 実行時間: どこに時間が掛かるか (LLVM `-O2`)

self は葉の関数。LLVM がインライン展開した関数は呼び出し元に含まれる
(`query::search` の self が大きいのはそのため)。

| コマンド | toylang | ランタイム | システム | 大きいもの |
|---|---:|---:|---:|---|
| `archive` | 59% | 17% | 24% | `LabelDict::bump` 58% (inclusive)、うち `memcmp` 22% / `toy_mem_eq`。`prof_put` 6% |
| `compact` | 58% | 15% | 27% | `LabelDict::bump` 66% (inclusive)。`memcmp` 25% |
| `scan` | 75% | 6% | 19% | `cmd_scan` 47% (self)、`record::parse_line` 31%、open / read 15% |
| `verify` | 98% | 0% | 2% | `segfile::expand_all` 68%、`lsz::decode_frame` 20% |
| `query status=404` | 96% | 2% | 2% | `query::search` 73%、`decode_frame` 13% |
| `query error` | 98% | 0% | 2% | `query::search` 78%、`decode_frame` 19% |
| `fields path scan` | 98% | 0% | 1% | `expand_all` 65%、`decode_frame` 14% |
| `fields path` (索引) | 92% | 2% | 5% | `cmd_fields_indexed` 59%、`load_block` 21% |

読み取り:

- **`archive` / `compact` の 6 割は `LabelDict::bump`**。これは
  `Vec<String>` 2 本を頭から線形に比べる (`find`) ので、ラベルの値が増えると
  O(n) で効く。POC の側のアルゴリズムの問題で、バックエンドの問題ではない
  (hash 化すれば消える)。
- **ランタイムの確保表 `prof_put` が `archive` の 6%**。`free` を冪等にするため
  常時引いている番地 → サイズの表で (DROP-GLUE)、`archive` は確保 1.54M 回・
  realloc 2.30M 回・free 1.50M 回、1 回あたり ~26 ns。hash の偏りではなく、
  表が大きくてキャッシュに乗らないことが効いている。`String::push` の伸長で
  realloc が確保の 1.5 倍あるのも効いている。
- 読み取り系は 9 割以上が toylang の側 (検索ループと LSZ の展開) で、
  ランタイムはほぼ出てこない。

### コンパイル時間: LLVM は cranelift の約 24 倍

`toy build poc/logsearch --profile=compile` (release の `toy`、3 回とも ±3%):

| | 全体 | codegen | 内訳 |
|---|---:|---:|---|
| cranelift (opt speed) | 80 ms | 25 ms | 関数ごとに並列 (user 296 ms) |
| LLVM `-O0` (debug) | 354 ms | 265 ms | IR 構築 24 / verify 9 / 機械語 222 |
| LLVM `-O2` (`--release`) | 1,940 ms | 1,890 ms | IR 構築 19 / verify 6 / **`-O2` 1,090** / **機械語 770** |

**codegen は 1 スレッドで走る** (user ≒ wall)。cranelift は関数ごとに全コアで
並列に作っている。IR を作る段 (`llvm_build`) は 19 ms で、ほぼ全部が LLVM の中。
`--profile=compile` は 2026-10-08 にこの 4 段 (`llvm_build` / `llvm_verify` /
`llvm_O2` / `llvm_emit`) を出すようにし、見出しもバックエンドを名乗るようにした
(以前は LLVM のビルドにも `cranelift opt speed` と出ていた)。

コンパイラを同じサンプラで測ると、`-O2` の 1.09 s の内訳は InstCombine 229 ms、
IndVarSimplify 122 ms、Inliner 93 ms、GVN 52 ms、SimplifyCFG 44 ms、
CorrelatedValuePropagation 43 ms、SLPVectorizer 40 ms ほか (1 ms 単位の
サンプル数、残りは 30 ms 未満の pass が多数)。機械語の 0.77 s は
SelectionDAG 187 ms、Greedy レジスタ割り当て 130 ms、ループの pass
(LPPassManager) 95 ms、MachineScheduler 67 ms ほか。特定の pass が飛び抜けて
いるわけではなく、**pipeline 全体を 1 スレッドで回していることが効いている**。

縮める案 (未着手、todo の LLVM-COMPILE-TIME):

1. **機械語の段を並列にする**。モジュールを関数の組に分けて別々の
   `Context` / `TargetMachine` で object を作り、リンクで束ねる。`-O2` の後で
   分ければインライン展開は失わない (0.77 s → コア数で割れる)。
2. `-O2` 自体も分割できるが、分けた境界をまたぐインライン展開を失う。
   実行時間と引き換えになるので、分けるなら測ってから。

## 3. ビルドの仕方

LLVM 22 は Homebrew の `llvm@22` (keg-only) を使う。`llvm-sys` は
`LLVM_SYS_221_PREFIX` で場所を知る。機械ごとに違うので
`.cargo/config.toml` には書かない:

```bash
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p compiler --features llvm
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p toy --features llvm
```
