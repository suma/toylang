# LLVM_PERF — LLVM バックエンドの性能: プロファイルとコンパイル時間の検討

> 2026-10-08。AOT-LLVM ([`AOT_LLVM.md`](AOT_LLVM.md)) が landing した後に、
> (1) LLVM `-O2` で作ったバイナリの実行時間はどこに掛かるか、(2) コンパイラ自身の
> `--release` ビルドはなぜ遅いか、(3) それをどう縮めるか、を測った記録。
> cranelift との実行時間の比較は [`AOT_LLVM.md`](AOT_LLVM.md) §2.7。
> todo の項目は LLVM-COMPILE-TIME / RT-ALLOC-TABLE-COST、POC 側は
> [`../poc/logsearch/design-docs/ROADMAP.md`](../poc/logsearch/design-docs/ROADMAP.md) の 7。

## 0. 要点

- **プロファイルを取るには、フレームポインタが要った。** LLVM の関数に
  `"frame-pointer"` 属性が無く、`-O2` がフレームレコードを消していたので、
  サンプラはサンプルの 56〜97% でスタックを失っていた。clang / rustc と同じ
  `"non-leaf"` を付けて直した (`f9d60cbf`)。直す前の集計は**向きが逆**だった
  (「toylang の処理は 6〜9%」と出たが、実際は 58〜98%)。
- **実行時間の大半は toylang で書かれた処理にある** (58〜98%)。最大の塊は
  POC の `LabelDict` の線形探索 (`archive` / `compact` の約 6 割)。ランタイム
  (`toylang_rt`) で目立つのは常時引く確保表 `prof_put` (`archive` の 6%) だけ。
- **`--release` のコンパイルは cranelift の約 24 倍** (1.94 s 対 80 ms)。97% が
  LLVM の中で、`-O2` 1.09 s + 機械語の生成 0.77 s を **1 スレッド**で回している。
  特定の pass が飛び抜けているわけではない。
- **縮める方向は並列化**。pipeline を軽くする案は効かないか、実行時間を払う。
  試作の結果:
  - (B) 最適化の後で分割して機械語の生成だけ並列にすると、1.94 s → 1.42 s。
    実行時間は変わらない。
  - (C) 最適化の前に分割して `-O2` も並列にすると、1.94 s → 0.60〜0.67 s
    (8 分割)。実行時間は +0.1% (雑音の範囲)。ただし CPU は 1.8 倍、RSS は
    2.4 倍、バイナリは +12% になる。
- 段取りは **B → C**。試作のコードはリポジトリに入れていない (§6 に作り方と
  落とし穴を残した)。

## 1. 条件

| | |
|---|---|
| 機械 | Apple M5 Ultra (30 コア)、macOS 27.0.1 |
| LLVM | 22 (Homebrew `llvm@22`)、inkwell 0.10 (`llvm22-1`) |
| 対象 | `poc/logsearch` (entry 1,648 行 + モジュール 68 ファイル / 23,933 行、IR 61,523 命令 / 787 関数) |
| 入力 | `poc/logsearch/log` の実ログ (562 ファイル / 137 MB) |
| アーカイブ | `archive` で作った 12 セグメント / 444,549 レコード / 25 MB を読み取り系で共有 |
| コンパイラ | release ビルドの `toy` / `compiler` (`--features llvm`) |

実行時間の比較は、各コマンドを変種ごとに交互に 5 回ずつ走らせた中央値で取り、
基準版との比と、9 コマンドの幾何平均で示す。9 コマンドは次のとおり。

- `scan`
- `archive`
- `verify`
- `query error`
- `query status=404`
- `query path~/wp-`
- `fields path 20` (索引)
- `fields path 20 scan` (全走査)
- `compact`

出力 (stdout、所要時間の行と一時ディレクトリを除く) は**全変種で基準版と一致した**。

## 2. 測り方

### 2.1 サンプラ

Instruments の Time Profiler をコマンドラインから使う (1 ms 間隔):

```bash
xctrace record --template 'Time Profiler' --output run.trace --launch -- ./prog ARGS...
xctrace export --input run.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' > run.xml
```

