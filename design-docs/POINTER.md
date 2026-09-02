# POINTER — `ptr` を型のある窓に変える

> **状態: P1〜P6 landing 済み (2026-08-30)。フェーズは全部入った。**
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
| P5 | `Ptr<T>` を non-null 不変にし、不在は `Option<Ptr<T>>` ✅ (2026-08-30) | 中 | 摩擦 5。`has_next: bool` 方式が消える |
| P6 | `unsafe fn` の宣言と強制 (effect mask 1 行) ✅ (2026-08-30) | 小〜中 | 生 builtin を直接呼べる場所を stdlib に集約。`--effects` が土台 |

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

**P5 実装メモ (2026-08-30)**: 不変は**構成による規約**で、compiler 強制は
入れなかった — field visibility は「記録はするが未強制」
(`frontend/src/parser/stmt.rs` の注記) なので、強制には別の機能が要る。
`alloc(0)` は 1 バイトに丸める (全バックエンドが `heap_alloc(0)` に null を
返すので、丸めないと non-null 不変の穴になる)。不在は `Option<Ptr<T>>`
— **全レーンで動く** (再帰 struct `Node { next: Option<Ptr<Node>> }`、
enum payload の struct、match、enum-typed field の引数位置読み)。
ただし引数位置の enum field 読み (`has_next(node.next)`) は
compiled レーンが `pending_enum_value` を消費していなかったので、
`lower_call_arg_items` に 1 arm 足した (`load_enum_locals` で展開、
enum *binding* 引数と同じ形)。niche 最適化は不可 — 16 バイトのまま受け入れる
(「採らない選択肢」どおり)。例:
`interpreter/example/linked_list_typed_ptr.t`。

**P6 実装メモ (2026-08-30)**: マスクは予定どおり
[`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) の 1 行
(`RAW_READ | RAW_WRITE`) だが、**歩き方**が既存の 3 検査と違う —
`unsafe fn` は「この body 自身が触るか」なので**呼び先を辿らない**
(`EffectTable::new_direct_only`)。辿ると `Vec::push` を呼ぶ `main` まで
`unsafe` になり、stdlib に集約する意味が消える。検査本体は
[`frontend/src/type_checker/unsafe_check.rs`](../frontend/src/type_checker/unsafe_check.rs)
(81 行)、診断は `[E0024]` (`--explain E0024` に 3 通りの直し方)。

同時に **番地を作る・比べるだけの builtin を effect 無しに落とした**
(`ptr_offset` / `ptr_eq` / `ptr_is_null` / `null_ptr` / `str_to_ptr`) —
メモリの*内容*に触らないので、Rust の `as_ptr` / `offset_from` が safe
なのと同じ。これがないと「null か訊く」だけで `unsafe fn` が要る。
副作用として `const fn` から番地計算が呼べるようになった。

`unsafe` は **contextual** な修飾子 (`fn` の直前だけ、`never_allocates` /
`const` とは順不同)。trait の**シグネチャ**には body が無いので検査対象外
だが、**default body** は omit した impl が継承するので
`TraitMethodSignature::is_unsafe` として一緒に運ぶ。`extern fn` は
歩ける body が無いので宣言としてのみ受理する。

stdlib 側は `Vec` / `String` / `Dict` / `Box` / `Ptr` / `Span` /
`allocator` の生 builtin を叩く method が `unsafe fn` を持ち、
**呼び出し側は safe のまま** — これが P3〜P5 で作った窓の対価。
テストは [`interpreter/tests/unsafe_fn_tests.rs`](../interpreter/tests/unsafe_fn_tests.rs) (11 本)。

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
`docs/language.md` に「escape は未検査」と明記して進んだ。

**2026-09-02 に検査を入れた (`[E0026]`)。** 着手条件は満たされていた —
`fn f() -> Option<Span<u8>> { var v = Vec::new(); ...; v.as_span() }` が
**通ってしまい、解放済みメモリを読んで正しい答えを返す** (never-reuse
ヒープのおかげで「たまたま」当たる) のを実測した。採ったのは選択肢 2
ではなく **REGIONS の規則を所有者違いで再利用する**形: 「終わりが見える
ものから派生した値は、それより長生きする場所へ到達してはならない」を
allocator と**ローカルのバッファ**の 2 つの owner に対して適用する
(`region_check.rs` の `RegionKind`)。マーカ構文が要らないのは、
**パラメータを owner にしない**という REGIONS と同じ除外規則で
`Vec::as_span(&self)` が自動的に合法になるため。
未検査で残るのは closure が捕捉した窓と、reallocate を跨いだ窓
(どちらも lifetime の形をしていない)。

**`Ptr<T>` を生 `ptr` から作れない (CONV-SPAN)。** 公開関数は
`alloc` / `get` / `set` / `offset` / `as_raw` / `__getitem__` /
`__setitem__` の 7 つで、**構築の口が `Ptr::alloc` しか無い**。
一方 `String::as_ptr()` / `Vec::as_ptr()` は生 `ptr` を返すので、

```
String ──as_ptr()──> ptr ──✗──> Ptr<u8> ──from_parts()──> Span<u8>
```

が繋がらず、**既にあるバッファに対して `Span<T>` を作れない**。
`Span<T>` を「`&[T]` のライブラリ側の答え」と位置づけている以上、
これは穴 (2026-08-31 に [`NETWORK_IO.md`](NETWORK_IO.md) の設計中に発見)。

**`Ptr::try_from_raw(p) -> Option<Self>` は 2026-08-31 に landing** —
非 null 不変 (P5) を壊さないので既定はこちら。`Option<Ptr<T>>` という
戻り型自体が型検査 / lowering / tree-walker の 3 層で通らなかったため、
先にそれを直している (todo 完了済みの GENERIC-IN-ENUM-PAYLOAD /
SELF-IN-TYPE-ARG)。

残り: `Span::from_raw_parts(p: ptr, len: u64)`、`String::as_span()` /
`Vec<T>::as_span()`。状態は `todo.md` の NET (CONV-SPAN)。
