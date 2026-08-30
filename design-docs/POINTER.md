# POINTER — `ptr` を型のある窓に変える

> **状態: P1〜P3 landing 済み (2026-08-30)。P4〜P6 は未実装。**
> 実測は 2026-08-30。
> 現状の `ptr` は [`frontend/src/type_decl.rs`](../frontend/src/type_decl.rs)
> の `TypeDecl::Ptr` (型引数を持たない nullary variant)。builtin の一覧は
> [`../docs/language.md`](../docs/language.md) の「Heap and pointer builtins」、
> 効果の分類は [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md)、
> リージョン脱出は [`REGIONS.md`](REGIONS.md)。
> 関連する未実装項目は [`todo.md`](todo.md) の **slice 型 `&[T]`**。

## なぜ

`ptr` は C の `void*` そのもので、指す先について**何も言わない**。
そのため raw pointer を使うコードは、型検査器が持っていない情報を
プログラマが毎回手で補い続けることになる。

| # | 摩擦 | 現れ方 |
|---|---|---|
| 1 | 指す型が型に無い | 読みは注釈が唯一の shape 情報源 (`val v: T = __builtin_ptr_read(p, off)`)。省けない |
| 2 | 型混同が検査されない | `Vec<i64>` の `data` を `f64` として読んでもコンパイルは通る。`ptr` は全部同じ型 |
| 3 | オフセットの単位が混在 | `Vec` は `i * self.elem_size` を手書き、`__simd_load(p, i)` の `i` は**要素 index**。同じ `p` に 2 つの単位が乗る |
| 4 | 長さが無い | 境界検査は各コンテナが自前で持つ。`ptr` を渡した時点で長さは消える |
| 5 | null が値 0 で型に出ない | `linked_list_ptr.t` の `next: ptr` + `has_next: bool` の形が伝染する |
| 6 | 所有・解放が型に出ない | move check (E0014) は `Drop` を持つ**型**を追うので生 `ptr` は対象外。`Box` が `Drop` を被せて初めて所有が出る |
| 7 | サイズが値からしか取れない | `__builtin_sizeof(value)` は**値**を要る。`Vec` が `elem_size` を「最初に push された要素」から遅延取得しているのはこのため |

## C# と Java から何を取るか

方向は正反対で、**取るべき層が違う**。

**C# — 生ポインタを消さず隔離した**

- `unsafe` ブロック + `fixed` の中でだけ `T*` が書ける。`T*` は**型付き**で
  `p[i]` / `p->f` / `sizeof(T)` が要素単位で効く (摩擦 1〜3)
- 安全側は `Span<T>` / `ReadOnlySpan<T>` = **(ポインタ, 長さ) の fat pointer**。
  境界検査つき、`ref struct` なのでヒープにもフィールドにも置けず escape しない (摩擦 4)
- 素の番地は `nint` / `IntPtr` という別の型に隔離 — **これが今の `ptr` の正しい居場所**
- null は `T?` + flow 解析で型に出す (摩擦 5)

**Java — 言語からポインタを消し、FFM API (Java 22) で戻した**

- `MemorySegment` = 番地 + **サイズ** (空間境界)。範囲外は例外
- `Arena` = **時間境界**。閉じると、そこから取った全 segment が一斉に無効化される
- `MemoryLayout` + `VarHandle` で**型付きアクセサ**。バイトオフセットを手で書かない
- 生アクセスは *restricted method* で、`--enable-native-access` が無ければ警告
  (「安全でない操作は宣言せよ」)

**toylang は既に Java 22 側に近い。** `Arena` の temporal bounds は
REGIONS の `E0022` がやっていることそのもので、restricted method の宣言は
effect lattice に `RawRead` / `RawWrite` として**既に計算されている**
(下の実測 1 のプロトタイプに対して):

```text
$ cargo run -q -p interpreter -- --effects typed_ptr.t
main             alloc, raw_read, raw_write
TypedPtr::get    raw_read
TypedPtr::set    raw_write
```

「どの関数が unsafe か」には既に答えられる。足りないのは**宣言と強制**だけ。

## 層