XML の各 `row` が 1 サンプルで、`tagged-backtrace` の `frame` 列が葉から順に
並ぶ。要素は初出で `id` を持ち、以降は `ref` で参照されるので、読みながら
`id` → 要素の表を作って解決する。スタックが取れなかったサンプルは
`tagged-backtrace` の代わりに `sentinel` を持つ。**この割合を最初に見ること**
(§3)。

集計は 2 通りで取る。

- **self**: 葉のフレームの関数ごとに数える。
- **inclusive**: 1 サンプルのスタックに現れた関数ごとに、1 回ずつ数える。

関数は次の 4 つに分類する。

- toylang: バイナリの**ローカル**シンボル `_toy_*` (`nm` で `t`)。`main` も含める。
- ランタイム: 同じバイナリのそれ以外のシンボル (`toylang_rt` の Rust と、
  公開されている `toy_*` ヘルパ)。
- システム: 他のバイナリ (libSystem など)。
- kernel: バイナリの無いフレーム。

短いコマンドは 5 回録って合算した。

### 2.2 コンパイラ

`--profile=compile` がフェーズの木を出す。LLVM の codegen は 4 段に分かれる
(`llvm_build` / `llvm_verify` / `llvm_O2` / `llvm_emit`、`f9d60cbf` で追加)。

```bash
target/release/toy build poc/logsearch --release --codegen=llvm -o /tmp/ls --profile=compile
```

pass ごとの内訳は、コンパイラ自身を §2.1 のサンプラで録り、各サンプルを
**スタック上で最も葉に近い pass のフレーム**に帰属させて取った。

- 新しい pass manager の pass は `llvm::<Name>Pass::run(` という形で現れる。
  間に挟まる `PassModel<..>` はインライン展開されて消えている。
- 古い pass manager の pass は `<Name>::runOnMachineFunction(` /
  `runOnFunction(` という形で現れる。
- `PassManager` / `*Adaptor` / `ModuleInlinerWrapperPass` /
  `DevirtSCCRepeatedPass` は器なので数えない。
- 段は、スタックに `runPasses` があれば `-O2`、`addPassesToEmitFile` /
  `LLVMTargetMachineEmitToMemoryBuffer` があれば機械語の生成とした。

## 3. フレームポインタ (直したもの)

最初の計測では、LLVM 版のサンプルの多くにスタックが無かった。

| | スタックの無いサンプル |
|---|---:|
| LLVM 版 `verify` | 97% |
| LLVM 版 `scan` | 70% |
| LLVM 版 `archive` / `compact` | 56〜57% |
| cranelift 版 `verify` | 0% |

サンプラはフレームポインタの鎖をたどる。LLVM の backend は、関数属性
`"frame-pointer"` が無いと `"none"` として扱い、`-O2` がフレームレコードを
省いていた。clang と rustc は Apple のターゲットで `"non-leaf"` を付ける。
同じものを、本体を持つ全関数に付けた。

- スタックの無いサンプルは 0〜8% になった。
- 実行時間の差は雑音の範囲だった (−1〜+1%)。

**直す前の集計は向きが逆だった。** スタックの無いサンプルは捨てられ、
スタックを保っていたランタイム (Rust) と libSystem の分だけが残っていた。
その結果、「toylang 6〜9%、ランタイムとシステム 90%」と出た。この誤った数字から
「LLVM で伸びが小さいコマンドは、時間がランタイム側にある」と結論し、
一度 [`AOT_LLVM.md`](AOT_LLVM.md) §2.7 に書いた。今は訂正してある。

## 4. 実行時間のプロファイル (LLVM `-O2`)

self は葉の関数の値。LLVM がインライン展開した関数は、呼び出し元に含まれて
見える (`query::search` の self が大きいのはそのため)。

