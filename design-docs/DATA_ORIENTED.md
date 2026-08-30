# DATA-ORIENTED — 配列の layout をユーザが選べるようにする

> **状態: Phase 0 (2026-08-30 landing) / 0.5 未着手**。実装サイトは
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
SoA: slot = columns[j], index = i
```

に入れ替えるだけになる。**Phase 0 の実装はこの形をさらに単純化した**:
単一 slot に layout flag を持たせる代わりに **leaf ごとに独立した
`ArraySlotId` (列) を確保**する。各列は「その leaf 型の homogeneous
scalar 配列」という既存の形なので、`InstKind::ArrayLoad` / `ArrayStore`
も codegen も IR VM も**一切変わらない** — 変更は `compiler_lower` だけ
に閉じた (事実 2 の主張よりさらに強い)。`ArraySlotInfo` に layout
flag を足す案はこの方式で不要になった。

**ただし uniform 8 バイト列 (現行の `ARRAY_LEAF_STRIDE` 規約) は
Phase 0 の実装範囲。** 列ごとの tight pack (事実 3) は列ごとに stride
が違うので codegen の `ArrayLoad` / `ArrayStore` にも SoA 分岐が要る
と設計では予想していたが、列方式では各列が独自の
`elem_stride_bytes` を持てるので **0.5 は stride の切り替えだけ**に
縮む (下の段階表)。

### 事実 3: SoA は棚上げ中の pack 問題も解く

`array_layout.rs::ARRAY_LEAF_STRIDE` のコメントにある通り、compound 要素の
配列は leaf あたり 8 バイト固定で、`[PackedRgba; N]` が 4 バイトではなく
32 バイト/要素を食う (NUM-W-AOT-pack Phase 2 が未着手)。

SoA にすると**各列が同型になる**ので、列ごとに `elem_stride_bytes` を
その leaf の実サイズに落とせる。AoS のままでこれをやると要素内のパディングと
アラインメントを扱う必要があるが、SoA では列ごとに独立に決められる。
**NUM-W-AOT-pack Phase 2 (compound 要素の pack) は SoA 側では自然に解ける**
— 本文書の段階では Phase 0.5 として回収する。

## 中心案: `soa` 前置修飾子 — stack と heap で別の仕組み

```rust
struct Particle { x: f64, y: f64, mass: f64 }

val ps: soa [Particle; 1024]    # 静的・スタック slot・確保なし (Phase 0)
val qs: [Particle; 1024]        # AoS: 従来どおり
val bodies: soa Vec<Particle>   # 動的・単一ヒープ領域を列に区切る (Phase 2)

