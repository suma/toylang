# SIMD — vector を型にし、intrinsic を最小限にする

> **状態: 設計のみ (未実装)**。前提は [`DATA_ORIENTED.md`](DATA_ORIENTED.md)
> の Phase 0 (`soa` 配列) と slice `&[T]`。
> 関連: [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) (intrinsic のエフェクト)、
> [`GUARD_ELISION.md`](GUARD_ELISION.md) (ベクトル化の前提条件)、
> [`BUILTIN_ARCHITECTURE.md`](BUILTIN_ARCHITECTURE.md) (builtin を足すコスト)。

## 設計を決める 2 つの制約

### 制約 1: builtin を 1 個足すコストが高い

builtin を 1 つ増やすには `BuiltinFunction` enum
([`frontend/src/ast/expr.rs`](../frontend/src/ast/expr.rs)) + 
`BuiltinFunctionSymbols` の intern + **`FULL_AST_CACHE_SCHEMA_VERSION` の bump**
([`frontend/src/cache.rs`](../frontend/src/cache.rs)) + 型検査シグネチャ +
`effects.rs::builtin_effect` の表 + 4 実行系の対応が要る。

`__simd_add_f64x2` / `__simd_add_i32x4` / ... と「op × lane 型 × lane 数」で
並べると数百個になり、**この設計は破綻する**。

### 制約 2: 4 実行系の答えが一致しなければならない

`compiler/tests/consistency/` の `assert_consistent` がこのプロジェクトの
背骨で、f64 の print が 3 バックエンドで食い違った過去の不具合と同種の
リスクが SIMD にはある (畳み込み順序・trap・lane 幅)。**意味論を先に固定
しないと必ず割れる。**

## 中心案: 演算子を lane-wise に効かせる

Zig の `@Vector` と同じく **vector を型にして、通常の演算子を lane-wise に
定義**する。これで intrinsic は「型と演算子で表現できないもの」だけに絞れる。

```rust
val a: f64x2 = __simd_splat_f64x2(1.5f64)
val b: f64x2 = __simd_load_f64x2(xs, i)
val c = a * b + a              # lane-wise。intrinsic ではなく普通の演算子
val s: f64 = __simd_reduce_add(c)
```

残る真の intrinsic は 10 個程度:

| intrinsic | 役割 |
|---|---|
| `__simd_splat_<T>(x)` | スカラー → 全 lane |
| `__simd_load_<T>(s: &[E], i)` / `__simd_store_<T>(s, i, v)` | slice ↔ vector |
| `__simd_extract(v, K)` / `__simd_insert(v, K, x)` | `K` は定数 lane index |
| `__simd_shuffle(a, b, [K...])` | 定数マスク |
| `__simd_select(mask, a, b)` | branch-free 選択 |
| `__simd_reduce_add / min / max / and / or(v)` | 水平畳み込み |
| `__simd_any(mask)` / `__simd_all(mask)` | 比較結果の縮約 |

`BuiltinFunction` に足す variant はこれだけで、cache schema の bump は 1 回で済む。

## 型

### まず 128bit 幅だけ

x86-64 の SSE2 と aarch64 の NEON は**無条件に存在する**ので、128bit に
限る限り**可搬性の問題がゼロ**になる。これが幅を絞る最大の理由 (AVX2 /
SVE は Phase 4)。

lane 型は既存 primitive に対応させて 9 種:

```
i8x16  u8x16   i16x8  u16x8   i32x4  u32x4   i64x2  u64x2   f64x2
```

- **`f32` が言語に無いので `f32x4` が出てこない。** SIMD の主戦場は
  `f32x4` なので、これは実用上の制約として大きい (下の「決めていない論点」)。
- `TypeDecl::Vector { lane, lanes }` を 1 つ足す。`Simd<f64, 2>` にしないのは
  const generics が未実装だから。`f64x2` を lexer で primitive 型名として
  認識するのが最小。
- **vector は 1 SSA 値**。struct と違って leaf 分解が要らず、
  **関数境界をそのまま渡れる** (compound より扱いが楽)。
- `__builtin_sizeof(f64x2) == 16`。

### 4 実行系への載せ方

既存パターン (`dyn Trait` の載せ方) にそのまま沿う:

