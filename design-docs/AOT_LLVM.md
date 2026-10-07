# AOT_LLVM — AOT の 2 つ目のバックエンドとしての LLVM

> 状態: **L0 と L1 が landing、L2 の大半も (2026-10-07)**。
> `interpreter/example/` の 158 本のうち **143 本が cranelift と stdout・
> stderr・終了コードまで一致**、12 本は未対応の命令で断る (closure /
> 間接呼び出し / SIMD)、3 本の差は backtrace が出ないこと (L3) と
> ポート番号 (毎回違う)。決定の経緯は `todo.md` の AOT-LLVM 項。

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

**`--release` の既定を LLVM に切り替えるのは L5 まで待つ。** LLVM が
全命令を扱えるようになる前に切り替えると、既存の `--release` ビルド
(POC の release ビルド、テスト) が「未対応の命令」で落ちる — feature
無しなら全部エラーになる。それまで `--release` は今までどおり
cranelift で、契約を外すだけである。

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

## 3. ビルドの仕方

LLVM 22 は Homebrew の `llvm@22` (keg-only) を使う。`llvm-sys` は
`LLVM_SYS_221_PREFIX` で場所を知る。機械ごとに違うので
`.cargo/config.toml` には書かない:

```bash
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p compiler --features llvm
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo build -p toy --features llvm
```
