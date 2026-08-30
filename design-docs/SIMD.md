# SIMD — vector を型にし、intrinsic を最小限にする

> **状態: Phase 2 landing 済み (2026-08-30)**。仕様の正本は
> [`docs/language.md`](../docs/language.md) の「SIMD vectors」節。
> **Phase 0 (`soa` 配列) / Phase 1 (slice `&[T]`) は前提から外した** —
> `__simd_load` / `__simd_store` を `ptr` + 要素 index にしたので、
> `Vec<T>` / `String` が既に持っている raw ポインタにそのまま乗る。
> slice が入ったら受け口を足せばよい。
> **論点 1 は解決済み (2026-08-30)**: `f32` を言語に足した (SIMD-F32)。
> `f32x4` は lane 型の 1 つになった。
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

lane 型は既存 primitive に対応させて 10 種:

```
i8x16  u8x16   i16x8  u16x8   i32x4  u32x4   i64x2  u64x2   f32x4   f64x2
```

- **`f32` は 2026-08-30 に言語に足された** (論点 1 の解決)。scalar `f32`
  の算術・比較・`as` cast・print が 3 バックエンド一致で動く
  (example: `interpreter/example/float32.t`、pin:
  `compiler/tests/consistency/float32.rs`)。`f32x4` が SIMD の主戦場。
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

### 3. AOT の ISA は baseline 固定 (native ではない)

`make_object_module()` は `cranelift_native::builder()` ではなく
`isa::lookup(Triple::host())` を使う。前者は**ビルドマシンの CPU 機能を
検出して有効化する** (AVX2 / BMI 等) ので、生成物が「ビルドしたマシン
以上の CPU」を要求しうる — 新しい x86-64 で作ったバイナリが古い
x86-64 で落ちる、という形になる。**これは SIMD とは無関係に元から
あった穴**だが、stdlib が SIMD を使うようになって表面化しやすくなった。

baseline にしても**この言語のベクタは何も失わない**: 128bit で止めた
のは SSE2 (x86-64) と NEON (aarch64) がどちらも baseline だからで、
実際 baseline ISA でも `fmul.2d` / `bsl.16b` は出る。baseline を超える
のは 256bit (AVX2) のときで、そこで初めて runtime dispatch が要る
(論点 2)。

JIT (`jit.rs`) は `cranelift_native` のまま — 生成コードがそのマシンから
出ないので、可搬性を守る相手が居ない。

### 4. native lane width を問う API を作らない

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

### B. stdlib kernel の SIMD 化 — 費用対効果が最大 (landing 済み)

**ユーザコードを一行も変えずに効く。** stdlib は toylang で書かれているので、
これは処理系ではなくライブラリの変更になる。

置換したもの (2026-08-30):

| 対象 | 場所 | 形 |
|---|---|---|
| `String::eq` / `Vec<u8>::eq` | `string.t` / `collections/vec.t` | 16 バイト比較 + `__simd_all` |
| `CaseConvert` (`to_upper` / `to_lower`) | `string.t::fold_ascii_case` | ASCII 大小変換は完全に lane-wise |
| `Contains` / `Split` | `string.t` | needle / sep の先頭バイトを 16 バイトずつ走査 (memchr) |

**実測** (AOT、4096 バイトの文字列、aarch64、中央値):

| kernel | scalar | SIMD | |
|---|---|---|---|
| `eq` | 0.26s | 0.02s | **13x** |
| `to_upper` | 1.38s | 0.08s | **17x** |
| `contains` (先頭バイトが稀) | 1.93s | 0.12s | **16x** |
| `contains` (先頭バイトが 26 バイト周期) | 0.43s | 0.28s | 1.5x |
| `split` | 0.93s | 0.63s | 1.5x |

読み方に注意が要る数字が 2 つある。

**`to_upper` の 17x は SIMD だけの効果ではない。** 内訳は
scalar 1.38s → **一括確保 + `mem_copy` に変えて 0.71s** (1.9x) →
**lane-wise fold で 0.08s** (さらに 8.9x)。byte ごとの `push` を
やめないとベクタ経路に届かないので、この 2 つは分離できない。