```text
L4  unsafe 境界      unsafe fn + effect mask 1 行     ← C# unsafe / Java restricted method
L3  所有             Box<T> (済) + Option<Ptr<T>>     ← C# T? / Java Optional
L2  窓               Span<T> = (Ptr<T>, len) 境界検査 ← C# Span<T> / Java MemorySegment
L1  型付きポインタ   Ptr<T> 要素 index、注釈不要      ← C# T* / Java VarHandle
L0  生の番地         ptr (現状のまま、stdlib 専用)    ← C# nint / IntPtr
```

**L1 と L2 は言語機能ではなく stdlib の struct にできる。**
`Box<T>` が「パーサ・型検査・バックエンドで一切特別扱いされていない」のと
同じ手口がそのまま効く — `T` はフィールドに現れず `ptr` の背後にしか居ないので、
再帰型検査も move check も既存のまま通る。

## 実測 (2026-08-30)

### 実測 1: 型付きポインタは今日のコンパイラで動く

```rust
struct TypedPtr<T> { addr: ptr, stride: u64 }

impl<T> TypedPtr<T> {
    fn get(&self, i: u64) -> T {
        val v: T = __builtin_ptr_read(self.addr, i * self.stride)
        v
    }
    fn set(&mut self, i: u64, value: T) {
        __builtin_ptr_write(self.addr, i * self.stride, value)
    }
}
```

`--all-backends` で `all 3 backends agree`。**コンパイラ変更ゼロで摩擦 1〜3 が消える。**

### 実測 2: `sizeof(T)` が無いのが L1 の前提条件

上の `alloc` は `proto: T` という**代表値を要求する**羽目になった
(`__builtin_sizeof` が値しか取らないため) ので、`Ptr<T>::alloc(n)` が書けない。

実装は軽い: AOT の `SizeOf` lowering は既に**値の IR 型**から
monomorph subst 下でサイズを出している
([`compiler_lower/src/expr.rs`](../compiler_lower/src/expr.rs) の
`lower_builtin_reflection`)。型引数を解決する経路を足すだけで、
サイズ計算そのものは再利用できる。副産物として `Vec` / `Dict` /
`String` の `elem_size` フィールド (遅延取得) が畳める。

### 実測 3: `__getitem__` に穴が 2 つある

C# の indexer 相当 (`p[i]`) を載せようとして両方踏んだ。
**どちらも 2026-08-30 に解消** (P2、実装メモは git log の
「POINTER P2」コミット):

- `fn __getitem__(&self, i: u64)` は
  `[E0010] __getitem__ method must have at least 2 parameters` で落ちる。
  `self: Self` 形しか通らない —
  [`frontend/src/type_checker/struct_literal.rs`](../frontend/src/type_checker/struct_literal.rs)
  の `check_struct_getitem_access` の arity 検査が `&self` 短縮形を数えていない
- generic struct では**戻り型 `T` が置換されない**。`p[0u64]` が
  `Generic(T)` のまま返り `E0001`。`p.get(0u64)` は通るので、
  置換が getitem 経路だけ抜けている

解消に当たって 3 つ目の穴も出た: **compiled レーンは struct `p[i]` /
`p[i] = v` を lowering できなかった** (array binding しか受けていない)。
`lower_slice_access` / `lower_slice_assign` が struct / enum binding を
`__getitem__` / `__setitem__` の method 呼び出しに委譲することで、
tree-walker が元から持っていた dispatch に揃った。

## フェーズ

コスト順で、各段階が単体で価値を持つように並べた。

| # | やること | 規模 | 効果 |
|---|---|---|---|
| P1 | `__builtin_sizeof::<T>()` (型引数形) ✅ (2026-08-30) | 小 | 実測 2 の解消。`elem_size` 遅延取得も畳める |
| P2 | `__getitem__` の `&self` 受理 + generic 戻り型の置換 ✅ (2026-08-30) | 小 | `p[i]` が書ける。`Vec` / `Dict` にも効く |
| P3 | `core/std/ptr.t` に `Ptr<T>` (`alloc` / `get` / `set` / `offset` / `as_raw` / `__getitem__` / `__setitem__`) ✅ (2026-08-30) | 小 (stdlib のみ) | 摩擦 1〜3、6 の入口。**コンパイラ無変更** (module 統合の remap 1 箇所を除く、下記) |
| P4 | `Span<T> = { p: Ptr<T>, len: u64 }` + 境界検査 ✅ (2026-08-30) | 中 (stdlib) | 摩擦 4。todo の **slice 型 `&[T]`** をライブラリ側で回収でき、`__simd_load` の受け口にもなる |
| P5 | `Ptr<T>` を non-null 不変にし、不在は `Option<Ptr<T>>` | 中 | 摩擦 5。`has_next: bool` 方式が消える |
| P6 | `unsafe fn` の宣言と強制 (effect mask 1 行) | 小〜中 | 生 builtin を直接呼べる場所を stdlib に集約。`--effects` が土台 |