| コマンド | toylang | ランタイム | システム | 大きいもの |
|---|---:|---:|---:|---|
| `archive` | 59% | 17% | 24% | `LabelDict::bump` 58% (inclusive)、うち `memcmp` 22% / `toy_mem_eq`。`prof_put` 6% |
| `compact` | 58% | 15% | 27% | `LabelDict::bump` 66% (inclusive)。`memcmp` 25% |
| `scan` | 75% | 6% | 19% | `cmd_scan` 47% (self)、`record::parse_line` 31%、open / read 15% |
| `verify` | 98% | 0% | 2% | `segfile::expand_all` 68%、`lsz::decode_frame` 20% |
| `query status=404` | 96% | 2% | 2% | `query::search` 73%、`decode_frame` 13% |
| `query error` | 98% | 0% | 2% | `query::search` 78%、`decode_frame` 19% |
| `query top=status` | 89% | 0% | 10% | `segfile::load_block` 62%、`decode_frame` 26% |
| `fields path scan` | 98% | 0% | 1% | `expand_all` 65%、`decode_frame` 14% |
| `fields path` (索引) | 92% | 2% | 5% | `cmd_fields_indexed` 59%、`load_block` 21% |
| `object status=404` | 83% | 0% | 17% | `load_block` 56%、`decode_frame` 26% |

読み取り:

- **`archive` / `compact` の約 6 割は `LabelDict::bump`**。中身の `find` は、
  `keys` / `values` の `Vec<String>` を頭から比べる線形探索。ラベルの値の数に
  比例して効く。POC 側のアルゴリズムの問題で、`(キー, 値)` で引く表にすれば
  消える (POC ROADMAP の 7)。
- **ランタイムの確保表 `prof_put` が `archive` の 6%**。これは、`free` を冪等に
  するために常時引いている番地 → サイズの表 (DROP-GLUE)。`archive` は次の回数を
  この表に通す。
  - 確保 1.54M 回
  - realloc 2.30M 回
  - free 1.50M 回

  1 回あたり ~26 ns。hash は奇数の乗数で bump の連続番地をよく散らしているので、
  偏りではない。表が大きくてキャッシュに乗らないことが効いている。
  `String::push` の伸長で、realloc が確保の 1.5 倍出ているのも一因
  (RT-ALLOC-TABLE-COST)。
- 読み取り系は 9 割以上が toylang の側 (検索ループと LSZ の展開) で、
  ランタイムはほぼ出てこない。cranelift との伸びの差 (1.45〜2.56 倍、§2.7) が
  どこから来るかは、まだ説明できていない。

## 5. コンパイル時間のプロファイル

### 5.1 フェーズ

`--profile=compile` の値 (3 回とも ±3%):

| | 全体 | codegen | 内訳 |
|---|---:|---:|---|
| cranelift (opt speed) | 80 ms | 25 ms | 関数ごとに全コアで並列 (user 296 ms) |
| LLVM `-O0` (debug) | 354 ms | 265 ms | IR 構築 24 / verify 9 / 機械語 222 |
| LLVM `-O2` (`--release`) | 1,940 ms | 1,890 ms | IR 構築 19 / verify 6 / **`-O2` 1,090** / **機械語 770** |

- フロントエンド (parse → 型検査 → lowering) は両者で同じで、約 50 ms。
- LLVM の IR を組み立てる段は 19 ms。
- 残りはすべて LLVM の中で、**1 スレッドで走る** (user ≒ wall)。

### 5.2 pass ごと

コンパイラのサンプル 1,986 ms のうち、`-O2` が 55.5%、機械語の生成が 39.0%、
それ以外 (フロントエンドと IR の構築) が 5.4%。

| 段 | pass | ms |
|---|---|---:|
| `-O2` | InstCombine | 229 |
| `-O2` | IndVarSimplify | 122 |
| `-O2` | Inliner | 93 |
| `-O2` | GVN | 52 |
| `-O2` | SimplifyCFG | 44 |
| `-O2` | CorrelatedValuePropagation | 43 |
| `-O2` | SLPVectorizer | 40 |
| `-O2` | JumpThreading / ConstraintElimination / SROA / IPSCCP / SCCP / EarlyCSE / LoopIdiom / LoopDeletion / LoopUnroll ... | 各 20〜37 |
| 機械語 | SelectionDAG (命令選択) | 187 |
| 機械語 | Greedy レジスタ割り当て | 130 |
| 機械語 | LPPassManager (codegen 前のループ pass) | 95 |
| 機械語 | MachineScheduler | 67 |
| 機械語 | LiveVariables / RegisterCoalescer / MachineLICM / LiveIntervals ... | 各 20 前後 |

飛び抜けた pass は無い。効いているのは、pipeline 全体を 1 スレッドで
回していることである。

## 6. 縮める検討 (試作して計測)