**memchr 形 (`contains` / `split`) は入力で 10 倍変わる。** skip は
「16 バイトの窓に先頭バイトが 1 つも無ければ窓ごと捨てる」ので、
先頭バイトが稀なら 16x、密なら naive 比較に落ちて 1.5x。`split` の
1.5x はさらに part ごとの `substring` 確保が支配的なため
(走査自体はもっと速くなっている)。

**全テストの実行時間は変わらない** (10.64s → 10.50s、ノイズの範囲)。
テスト中の文字列はほぼ 16 バイト未満で、ベクタ経路に入らない。
長い入力の pin は `compiler/tests/consistency/simd.rs` の
「Stdlib kernels」節にある (chunk 境界と tail の両方を踏む)。

**まだ手を付けていない候補**:

| 対象 | 場所 | 形 |
|---|---|---|
| `sum` / `min` / `max` | `collections/vec.t` | reduce。**そもそも API が無い**ので追加から |
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

| Phase | 内容 | 状態 |
|---|---|---|
| **0** | `soa [T; N]` | 未着手 (DATA_ORIENTED.md)。SIMD の前提ではなくなった |
| **1** | slice `&[T]` | 未着手。`ptr` で代替したので前提ではない |
| **2** | vector 型 + lane-wise 演算子 + intrinsic | **landing 済み** (下記) |
| **3** | stdlib kernel の置換 (戦略 B) | **landing 済み** (下記)。`--simd-report` (戦略 D) は未着手 |
| **4** | 限定自動ベクトル化 (戦略 C) / 256bit + feature detection | 未着手 |

## Phase 2 で実際に入ったもの (2026-08-30)

設計からの差分は 4 つ。いずれも「4 実行系が一致しなければならない」
という制約 2 から出た。

### 1. lane 型は 5 種 (10 種ではない)

`f64x2` / `f32x4` / `i32x4` / `i64x2` / `u8x16`。表を全部埋めるより、
64/32/8bit と float/signed/unsigned を一通り踏む組を先に 4 実行系で
通した。`i64x2` が入っているのは**比較の結果型** — `f64x2 < f64x2` は
lane 幅の等しい整数 vector を返す必要があり、他に置き場所が無い。
残り 5 種 (`i8x16` / `i16x8` / `u16x8` / `u32x4` / `u64x2`) は
`VectorType` / `VecTy` に行を足すだけ。

型は `TypeDecl::Vector(VectorType)` — 設計note の `{ lane, lanes }`
ではなく閉じた enum にした。幅が 128bit 固定で lane 行列が表である以上、
enum なら `f64x3` が表現不能になり、バックエンドごとに弾く必要が無い。

### 2. load / store は `ptr` + **要素 index**

slice が無いので `__simd_load(p: ptr, i: u64)`。`i` は**要素**番号で、
lane `k` は `(i + k) * lane_bytes` を読む。`__builtin_ptr_read` の
offset が**バイト**なのとは違うので、`docs/language.md` に明記した。
これで `Vec<T>` の `data: ptr` にそのまま載り、戦略 B (stdlib kernel の
置換) が Phase 1 を待たずに着手できる。

### 3. 型名サフィックスは付けない

`__simd_splat_f64x2` ではなく `__simd_splat`。型は**型検査器が call に
焼き込む** (`type_checker/simd.rs::stamp_simd_call` が合成の `u64`
引数を追加する) ので、4 実行系はどれも引数リストから読むだけで済む。
`__builtin_ptr_read` が「各 lowering で let 束縛を特別扱いする」形で
同じ問題を解いているのに対し、AST で 1 回やる方が安い。文脈は注釈でも
演算子の相手でもよい (`v & __simd_splat(15u8)` が通る)。

intrinsic は 13 個 (`__simd_shuffle` は定数マスク配列が要るので見送り)。

### 4. `<<` / `>>` の右辺はスカラー

cranelift のベクタシフトは量をスカラーで取る (SIMD ISA も同じ)。
lane ごとに違う量でシフトする形は入れていない。

### IR VM の slot 幅を 16 バイトにした件 — 測って払うと決めた

ベクトルは IR で単一の値 (struct のように leaf 分解されない) なので
1 slot に収まる必要があり、`RawSlot` を 8 → 16 バイトに広げた。
**代償はベクトルを使わないプログラムにも及ぶ**ので、4 種のワークロードで
実測した (IR VM、release、中央値):

