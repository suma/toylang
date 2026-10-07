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

## 3. ビルドの仕方

LLVM 22 は Homebrew の `llvm@22` (keg-only) を使う。`llvm-sys` は
`LLVM_SYS_221_PREFIX` で場所を知る。機械ごとに違うので
`.cargo/config.toml` には書かない:

```bash
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p compiler --features llvm
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p toy --features llvm
```
