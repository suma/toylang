# DATA-ORIENTED — 配列の layout をユーザが選べるようにする

> **状態: 設計のみ (未実装)**。実装サイトの見込みは
> [`compiler_lower/src/array_access.rs`](../compiler_lower/src/array_access.rs) と
> [`compiler_lower/src/array_layout.rs`](../compiler_lower/src/array_layout.rs)。
> SIMD 側の設計は [`SIMD.md`](SIMD.md) にあり、本文書の Phase 0 が
> その前提になっている。

## なぜ

DoD (data-oriented design) の実務は「同じアルゴリズムのまま、データの
並べ方だけを変えて測る」ことに尽きる。Zig の `MultiArrayList` はこれを
ライブラリで提供するが、**API が変わる** — `list.items[i].x` が
`list.items(.x)[i]` になり、AoS と SoA を差し替えて測るには呼び出し側を
書き換える必要がある。

toylang には、この API 変更を**避けられる構造上の理由**がある。

## 効いてくる 3 つの事実

### 事実 1: IR は既にスカラー化されている

[`compiler_ir/src/layout.rs::flatten_compound_leaf_types`](../compiler_ir/src/layout.rs)
の通り、struct / tuple / enum は SSA 値として存在せず leaf ごとの local に
分解される。**関数ローカルの struct は既に SoA** であり、AoS なのは
「配列に入れたとき」だけ。

つまり `val p = ps[i]` で要素をまるごと取り出す操作は、AoS でも SoA でも
「leaf local を n 個埋める」という同じ形になる。**layout を変えても
要素アクセスのコストモデルが変わらない**。

### 事実 2: AoS ↔ SoA は添字式の掛け算の順序だけ

現状の添字計算は 2 段になっている:

| 層 | 式 | 場所 |
|---|---|---|
| lowering | `leaf_idx = element_index * leaf_count + j` | `array_access.rs` (`leaf_idx` の 4 箇所) |
| codegen | `byte_off = leaf_idx * stride` | `compiler/src/codegen/lower_inst.rs` の `ArrayLoad` / `ArrayStore` |

`j` はその要素の何番目の leaf かで、`leaf_count` は
`array_layout.rs::leaf_scalar_count`。SoA はこの式を

```
AoS: leaf_idx = i * leaf_count + j
SoA: leaf_idx = j * length     + i
```

に入れ替えるだけになる。**`InstKind::ArrayLoad` / `ArrayStore` の形も
codegen も一切変わらない** — codegen は leaf index を受け取って stride 倍
するだけで、要素の切り方を知らないため。変更は lowering の 2 ファイルに
閉じる。

### 事実 3: SoA は棚上げ中の pack 問題も解く

`array_layout.rs::ARRAY_LEAF_STRIDE` のコメントにある通り、compound 要素の
配列は leaf あたり 8 バイト固定で、`[PackedRgba; N]` が 4 バイトではなく
32 バイト/要素を食う (NUM-W-AOT-pack Phase 2 が未着手)。

SoA にすると**各列が同型になる**ので、列ごとに `elem_stride_bytes` を
その leaf の実サイズに落とせる。AoS のままでこれをやると要素内のパディングと
アラインメントを扱う必要があるが、SoA では列ごとに独立に決められる。
**Phase 2 は SoA 側では自然に解ける。**

## 中心案: `soa` を配列型の修飾子にする

```rust
struct Point { x: f64, y: f64, mass: f64 }

val ps: soa [Point; 1024]      # SoA: x[0..1024] | y[0..1024] | mass[0..1024]
val qs: [Point; 1024]          # AoS: 従来どおり

ps[i].x = 1.0f64               # 書き方は同一
val p = ps[i]                  # 要素まるごとの取り出しも同一 (事実 1)
```

**型検査から見て `soa [Point; N]` と `[Point; N]` は同じ型**にする。
要素型は `Point` のままで、`soa` は値の意味ではなく置き方の指定。したがって

- 型検査器・パターンマッチ・move check・REGION 検査に変更が要らない
- ユーザは `soa` を付け外しして計測できる (DoD の実務そのもの)
- 誤って「SoA 型」と「AoS 型」の 2 つの型ができて API が割れることがない

### 実装の見込み