| 実行系 | 実装 |
|---|---|
| tree-walker | `Object::Simd(Vec<Object>)` の素朴なループ。**正しさのオラクル** |
| IR VM | slot に lane 配列 |
| AOT / compiler JIT | cranelift の `I8X16` / `I32X4` / `F64X2` に直行 (0.131 は全部持つ) |
| interpreter JIT | silent fallback (`ScalarTy` に vector が無い) |

`compiler_lower` に置けば IR VM / AOT / compiler JIT の 3 つに効き、
tree-walker だけ別途 — CODE_MAP.md の構図そのまま。

## 意味論: 先に固定するもの

### 1. 畳み込み順序を仕様で固定する

`__simd_reduce_add` を pairwise tree にすると、tree-walker の素朴実装と
cranelift の縮約命令で f64 の結果が変わる。

**lane 0 → n の逐次加算に固定する。** reduce はループ末尾に 1 回しか
出てこないので、逐次でも性能は落ちない。`docs/language.md` に明記する。

### 2. lane-wise 演算は trap しない

スカラーの `u64 -` は underflow で panic し、`/` は 0 除算で panic する
(RUNTIME-TRAP)。lane ごとに guard を入れるとベクトル化の意味が消えるので:

- **SIMD の整数演算は wrap で定義する**
- **`__simd_div` は提供しない** (必要なら乗算逆数か scalar ループ)

`+` / `*` の overflow を「ビルドプロファイルに依らず wrap」と決めた前例と
同じ流儀。非対称なので `docs/language.md` の RUNTIME-TRAP 節に併記する。

### 3. native lane width を問う API を作らない

`__simd_native_lanes()` のようなものを入れると値がホストとバックエンドで
変わり、`assert_consistent` が壊れる。**幅は常にソースに書く。**

## `__` 名前空間の整理

**現状 `__` はコンパイラ合成名で埋まっている:**

| 合成名 | 出所 |
|---|---|
| `__old_N` | `parser/expr/primary.rs::parse_old_snapshot` |
| `__try_t_N` / `__try_v_N` / `__try_e_N` / `__try_conv_N` / `__try_err_N` | `parser/expr/mod.rs` (`?` の desugar) |
| `__iter_for_N` | `parser/stmt.rs` (iterator protocol) |
| `__ae_l_N` / `__ae_r_N` / `__dbg_N` | `parser/expr/macros.rs` |
| `__cmp_N` / `__coalesce_*_N` | `parser/expr/mod.rs` |
| `__su_N` | `parser/expr/primary.rs` (struct update) |
| `__ifval_dummy_N` / `__test_N` | `parser/expr/control.rs` / `program_parser.rs` |
| `__extern_io_*_status` | `core/std/io.t` (RUNTIME-IO) |

無条件にユーザへ開くと desugar と衝突するので、規約を明文化する:

| prefix | 所有者 | 規則 |
|---|---|---|
| `__builtin_*` | 処理系 | 既存。低レベル / unsafe 層 |
| `__simd_*` | 処理系 | intrinsic。**pure** |
| `__toy_*` | コンパイラ合成 | 上記の desugar 名を**ここへ改名**し、衝突源を 1 箇所に隔離 |
| その他の `__*` | ユーザ | 定義可。ただし安定性の保証は無い旨を `--explain` に書く |

### エフェクト

`__simd_*` は演算とレーン操作しかしないので `EffectSet::EMPTY` に置ける
(`effects.rs::builtin_effect`)。すると:

- `const fn` から呼べる (COMPILE-TIME-EVAL の適格性を壊さない)
- `never_allocates` を壊さない
- `requires` / `ensures` の述語に書ける (contract purity を壊さない)

例外は `__simd_load` / `__simd_store` で、それぞれ `RawRead` / `RawWrite`。

## 最適化戦略

段階を 4 つに分ける。**A → B が本命、C は狭く、D が toylang の差別化。**

### A. 明示 SIMD (ユーザと stdlib が書く)

上記の型と intrinsic。まずここだけを landing させ、正しさを
`assert_consistent` で固める。

### B. stdlib kernel の SIMD 化 — 費用対効果が最大

**ユーザコードを一行も変えずに効く。** stdlib は toylang で書かれているので、
これは処理系ではなくライブラリの変更になる。現在スカラーループで書かれて
いる明白な候補:

| 対象 | 場所 | 形 |
|---|---|---|
| `Vec<u8>::eq` | `core/std/collections/vec.t` | 16 バイト比較 + `__simd_all` |
| `Contains` / `Split` | `core/std/str_ops.t` | memchr 相当 |
| `CaseConvert` | `core/std/string.t` | ASCII 大小変換は完全に lane-wise |
| `sum` / `min` / `max` | `core/std/collections/vec.t` | reduce |
| `Vec<T>::sort` の小配列部分 | 同上 | 分岐削減 |