§5 の 1.94 s を対象に、次の 3 方向を試作した。

- (A) pipeline を軽くする
- (B) 機械語の生成だけ並列にする
- (C) `-O2` ごと並列にする

試作は `compiler/src/llvm/mod.rs` に足した実験コードで、環境変数で切り替える形。
リポジトリには入れていない。

### 6.1 (A) pipeline を軽くする — 効かない

| 変種 | コンパイル | `-O2` | 機械語 | 実行時間 (幾何平均) |
|---|---:|---:|---:|---:|
| 基準 (`default<O2>`、codegen `Default`) | 2,014 ms | 1,107 | 795 | — |
| `default<O1>` | 1,736 ms | 840 | 778 | +0.9% |
| codegen を `Less` に | 1,982 ms | 1,127 | 768 | (測らず) |
| `default<O1>` + codegen `Less` | 1,697 ms | 841 | 770 | (測らず) |
| `default<Os>` | 1,265 ms | 643 | 506 | +7.3% (`archive` / `compact` +18〜20%、`scan` +15%) |
| codegen を `None` に | 1,323 ms | 1,093 | 113 | **+194%** |

- `O1` は実行時間をほぼ保つが、14% しか縮まない。
- `Os` は縮むが、実行時間を払う。
- codegen を `None` にすると、命令選択が FastISel、レジスタ割り当てが fast になり、
  バイナリが 3 倍遅くなる。

### 6.2 (B) 機械語の生成だけ並列にする — 1.94 s → 1.42 s、実行時間は不変

`-O2` を済ませたモジュールを N 個に分け、N スレッドで object を作る (作り方は
§7.1)。関数は命令数で釣り合うように配る。

| 分割数 | 機械語 | 全体 |
|---:|---:|---:|
| 1 | 795 ms | 1.94 s |
| 2 | 442 ms | (`ld -r` が初回だけ 437 ms) |
| 4 | 314 ms | 1.53 s |
| 8 | 207 ms | 1.42 s |
| 16 | 174 ms | 1.38 s |

- 実行時間は −0.3% (雑音)。最適化後の IR は同じで、変わるのは関数の並び
  だけだから。
- 分割と bitcode の書き出しは 15 ms、`ld -r` は 14 ms。
- 16 分割で頭打ちになるのは、一番大きい関数 (5,000 命令) を持つ分割に律速される
  ため。
- **残る `-O2` の 1.09 s が全体の 78% になる。**

### 6.3 (C) `-O2` ごと並列にする — 1.94 s → 0.60〜0.67 s、実行時間 +0.1%

`-O2` の前に分ける、ThinLTO 風の形 (作り方は §7.2)。素直に分けると、分割の境界を
またぐインライン展開を失う。それを取り戻す手当てを順に足して、8 分割で測った。

| 変種 | 全体 | 実行時間 (幾何平均) | 目立つ差 |
|---|---:|---:|---|
| 小さい関数 (≤ 80 命令) を `available_externally` で全分割に写す ※ | 0.53 s | +2.1% | `scan` +4.5%、`verify` +4.6% |
| 写す閾値を 200 命令に ※ | 0.60 s | +1.0% | `scan` +10% |
| 呼び出し元が 1 つの関数を、呼び出し元と同じ分割に集める (クラスタの大きさに上限) | 0.69 s | +2.1% | `scan` +10% |
| 呼び出し元が 4 つまでの関数も集める | 0.64 s | +0.7% | `scan` +7〜9% |
| **写しを `available_externally` ではなく、分割ごとの internal にする** | **0.67 s** | **+0.1%** | なし |

※ の 2 行は、呼び出しグラフが空のまま測ったもの (§7.3 の 1)。どの関数も
クラスタにまとまらず、全関数が外部シンボルになっていた。

**`scan` が最後まで遅かった理由。** `LogReader::next_line` (132 命令、呼び出し元
2 つ) は、基準版では `cmd_scan` にインライン展開されていた。分割版では呼び出しの
まま残った。`otool -tV` で `cmd_scan` の `bl` を数えて確かめた (基準 0 回、分割版
2 回)。

- LLVM のインライナは、「static な関数への最後の呼び出し」に大きなボーナスを
  与える (展開すれば本体を消せるため)。