P3 が入れば **`Box` / `Vec` / `String` / `Dict` の `data: ptr` を
`Ptr<T>` に置き換えられる**。stdlib 全体でバイトオフセット計算が 1 か所に集まる。

**P3 実装メモ (2026-08-30)**: `core/std/ptr.t` は「コンパイラ無変更」の
予定だったが、1 箇所だけ触った — **module 統合の remap が
`BuiltinFunction::SizeOfType(TypeDecl)` の payload を素通りさせていた**。
`__builtin_sizeof::<T>()` を stdlib モジュールの body に書くと、turbofish
の `T` だけ module-interner の symbol に残り、monomorph subst
(remap 済みの `generic_params` が鍵) が見つけられない。interpreter の
`module_integration.rs::map_expr` が BuiltinCall の payload を
`remap_type_decl` する 1 arm で解消 (compiler の AOT / IR VM は同じ
統合 pass を使うので 1 箇所で全レーン直る)。

**P4 実装メモ (2026-08-30)**: `Span<T>` は `Ptr<T>` を field に持つ
(generic struct を field 型に、外側の `T` を型引数として —
`substitute_field_type` がそのままで通る)。要素アクセスは
`self.data.addr` (2 段 field chain) に `__builtin_sizeof::<T>()` を掛ける
だけで、**コンパイラ無変更**。関数境界は struct by-value の既存
flatten に乗る (`fn sum(s: Span<u64>) -> u64` が 3 バックエンド)。

P6 のマスクは [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) の表に 1 行足すだけ:

```text
unsafe fn を要求   RAW_READ | RAW_WRITE
```

`Ptr<T>` / `Span<T>` を経由する限り呼び出し側は safe のまま、
生 builtin を直接叩く関数だけが宣言を要求される。

## 採らない選択肢

- **Java 式の「`ptr` 全廃」** — GC が前提。toylang は allocator を明示する
  言語なので、L0 は残して `unsafe` に隔離する C# 側の解を採る
- **C# の `fixed` / `stackalloc`** — `fixed` は GC の移動を止めるための構文。
  移動する GC が無いので意味がない
- **`Option<Ptr<T>>` の niche 最適化** — enum layout が「u64 タグ + 全 variant
  payload 連結」で固定 (PTR-READ-ENUM) なので、番地 0 を `None` に畳む余地が無い。
  16 バイトになるのを受け入れる

## 未解決の論点

**`Span<T>` の escape をどう止めるか (C# の `ref struct` 相当)。**
ここだけ素直に載らない。現状の escape rule (REF-Stage-2 (e)) は
**`&T` 型にしか効かず**、`ptr` を含む struct は return もフィールド格納もできる。
REGIONS (`E0022`) は arena 由来の値を捕まえるが、**default allocator 由来の
`Span` は素通り**する。選択肢は 2 つ:

1. P4 の時点では検査しない (「`Span` を返すのは危険だが検査されない」と明記して進む)
2. struct 宣言に escape 禁止マーカを足す (`ref struct` 相当)。
   `TypeDecl::contains_ref()` と同じ形の再帰判定で拾えるが、
   「どこまでを escape とみなすか」は REF-Stage-2 の規則を再利用できる

**P4 で 1 を採用した (2026-08-30)** — `core/std/span.t` のヘッダと
`docs/language.md` に「escape は未検査」と明記して進む。
選択肢 2 は、実際に dangling span が実プログラムで問題になったときの
着手候補として残る。