退行は `compiler/tests/example_consistency.rs` (全 example を 3 バックエンドで
突き合わせる) がその場で捕まえる。

### C. 限定的な自動ベクトル化 — 汎用はやらない

cranelift に自動ベクトル化は無いので、やるなら `compiler_lower` の IR
レベル。ただし**汎用の loop vectorizer は作らない**。認識するのは次の
狭いパターンだけ:

```
soa 配列  +  `for i in 0..n` の range loop
  + guard がゼロ (GUARD-ELISION 済み)
  + body が pure (EffectSet が EMPTY — call / print / heap なし)
  + ループ跨ぎ依存なし (i の単調な添字のみ)
```

**この 4 条件は既存の機構でそのまま判定できる**のが、この言語で自動
ベクトル化をやる場合の勝ち筋:

| 条件 | 判定に使うもの |
|---|---|
| SoA か | `ArraySlotInfo.layout` (DATA_ORIENTED Phase 0) |
| guard が無いか | `compiler_lower/src/contract_facts.rs::ContractFacts` |
| pure か | `frontend/src/type_checker/effects.rs::EffectTable` |
| 添字が単調か | ループ lowering (`compiler_lower/src/loops.rs`) が持つ induction 変数 |

**新しい解析を書かずに済む。** GUARD-ELISION が既に「境界チェックの除去」を
やっているので、ベクトル化の最大の障害が最初から無い。

### D. `--simd-report` — 「なぜベクトル化されなかったか」を聞ける

LLM_FEEDBACK_LOOP の思想 (`--explain` / `--api` / `--effects` / 型ホール) の
延長。**C の条件チェックリストがそのまま出力になる。**

```
$ cargo run -q -p compiler -- nbody.t --simd-report
nbody.t:42  step()  loop `for i in 0..n`
  vectorized: no
  - array `bodies` is AoS; add `soa` to make the field columns contiguous
  - bounds guard present at bodies[i]; add `requires i < n` or bound the range
nbody.t:58  advance()  loop `for i in 0..n`
  vectorized: yes (f64x2, 512 iterations + 0 scalar tail)
```

gcc の `-fopt-info-vec-missed` に相当するが、**言語側の直し方 (`soa` を
付けろ / `requires` を書け) を名指しで返す**点が違う。エージェントが回す
ループに合う形で、`--profile=mem` / `--effects` と同じ「聞けば答える CLI」の系譜。

## 段階

| Phase | 内容 | 規模 | 前提 |
|---|---|---|---|
| **0** | `soa [T; N]` | 小 | — (DATA_ORIENTED.md) |
| **1** | slice `&[T]` | 中 | — |
| **2** | vector 型 9 種 + lane-wise 演算子 + intrinsic 10 個 (128bit のみ) | 中〜大 | Phase 1 |
| **3** | stdlib kernel の置換 (戦略 B) + `--simd-report` (戦略 D) | 中 | Phase 2 |
| **4** | 限定自動ベクトル化 (戦略 C) / 256bit + feature detection | 大 | Phase 3 |

## 決めていない論点

1. **`f32` を言語に足すか。** SIMD の主戦場は `f32x4`。足さないなら SIMD の
   用途は整数・バイト処理と `f64x2` に限られる。**Phase 2 より前に決める必要がある** —
   後から足すと lane 型の組が変わり、型名と intrinsic の綴りが増える。
2. **256bit (AVX2) をどう入れるか。** `cranelift-native` はホストの ISA を
   見るので、AOT バイナリの可搬性と衝突する。選択肢は (a) `--target-cpu` で
   明示、(b) runtime dispatch (関数の multi-versioning が要る)、(c) やらない。
   Phase 4 まで判断を遅らせる。
3. **`__toy_*` への改名を今やるか。** 合成名の衝突は現時点では実害が無い
   (ユーザが `__old_0` を定義すれば壊れるが、誰もしない)。`__` をユーザに
   開くと決めた時点で必要になる。
4. **mask 型を分けるか。** `a < b` の結果を `bool` の vector とするか、
   lane 型と同幅の整数 vector (全 1 / 全 0) とするか。cranelift は後者。
   前者にすると `__simd_select` の型が綺麗になるが、変換が要る。