- `available_externally` の写しはローカルではないので、このボーナスが付かない。
- 写しを分割ごとの internal 関数にすると、各分割で「最後の呼び出し」になり、
  基準版と同じく展開された。
- 展開されずに残った写しは、分割ごとに本体が出る。そのためバイナリは大きくなる
  (459 KB → 512 KB、閾値 400 命令なら 599 KB)。

写す閾値と実行時間 (internal の写し、8 分割):

| 閾値 | 全体 | CPU (user) | 実行時間 (幾何平均) | `scan` |
|---:|---:|---:|---:|---:|
| 80 | 0.55 s | 2.73 s | +1.6% | +7.1% |
| 200 | 0.67 s | 3.45 s | +0.1% | −0.4% |
| 400 | 0.82 s | 4.54 s | −0.1% | −2.8% |

最終形 (internal の写し、閾値 200 命令、呼び出し元 4 つまで同じ分割、クラスタは
1 分割の負荷の半分まで) の、分割数ごとの値 (3 回):

| 分割数 | 全体 (wall) | CPU (user) | ピーク RSS |
|---:|---:|---:|---:|
| 1 (基準) | 1.94 s | 1.93 s | 170 MB |
| 4 | 0.87〜0.91 s | 2.68〜2.70 s | — |
| 8 | 0.58〜0.63 s | 3.33〜3.48 s | 416 MB |
| 12 | 0.56〜0.59 s | 4.29〜4.33 s | — |
| 16 | 0.49〜0.56 s | 4.40〜4.80 s | — |
| 24 | 0.43〜0.47 s | 5.29〜5.39 s | — |

**CPU が増える理由。** 小さい関数は、使う分割ごとに最適化し直される。8 分割の
`-O2` の合計は 2.3 s で、直列のときは 1.09 s だった。分割の中で参照されない写しを
`-O2` の前に消してみたが、変わらなかった (バイナリもバイト一致)。写しは、もともと
呼ばれているものしか効いていない。

**正しさ。** `interpreter/example/*.t` のうち `--release --codegen=llvm` で作れる
158 本を、基準版と 8 分割版で作った。終了コードと stdout はすべて一致した。

## 7. 試作の作り方と落とし穴

### 7.1 (B) 最適化後の分割

1. `-O2` の後、内部リンケージの関数と大域変数を、`External` + `Hidden` に変える
   (別の object から参照できるように)。
2. 名前の無い大域変数には名前を付ける (`__toy_part_g{n}`)。
3. 関数を命令数の大きい順に、いちばん軽い分割へ配る。
4. モジュールを bitcode でメモリに書く。
5. 各スレッドで、次を行う。
   - 新しい `Context` に bitcode を読む (`Module::parse_bitcode_from_buffer`)。
   - 受け持ち以外の関数の本体を捨てて、宣言にする (方法は §7.3 の 2)。
   - 大域変数の定義は分割 0 だけが持つ。ほかの分割では、
     `LLVMSetInitializer(g, null)` で宣言にする。
   - `TargetMachine` を作り直して、object を書く。

   `Target::initialize_native` はプロセスで 1 回だけ呼ぶ。
6. 試作では `ld -r` で 1 つの object にまとめて、driver を触らずに済ませた。
   実装するなら、driver (`link_executable`) が object を複数受け取れるようにすれば、
   `ld -r` は要らない。

### 7.2 (C) 最適化前の分割

§7.1 との違いは、分割の単位と、小さい関数の扱い。

1. **呼び出しグラフ**: 本体を持つ関数ごとに use list をたどり、使い手が命令なら、
   それを含む関数を呼び出し元とする。命令以外 (vtable などの定数) から参照される
   関数は、アドレスを取られた (escape) として扱う。
2. **クラスタ**: union-find で関数をまとめる。
   - 呼び出し元が 1 つの関数は、その呼び出し元と同じクラスタにする。
   - 呼び出し元が 4 つまでの関数は、呼び出し元たちと同じクラスタにする。
   - どちらも、小さい関数から順に、クラスタの命令数が「全体 / 分割数 / 2」を
     超えない範囲で行う。

   上限が無いと、`main` → 全 `cmd_*` → その下請けが 1 つのクラスタになり、
   1 つの分割に倍の負荷が載った (0.91 s)。