| 変更点 | 場所 |
|---|---|
| `soa` トークンと配列型パーサ | `frontend/src/lexer.l`, `frontend/src/parser/types.rs` |
| `TypeDecl::Array` に layout フラグ | `frontend/src/type_decl.rs` (`source_name` も対応) |
| `ArraySlotInfo` に `layout: Aos \| Soa` | `compiler_ir/src/lib.rs` |
| 添字式の分岐 | `compiler_lower/src/array_access.rs` |
| 列ごとの stride / base offset | `compiler_lower/src/array_layout.rs` |
| tree-walker | 配列表現が `Vec<Object>` なので**変更不要** (観測できる差が無い) |

tree-walker が変更不要なのは重要で、**layout を変えても答えが変わらない**
ことのオラクルがそのまま手に入る。`assert_consistent` は
「同じプログラムを `soa` 有り / 無しで走らせて一致」を pin すればよい。

### enum の SoA

`soa [Shape; N]` は tag 列と payload 列が分離する。`__builtin_sizeof` の
enum 規約 (u64 タグ + 全 variant の payload 連結) が既にフラットなので、
leaf 分解の枠にそのまま乗る。**tag だけを舐めるループ**が cache 効率で
効き、Zig の `MultiArrayList` が union に対してできないところを超えられる。

ただし `leaf_scalar_count` は現在 `Type::Enum(_) => 1` (配列要素として
未サポート) なので、これは配列要素としての enum 対応とセットになる。
Phase 0 のスコープ外。

## slice が前提条件になる

「x 列だけを関数に渡す」「2 要素ずつ読む」時点で `&[T]`
(todo.md の `slice 型 &[T]` ★) が要る。

```rust
fn total_mass(ms: &[f64]) -> f64 { ... }
total_mass(ps.mass)      # SoA なら連続。AoS では stride が合わず型エラー
```

**SoA も SIMD も slice を待っている**ので、todo.md の優先度を ★ から ★★ に
引き上げるのが妥当。「SoA でしか書けない関数」が型で表現できることが、
SoA を単なる最適化フラグ以上のものにする。

## 採らない案

### リフレクション builtin で stdlib 側に `SoaVec<T>` を書く

`__builtin_field_count(T)` / `__builtin_field_offset(T, i)` を足せば、
allocator を stdlib に置いたのと同じ流儀で SoA コンテナをユーザ空間に
書ける — ように見える。**書けない。**

`__builtin_ptr_read` は**型注釈が唯一の shape の情報源**
(`let_lowering.rs::lower_let_builtin_ptr_read`) なので、実行時に決まる
field index で読み書きする形が表現できない。コアに `soa` を入れる方が素直。

### hot / cold フィールド分割の属性

属性構文そのものが言語に無い。`soa` があれば用途の大半を吸収する。

### entity index の newtype (`Idx<T>`)

**言語変更は要らない** — tuple struct (NEWTYPE) で今日書ける。
`interpreter/example/linked_list_arena.t` が既に arena + index の形。
必要になったら `core/std/slotmap.t` を置く話であって、本文書の範囲外。

## 段階

| Phase | 内容 | 規模 |
|---|---|---|
| **0** | `soa [T; N]` (scalar / struct / tuple 要素)。列ごとの tight pack 込み | 小 |
| **1** | slice `&[T]` — SoA の窓 | 中 |
| **2** | `soa` な heap コンテナ (`SoaVec<T>`) — Phase 1 + generics の上に stdlib で | 中 |
| **3** | 配列要素としての enum + tag 列の分離 | 中 |

**Phase 0 は単体で価値があり、SIMD をやらなくても無駄にならない。**
事実 2 のおかげで変更が lowering の 2 ファイルに閉じ、
「`soa` の有無で答えが変わらない」を `assert_consistent` で pin すれば完了する。

## 決めていない論点

1. **`soa` の綴りと位置** — `soa [Point; N]` (前置) か `[Point; N] soa` (後置) か。
   前置は「置き方の修飾」に読め、後置は既存の型構文を壊さない。
2. **`soa` を heap コンテナへどう伝えるか** — Phase 2 で `Vec<T>` と
   `SoaVec<T>` を別型にするのか、`Vec` に layout パラメータを持たせるのか。
   後者は const generics 相当が要る。
3. **要素まるごとの書き込み (`ps[i] = p`)** — leaf ごとに散らばった store に
   なる。SoA で「要素単位の更新が主」なワークロードは AoS より遅くなるので、
   `--simd-report` (SIMD.md) と同じ流儀で**警告を出すか**は未決。