| workload | 8 byte | 16 byte | 差 |
|---|---|---|---|
| call 中心 (`fib(30)`) | 2.52s | 2.57s | +2.0% |
| ループ + 算術 | 2.43s | 2.56s | **+5.3%** |
| struct 中心 | 1.95s | 1.94s | −0.5% |
| stdlib (`Vec` push/get) | 1.05s | 1.08s | +2.9% |

**典型 2〜3%、最悪 5.3%。** 最初に `fib` 1 本で見た「6%」は上振れだった。

消す場合の設計は「値 arena を `n_values + n_locals` 分だけフレームに持ち、
slot にはその index を入れる」形だが、**呼び出し境界でベクトルをコピーする
必要があり**、フレーム局所の index が呼び出しを跨ぐという新しいバグの
クラスを持ち込む。2〜3% と引き換えにする価値は無いと判断した。

再考する条件: IR VM がプロファイルの主役になったとき、または
lane 型が増えて 16 バイトを超える幅 (256bit) を入れるとき。

### 実装サイト

| 層 | 場所 |
|---|---|
| 型・lane 表 | `frontend/src/type_decl.rs::VectorType` |
| intrinsic 定義 | `frontend/src/ast/expr.rs::SimdOp` |
| 型検査 + 型の焼き込み | `frontend/src/type_checker/simd.rs` |
| tree-walker (オラクル) | `interpreter/src/evaluation/simd.rs`、値は `object.rs::SimdValue` |
| lowering (IR VM / AOT / compiler JIT) | `compiler_lower/src/simd.rs`、IR は `compiler_ir::VecTy` + `InstKind::Simd*` |
| IR VM 実行 | `compiler_vm/src/simd.rs` (slot は 8 → 16 バイトに拡げた) |
| cranelift codegen | `compiler/src/codegen/simd.rs` |
| 表示 | `compiler/runtime/toylang_rt` の `toy_print_vec` / `toy_to_string_vec` |
| 一致テスト | `compiler/tests/consistency/simd.rs`、tree-walker 単体は `interpreter/tests/simd_tests.rs` |

### 残っている穴

- **`__simd_shuffle`** — 定数マスク配列の受け取りが要る
- **lane 型 5 種の追加** — 表を埋めるだけ
- **`[f64x2; N]`** — vector を配列要素にする経路は未整備
  (`array_layout.rs` の 8 バイト leaf slot に収まらない)
- **`var v: f64x2` (初期化子なし)** — ゼロ vector の IR 定数が無いので
  拒否。`__simd_splat(0.0f64)` と書く
- **interpreter 側 JIT** — silent fallback (`ScalarTy` に vector が無い)
- ~~**IR VM の slot 幅**~~ — **測って払うと決めた (2026-08-30)**。

## 決めていない論点

1. ~~**`f32` を言語に足すか。**~~ **解決済み (2026-08-30): 足した。**
   SIMD の主戦場である `f32x4` を lane 型に含めるため、scalar `f32`
   を primitive として追加 (lexer `1.5f32` / `TypeDecl::Float32` /
   IR `Type::F32` / cranelift `F32`)。暗黙 widening は無し (`as` で明示、
   NUM-W と同じ流儀)。残りは f32x4 の intrinsic 名に `f32x4` が増える
   だけ。
2. **256bit (AVX2) をどう入れるか。** `cranelift-native` はホストの ISA を
   見るので、AOT バイナリの可搬性と衝突する。選択肢は (a) `--target-cpu` で
   明示、(b) runtime dispatch (関数の multi-versioning が要る)、(c) やらない。
   Phase 4 まで判断を遅らせる。
3. **`__toy_*` への改名を今やるか。** 合成名の衝突は現時点では実害が無い
   (ユーザが `__old_0` を定義すれば壊れるが、誰もしない)。`__` をユーザに
   開くと決めた時点で必要になる。
4. ~~**mask 型を分けるか。**~~ **解決済み (2026-08-30): 後者。**
   `a < b` は lane 幅と同じ整数 vector (全 1 / 全 0) を返す。cranelift の
   表現そのままなので比較と `__simd_select` の間に変換が入らない
   (`bitselect` に渡すときの `bitcast` は実行時 no-op)。専用 mask 型は
   lane 型ごとに増えるうえ、変換規約を別に決める必要がある。