3. **配分**: クラスタを命令数の大きい順に、いちばん軽い分割へ配る。
4. **リンケージ**: 次のどれかに当たる関数だけを、`External` + `Hidden` にする。
   - escape する
   - 別の分割の大きい関数から呼ばれる
   - 小さい関数から呼ばれる

   それ以外は元のリンケージ (internal) のまま残し、`-O2` が消せるようにする。
5. **各分割**: 受け持ち以外の関数を、次のように扱う。
   - 小さく (≤ 200 命令)、escape しないもの: internal の写しにする。
   - 小さく escape するもの: `available_externally` にする。
   - それ以外: 宣言にする。
   - `main` は写さない。

   大域変数は、分割 0 以外では、定数なら `available_externally` (中身が見えて
   定数畳み込みが効く)、そうでなければ宣言にする。

   その後、各スレッドで `default<O2>` を回して object を書く。

### 7.3 落とし穴 (踏んだもの)

1. **inkwell の `BasicValueUse::get_user()` は、値を返す call を命令として返さない。**
   値の型に応じた enum (`IntValue` など) が返る。`InstructionValue` の場合だけを
   見ていたので、ほとんどの呼び出しを「escape」と数え、呼び出しグラフが空になって
   いた。C API の `LLVMIsAInstruction` → `LLVMGetInstructionParent` →
   `LLVMGetBasicBlockParent` で呼び出し元を引くこと。
2. **inkwell 0.10 には「関数の本体を捨てる」API が無い。** ブロックを 1 つずつ
   消すと、ほかのブロックからの参照が宙に浮く。次の手順で置き換えた。
   1. 同じ型の宣言を別名で作る。
   2. 引数と戻り値の属性を写す。`signext` / `zeroext` は ABI なので必須。
   3. `replace_all_uses_with` で使い手を宣言に付け替える。
   4. 元の関数を `delete` する (中で本体ごと消える)。
   5. 宣言を元の名前に戻す。
3. **小さい関数の写しが展開されると、その分割から、元の所有者の分割の
   internal 関数が呼ばれる。** 写しの callee まで `Hidden` にしないと、リンク時に
   未定義シンボルになる (`_toy_drop_glue_38`、`_toy_segfile__decode_block`)。
   そのため §7.2 の 4 に「小さい関数から呼ばれる」を入れた。
4. **`available_externally` の写しには、インライナの「最後の呼び出し」ボーナスが
   付かない** (§6.3)。internal の写しにする。
5. `toy build` のリンク失敗は、`cc` の終了コードしか言わない (`toy: \`cc\` exited
   with status 1`)。原因を見るには、`compiler --emit=object` で object を作り、
   手で `cc` に渡す。

## 8. 結論と段取り (LLVM-COMPILE-TIME)

1. **まず (B)。** コードの意味が変わらないので、検査は「同じ出力」だけで済む。
   1.94 → 1.42 s。driver が object を複数受け取れるようにする。
2. **次に (C)。** 1.42 → 0.6 s で、実行時間は変わらない。ただし分け方の規則
   (写す閾値、クラスタの上限) が実行時間に効き、その値は POC 1 本で決めたもの。
   入れるなら次を用意する。
   - consistency / example_consistency の LLVM レーンを、分割ありでも回す。
   - `--codegen-parts=N` を設ける。既定はコア数と 8 の小さい方にし、1 で直列に
     戻せるようにする。

   CPU 1.8 倍と RSS 2.4 倍は、`toy test` のように複数のビルドを並べる場面で
   効くので、並列ジョブ数との兼ね合いを決める。
3. (A) はやらない。`O1` の 14% は、(B) / (C) の後なら誤差になる。

## 9. 再現

- 実行時間の比較と、変種ごとのビルドは、§1 の条件で `toy build ... --release
  --codegen=llvm -o <変種>` を作り、§1 の 9 コマンドを交互に走らせる。
- 測定スクリプト (`xctrace` の録画と集計、変種の比較) はリポジトリに置いて
  いない。§2 の手順で作り直せる。
- 実ログ由来の値 (アドレス、ホスト名) は、ここには書いていない (CLAUDE.md の
  規約)。traversal の問い合わせは比較から外した。