ps[i].x = 1.0f64                # 書き方はどれも同一
val p = ps[i]                   # 要素まるごとの取り出しも同一 (事実 1)
```

**構文は 1 つでも、下げ先の仕組みは 2 つ**に分ける (未決 1・2 を閉じた、
2026-08-30)。判断の原則:

> **layout は、値が API を越えないなら binding の属性、
> 越えるなら nominal identity の一部。**

### `soa [T; N]` — 同じ型 + binding の layout フラグ (Phase 0)

**型検査から見て `soa [Point; N]` と `[Point; N]` は同じ型**にする。
要素型は `Point` のままで、`soa` は値の意味ではなく置き方の指定。したがって

- 型検査器・パターンマッチ・move check・REGION 検査に変更が要らない
- ユーザは `soa` を付け外しして計測できる (DoD の実務そのもの)
- 誤って「SoA 型」と「AoS 型」の 2 つの型ができて API が割れることがない

これが成立するのは **layout 情報が関数の外に出ないから**。すべての
アクセスサイトが同じ関数内にあり、`Binding::Array` のフラグが唯一の
情報源になる。配列全体が別の binding / 別の関数へ渡る経路は要素ごとの
物質化 (leaf local に詰めて詰め直す) を通るものとする — **layout が違う
2 つの binding 間で backing slot を共有すると相互に誤読する**ので禁止。
混在コピーが頻出するプログラムはそもそも AoS で書くべきで、物質化経路は
「遅いが正しい」だけで十分。

#### Phase 0 の性能の核心: `ps[i].f` の単列 shortcut

現状 `lower_slice_access` は compound 要素を**全 leaf 読み込み**してから
field access が該当 local を拾う (`array_access.rs` の per-leaf ループ)。
このまま SoA にしても 1 要素アクセスが全列に触れるので**帯域は減らない**
(メモリ削減だけの SoA になる)。

`ps[i].x` は FieldAccess(SliceAccess) の chain として降ってくるので、
field 名 → leaf `j` を compile time に解き、**1 本の `ArrayLoad`**
(SoA: 列 slot `j` + 要素 index / AoS: `i * leaf_count + j`) に落とす。
特定 field だけを舐めるループが速くなるのはこれが効いてからで、DoD の
実利の本体。

**実装時に判明した事実**: `ps[i].f` は compiled lane では SoA 以前に
**一切動いていなかった** (`resolve_field_chain` が SliceAccess root を
拒否 — tree-walker だけが対応)。だから shortcut は AoS / SoA 両対応の
新規機能であり、AoS 側には `nested_struct_array_test.t` /
`struct_array_test.t` という「動くはずの example が ERROR_EXAMPLES
行き」だった分があった (Phase 0 landing で両方復活し skip list から
外した)。

### `soa Vec<T>` — stdlib `SoaVec<T>` への sugar (Phase 2)

`soa Vec<Particle>` は型検査器が stdlib (`core/std/collections/soa_vec.t`)
の `SoaVec<T>` に書き換える — `?` / `??` / struct update と同じ流儀で
**バックエンドは砂糖を見ない**。call surface (`get` / `set` / `push` /
`pop` / `size` / `__getitem__` / `__setitem__` / iter) を `Vec<T>` と同一形に
保つので、「`soa` を付け外して測る」は型注釈 1 行の差で済み、API は割れない。

#### same-type にできない理由 (未決 2 の決定)

`Vec` は値が関数境界を越え、layout が**観測可能**になる:

- `as_ptr()` の返す番地の意味 (AoS の要素先頭か、列区切りの先頭か)
- grow 時のコピー方法 (下記 — 単純 realloc は列を撒き散らす)
- Drop が解放する領域
- `retains(N)` / `--profile=mem` のバイト数 (tight pack 後は AoS/SoA で値が違う)

same-type にすると全 receiver が両 layout を扱う runtime tag と分岐を要求
する。stack 配列が same-type で済むのは上の観測可能性が無いからで、heap は
原則の逆の側に落ちる。よって **`Vec<T>` に layout パラメータ (const generics
相当) を持たせる案は採らず、別 nominal 型 + sugar** とする。

#### 「列ごとに確保」ではなく単一領域の列分割

列ごとの独立確保は列数 = leaf 数だけ `ptr` field を要し、**field 数が
T 依存の struct** になる。`Box<T>` / `Vec<T>` / iterator 群が
「T を field に現さない」規律を守っているのは per-monomorph struct layout
を避けるためで、ここで破るわけにはいかない。Zig `MultiArrayList` と同じ
**1 確保を列に区切る**方式:

```
buffer: [col_0 × cap][col_1 × cap]...[col_{k-1} × cap]
leaf j of elem i  →  byte_off = prefix_j * cap + i * stride_j
```

`prefix_j` (前方列の stride 累積) と `stride_j` は monomorph 時点の定数、
`cap` だけ runtime。struct は `Vec` と同じ 4 field のまま:

```rust
struct SoaVec<T> {
    data: ptr,       # 単一バッファ、列に区切る
    len: u64,
    cap: u64,
    elem_size: u64,  # AoS 換算の 1 要素バイト数 (総量計算用)
}
```

Drop / `with allocator` / REGION / move check は `Vec<T>` と同一に動く
(確保 1 本、解放 1 本)。

#### builtin 3 個: `__builtin_soa_read` / `__builtin_soa_write` / `__builtin_soa_grow`

erased generic な stdlib には per-leaf offset が書けない (下の
「採らない案」の通り)。だが AOT-COMPOUND-PTR-RW の monomorph 展開
(`compute_leaf_layout` — compound `__builtin_ptr_read/write` を per-leaf に
展開する機構、`let_lowering.rs` / `expr.rs`) がまさにこのための鉤:

- `val v: T = __builtin_soa_read(p, elem_index, cap)` — `T` は注釈から
  (`__builtin_ptr_read` と同じ規約)。展開は leaf `j` を
  `prefix_j * cap + elem_index * stride_j` の `PtrRead` に
- `__builtin_soa_write(p, elem_index, cap, value)` — 同じ形の store
- `__builtin_soa_grow(old, old_cap, new_cap) -> ptr` — 新領域確保 →
  **列ごとに** copy → 旧領域解放。単純 realloc は列区切りの移動を扱えない
  (旧 `[x×4][y×4][m×4]` を memcpy すると新 `[x×8][y×8][m×8]` の y が
  ずれた位置に載る)。列 base は compile time、要素数は runtime

「採らない案」のリフレクション拒否と**矛盾しない**: あれは leaf 選択が
runtime に決まる形が `__builtin_ptr_read` の型注釈規約と衝突する話で、
こちらは leaf 選択が compile time のまま、runtime 引数は `cap` だけ。

`SoaVec<T>` の `get` / `set` / `push` / `pop` はこれらを呼ぶ普通の
stdlib method になる (parser / checker の特別扱いは sugar 解決だけ)。

> `__builtin_soa_*` を `BuiltinFunctionSymbols::new` に足したら
> `FULL_AST_CACHE_SCHEMA_VERSION` を上げること (`.toycache` の
> intern 順破壊対策 — CLAUDE.md 参照)。

#### 恩恵の本丸は Phase 1 slice 経由

`ps[i].x` の単列 shortcut は SoaVec では `__getitem__` 経由 (全 leaf 物質化)
になるので stack 配列のようには効かない。heap 版の帯域削減は
`ps.mass` → 列の `&[f64]` (Phase 1。checker が field 名 → leaf `j` を解いて
列 slice を構築) で初めて届く。**実装順 0 → 1 → 2 を変えない理由**。

#### 検証

SoaVec は別型なので「付け外して測る」は型注釈の差し替え
(`val ps: soa Vec<P>` ↔ `val ps: Vec<P>`) で、同じく 3 バックエンド
`assert_consistent` で pin する。確保回数は `Vec` と同一 (grow 1 回につき
1 確保) なので `allocations(N)` 契約はそのまま動く。バイト量契約
(`retains`) は tight pack 後に AoS/SoA で値が食い違う — layout が観測可能
であることの帰結で、別型にした理由の裏付け。

### 実装の見込み (stack 配列 — Phase 0 として landing 済み)

| 変更点 | 場所 | 状態 |
|---|---|---|
| `soa [` contextual 修飾子 (`soa` + 次が `[` のときだけ) | `frontend/src/parser/types.rs` (lexer 変更なし) | ✅ |
| `TypeDecl::Array(elems, size, soa)` 第 3 field (`is_equivalent` は無視) | `frontend/src/type_decl.rs` ほか arity 更新 | ✅ |
| `Binding::Array { storage: Interleaved \| Columns<Vec<slot>> }` | `compiler_lower/src/bindings.rs` | ✅ |
| `allocate_array_storage` — 列ごとに homogeneous slot | `compiler_lower/src/array_layout.rs` | ✅ |
| 添字式の分岐 (`i * leaf_count + j` ↔ 列 slot + 要素 index) | `compiler_lower/src/array_access.rs` ほか | ✅ |
| `ps[i].f` 単列 shortcut (AoS / SoA 両対応、読み・書き・ネスト chain) | `compiler_lower/src/field_access.rs` / `assign.rs` | ✅ |
| 範囲 slice の layout 継承・再 layout | `compiler_lower/src/let_lowering.rs` | ✅ |
| tree-walker | 配列表現が `Vec<Object>` なので**変更不要** (観測できる差が無い) | ✅ (最初から) |
| 列ごとの stride を leaf 実サイズに (tight pack) | `array_layout.rs` の stride 1 箇所 | Phase 0.5 |

tree-walker が変更不要なのは重要で、**layout を変えても答えが変わらない**
ことのオラクルがそのまま手に入る。`assert_consistent` は
「同じプログラムを `soa` 有り / 無しで走らせて一致」を pin する
(`compiler/tests/consistency/soa.rs` が 4-way で固定)。

**実装時に踏んだ既存バグ 2 件 (Phase 0 の前提として修正)**:

1. `val ps: [Point; 2] = [Point {...}, ...]` — 注釈付き struct 要素
   配列リテラルが checker で拒否されていた (要素型の `Identifier` と
   `Struct(name, [])` の綴り違いを unify していなかった。
   `collections.rs::visit_array_literal_impl`)
2. struct フィールド型の whitelist に f64 / f32 / narrow int が入って
   おらず `struct S { b: u8 }` / `struct P { x: f64 }` が宣言時に拒否
   されていた (`struct_literal.rs::visit_struct_decl_impl` — どちらも
   「それ以外の位置では全部動く型」で、scalar 幅の struct フィールドを
   書いた者がいなかった)

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

(Phase 2 の `__builtin_soa_*` はこの拒否の範囲外 — leaf 選択が
compile time に留まり、runtime 引数は `cap` だけなので上の規約と衝突しない。
heap 節を参照。)

### hot / cold フィールド分割の属性

属性構文そのものが言語に無い。`soa` があれば用途の大半を吸収する。

### entity index の newtype (`Idx<T>`)

**言語変更は要らない** — tuple struct (NEWTYPE) で今日書ける。
`interpreter/example/linked_list_arena.t` が既に arena + index の形。
必要になったら `core/std/slotmap.t` を置く話であって、本文書の範囲外。

## 構文の細部 (未決 1 を閉じた — 2026-08-30)

- **前置で確定**: `soa [T; N]` / `soa Vec<T>`。「置き方の修飾」に読め、
  宣言修飾子 (`unsafe fn` / `const fn` / `never_allocates fn`) と同じ位置感
- `soa` は contextual keyword (`test` と同じ)。型位置では `soa` の次が
  `[` か型名なら曖昧ゼロ。`struct soa` をユーザが宣言していたらそちらを
  優先して解除
- AST の持ち方は実装時に選ぶ: wrapper (`Soa(Box<TypeDecl>)`) は
  `soa Vec<T>` の sugar 解決と `soa soa` 拒否を 1 箇所で済ませるが
  `TypeDecl` の match 箇所が増え、`Array` への flag は差分が小さい。
  どちらでも Phase 0 の規模「小」は崩れない
- `soa` を認めるのは `[T; N]` と `Vec<T>` の位置のみ。他 (`soa dict` /
  ネスト `soa [soa [P; 4]; 8]`) は型エラーで拒否

## 段階

| Phase | 内容 | 規模 |
|---|---|---|
| **0** | `soa [T; N]` (scalar / struct / tuple 要素)。uniform 8 バイト列。**`ps[i].f` の単列 shortcut 込み** — **landing 済み (2026-08-30)**: 列方式 (事実 2 の注記) により IR / codegen / IR VM 無変更、`consistency/soa.rs` が soa 有無一致を 4-way で pin | ✅ 小 |
| **0.5** | 列ごとの tight pack — `allocate_array_storage` の stride を `ARRAY_LEAF_STRIDE` から leaf 実サイズへ (列方式なので codegen 分岐は不要、1 箇所の切替 + IR footprint の pin) | 小 |
| **1** | slice `&[T]` — SoA の窓。`ps.mass` → `&[f64]` | 中 |
| **2** | `soa Vec<T>` → `SoaVec<T>` sugar。単一領域の列分割 + builtin 3 個 | 中 |
| **3** | 配列要素としての enum + tag 列の分離 | 中 |

**Phase 0 は単体で価値があり、SIMD をやらなくても無駄にならない。**
事実 2 のおかげで変更が lowering の 2 ファイルに閉じ、
「`soa` の有無で答えが変わらない」を `assert_consistent` で pin すれば完了する。

## 決めていない論点

1. **要素まるごとの書き込み (`ps[i] = p`)** — leaf ごとに散らばった store に
   なる。SoA で「要素単位の更新が主」なワークロードは AoS より遅くなる。
   警告を出すかは未決だが、`--simd-report` (SIMD.md 戦略 D) と同じ
   「聞けば答える」tooling の側に置くのが妥当で、Phase 0 には入れない。

(未決 1「綴りと位置」は前置で決着、未決 2「heap コンテナへの伝達」は
`SoaVec<T>` 別型 + sugar (Phase 2) で決着 — いずれも 2026-08-30。本文中の
「構文の細部」「`soa Vec<T>`」節を参照。)
