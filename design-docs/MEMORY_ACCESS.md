# MEMORY-ACCESS — 1 バイトずつではなく「範囲」を primitive にする

> **状態: 提案 (2026-09-04)。M0 landing 済み (2026-09-05)、M1 以降は未実装。**
> 前提は [`POINTER.md`](POINTER.md) (L0〜L4 の層と `Ptr<T>` / `Span<T>`)、
> builtin の一覧は [`../docs/language.md`](../docs/language.md) の
> 「Pointer / memory builtins」、効果は [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md)。
> 本ファイルは POINTER の続きであって置き換えではない — P1〜P6 で作った
> 層はそのまま使い、**その層に流れていない**現状を直す。

## 要約

現在の最下層の primitive は

```rust
val b: u8 = __builtin_ptr_read(p, i)      # 幅は左辺の注釈から、offset はバイト
```

で、stdlib はこれを **213 箇所**で呼ぶ (`core/**.t`)。一方、範囲を一度に
動かす `mem_copy` / `mem_move` / `mem_set` は合計 **7 箇所**しか使われて
いない。つまり「メモリに触る」の既定形が**要素 1 個**になっている。

これは 3 つの独立した問題を持つ。

| # | 問題 | 一言 |
|---|---|---|
| A | **幅が演算に入っていない** | 左辺注釈という側路 (`pending_annotation` / `ptr_read_hints`) から来る。だから read は式ではなく文であり、注釈が無いとレーンごとに答えが割れる |
| B | **単位が演算に入っていない** | `ptr_read` はバイト、`__simd_load` は要素、`Ptr<T>` は要素。同じ番地に 3 つの単位が乗る |
| C | **粒度が要素固定** | 比較・探索・コピー・変換が全部手書きのスカラーループになり、速い版が要るところでは SIMD 版を**もう一度**手書きしている |

提案は「生 `ptr` を消す」ことではない (それは [`POINTER.md`](POINTER.md) の
「採らない選択肢」で決着済み)。**演算の単位を要素から範囲へ、幅と単位を
呼び出しの中へ移す**。

## 実測 (2026-09-04)

### 実測 1: 幅が演算に無いので、同じプログラムが 3 レーンで 3 通りの答えを出す

```rust
unsafe fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(16u64)
    __builtin_ptr_write(p, 0u64, 0x41u8)
    __builtin_ptr_write(p, 1u64, 0x42u8)
    val wide: u64 = __builtin_ptr_read(p, 0u64)   # u8 で書いて u64 で読む
    println(wide)
    0u64
}
```

```text
$ cargo run -q -p compiler -- confuse.t --all-backends
65                       # tree-walker: typed-slot map が最後に書いた u8 を返す
jit          25769820737 # 未初期化の上位バイト込み
aot          16961       # 0x4241 (little-endian、素直な 8 バイト load)
```

**診断は 1 つも出ない。** `--all-backends` が「不一致」と言うだけで、
言語としてはどれが正しいかを決めていない。型検査器は
`__builtin_ptr_read` の戻り型を注釈から取るだけで、**その番地に何が
書かれたかは型の問題になっていない**。

### 実測 2: 注釈が無いと compiled レーンが lowering できない

```rust
val s: u64 = (__builtin_ptr_read(p, 0u64) as u64) + 1u64
```

tree-walker は `4` を出す。JIT / AOT は
`compiler MVP could not infer source scalar type for 'as' cast` で落ちる。
read は**式ではなく `val NAME: TYPE = ...` という文の形**でしか存在せず、
それは `compiler_lower/src/let_lowering.rs` が PtrRead を構文的に特別扱い
していることで担保されている (`core/std/allocator.t:449` にも
「AOT MVP requires `val NAME: TYPE = __builtin_ptr_read(...)`」という
注記がある)。base64 の `val b0 / b1 / b2` のような一時束縛はこの制約の
産物で、アルゴリズム上の必要ではない。

側路の実装コストは `pending_annotation` 15 箇所 (4 ファイル) +
`ptr_read_hints` 19 箇所 + `read_annotated_scalar_bytes` の 3 段
フォールバック (typed slot → 注釈幅 → 8 バイト読み) である。
**同じ式の意味が「その番地に過去どう書いたか」で変わる**のはここ。

### 実測 3: 要素単位の代償 (debug build、比は桁の話として読む)

| 形 | 量 | 時間 |
|---|---|---|
| `__builtin_ptr_read/write` のループ (interpreter) | 20 万バイト書き + 20 万バイト読み | **~4.9 s** |
| 同じループ (AOT) | 同上 | **4 ms** |
| `__builtin_mem_copy` (interpreter) | **400 万**バイト | **~0.01 s** |

compiled レーンでは要素ループは素の load / store に落ちるので実害は薄い。
**tree-walker と IR VM では 1 バイトあたり ~12 µs** で、範囲演算 1 回
(~2.5 ns/byte) と 3〜4 桁違う。`--check` / `--test` / example の実行が
すべてこのレーンなので、「インタプリタが遅い」の相当部分はここにある。

### 実測 4: 範囲の primitive が育っていない

- `__builtin_mem_move` / `__builtin_mem_set` は **compiled レーンに存在しない**
  (`compiler MVP cannot lower builtin yet: MemMove` / `MemSet`)
- `__builtin_mem_set` は `docs/language.md` が `byte: u8` と書いているが、
  tree-walker は u64 しか受けない (`mem_set expects u64 value as second argument`)
- stdlib での使用は `mem_copy` 7 箇所のみ、`mem_move` / `mem_set` は 0 箇所

**使われないから実装されず、実装されていないから使われない。** その間に
`Vec::insert` / `Vec::remove` は要素を 1 個ずつずらすループを書いている。

### 実測 5: 同じアルゴリズムが何度も手書きされている

`core/std/string.t` は `__builtin_ptr_read` を 40 箇所で呼ぶ。部分文字列
探索の内側ループ (先頭バイトで当たりを付けて `j` で照合) は
`contains` / `split` / `find_from` / `rfind` / `replace` の **5 箇所**に
それぞれ手書きされている。`hex.t` / `base64.t` はさらに、同じ変換の
**SIMD 版とスカラー版を両方**持つ (`__simd_swizzle` / `__simd_shuffle` の
定数マスクを stdlib のソースに書いている)。

### 実測 6: `unsafe` が意味を失っている

stdlib の `fn` 860 本のうち **156 本 (18%) が `unsafe fn`**。P6 の狙いは
「生 builtin を叩ける場所を stdlib に集約する」ことだったが、集約先が
stdlib 全体になっている。`Vec<T>::get` が `unsafe` なのは、境界検査が
あるかどうかとは無関係に、中で `ptr_read` を呼ぶからでしかない。

## 設計

層は [`POINTER.md`](POINTER.md) のまま (L0 生番地 / L1 `Ptr<T>` /
L2 `Span<T>` / L3 所有 / L4 `unsafe`)。変えるのは**各層で何が primitive か**。

```text
今                              提案
L2  Span<T>  = 要素 get/set     Span<T>  = 範囲演算 (copy/fill/eq/find/chunks)
L1  Ptr<T>   = 要素 get/set     Ptr<T>   = 要素 get/set (幅は T から) ← 変更なし
L0  ptr_read(p, byte) -> 文脈型  ptr_read::<T>(p, byte) -> T (幅は呼び出しに)
```

### A. 幅を呼び出しに入れる — `__builtin_ptr_read::<T>(p, off)`

turbofish は既にある: `__builtin_sizeof::<T>()` が
`BuiltinFunction::SizeOfType(TypeDecl)` として parser
(`frontend/src/parser/expr/primary.rs:497`) から型を運んでいる。同じ形で
`PtrReadTyped(TypeDecl)` を足す。**IR 側は既に幅を持っている** —
`compiler_ir` の `InstKind::PtrRead { ptr, offset, elem_ty }` は
lowering 時に注釈から埋めているだけなので、埋める元を呼び出しに変える。

- 得るもの
  - read が**式になる**。`val` 一時束縛の連なりが消える
  - 実測 1 が**書けなくなる** — `ptr_read::<u64>` と `ptr_write(_, _, u8)`
    の食い違いは、少なくとも「どちらの幅か」が読めば分かる形になる
    (provenance の検査はしない — 後述)
  - 実測 2 の lowering 特例、`pending_annotation`、`ptr_read_hints`、
    `read_annotated_scalar_bytes` のフォールバック連鎖が**消える**
    (足すより消す量のほうが多い)
- 移行: 213 箇所は注釈がすぐ隣にあるので機械的に書き換えられる。旧形は
  1 リリース deprecated (警告) にして落とす
- 注意: builtin 名を足すので `FULL_AST_CACHE_SCHEMA_VERSION` を上げる
  (CLAUDE.md の注記どおり。`SizeOfType` の payload を module 統合で
  remap した箇所 — POINTER P3 実装メモ — と同じ手当てが要る)

### B. 単位を層で固定する

- **L0 (`ptr` + 生 builtin) はバイト**。これは `void*` の意味なので変えない
- **L1 / L2 (`Ptr<T>` / `Span<T>`) は要素**。ここは既にそうなっている
- 例外は `__simd_load(p, i)` の**要素 index**。仕様を変えると SIMD.md の
  既存コードが壊れるので、**変えずに到達経路を変える**: stdlib は
  `Span<T>::chunk(i) -> u8x16` 経由で読み、生の `__simd_load` は
  `span.t` の中だけにする。単位の混在は「同じ関数の中に 2 つの単位が
  出てくる」ことが害なので、層で隔離すれば消える

### C. 範囲を primitive にする (本題)

`Span<T>` に**範囲丸ごとの演算**を置き、実装をバックエンドごとに 1 つ
持つ。stdlib のバイトループはこの語彙で書き直せる。

| 形 | 提案 API | 今どう書かれているか |
|---|---|---|
| コピー | `dst.copy_from(src)` | `mem_copy` 7 箇所 / それ以外は要素ループ (`Vec::insert` / `remove`) |
| 埋める | `s.fill(v)` | 手書きループ (`mem_set` は compiled レーンに無い) |
| 等値 | `a.eq(b)` | `String::eq` の手書きループ |
| 順序 | `a.cmp(b) -> Ordering` | 無い (`Ord` は要素単位) |
| 1 バイト探索 | `s.find(v) -> Option<u64>` | `split` / `contains` の先頭バイト走査 |
| 列探索 | `s.find_seq(needle) -> Option<u64>` | **5 箇所**に手書き (実測 5) |
| 前方 / 後方一致 | `s.starts_with(t)` / `ends_with(t)` | 手書きループ |
| 述語走査 | `s.position(f)` / `s.count(f)` | `trim` / `split_whitespace` の手書きループ |
| ブロック反復 | `for c in s.chunks::<16>()` | `hex` / `base64` が `__simd_load` + 手書きの端数ループ |
| 幅つきスカラー | `s.read_u32_le(i)` / `write_u32_be(i, v)` | `(b0 as u64) * 65536u64 + ...` (base64 / sha256) |

**要点は「便利関数を足す」ことではない。**

1. **実装が 1 箇所になる。** `find_seq` の SIMD 版は runtime
   (`compiler/runtime/toylang_rt/`) に 1 つあればよく、stdlib のソースから
   `__simd_shuffle` の定数マスクが消える。tree-walker も**同じ意味論を
   1 回の呼び出しで**得る (実測 3 の 3〜4 桁がここで効く)
2. **端数ループが消える。** 今の SIMD 化は「16 バイトずつ + 端数を
   スカラーで」を stdlib の各所に書いており、**同じアルゴリズムの 2 実装**が
   常に食い違いうる。`chunks::<16>()` が端数を持つ形にすれば書き手は 1 回
3. **境界が型に乗る。** `Span<T>` は長さを持つので、これらは全部
   **safe fn** にできる (D)

実装は `mem_copy` の既存経路に倣う: IR に命令を 1 つ、`toylang_rt` に
`toy_span_*` を 1 つ、tree-walker に 1 arm。**まず `mem_move` / `mem_set` の
compiled レーン欠落と `mem_set` の署名不一致 (実測 4) を塞ぐのが最初の 1 歩**
で、これは提案の残りと独立に価値がある。

### D. `unsafe` を意味のある印に戻す

A + B + C の後、生 builtin を直接呼ぶのは `core/std/ptr.t` /
`span.t` / `allocator.t` と extern 境界だけになる。`Span<T>` の範囲演算は
境界検査つきなので `unsafe` が要らず、`Vec` / `String` / `Dict` の
method は `unsafe` が外れる (156 → 20 前後の見込み)。
`--effects` の `raw_read` / `raw_write` が**実際に見るべき 20 本**を
指すようになる。

あわせて B の副産物として `Vec<T>` の `elem_size` フィールド
(`__builtin_sizeof::<T>()` が入った今は不要で、`push` ごとに
`if self.elem_size == 0u64` を踏んでいる) が畳める。これは POINTER P1 が
「副産物として畳める」と書いたまま残っている。

## 採らない選択肢

- **生 `ptr` の廃止** — allocator と FFI が番地を必要とする。
  [`POINTER.md`](POINTER.md) で決着済み (C# 側の解: 消さずに隔離)
- **コンパイラでバイトループを `memcpy` / `memcmp` に畳む
  (loop idiom recognition)** — LLVM がやっていることだが、(1) alias と
  allocator の事実を証明する道具が今のパイプラインに無い、(2) **効いても
  compiled レーンだけ**で、一番遅い tree-walker は救われない、
  (3) 「畳めたかどうか」がソースから読めないので性能が予測できない。
  範囲演算を書き手の語彙にするほうが安い
- **注釈推論を賢くする** (式位置でも周囲から幅を取る) — 側路を太らせる
  だけで、実測 1 の不一致は残る
- **provenance / 型付きメモリの検査** (書いた型と読む型の一致を静的に
  強制する) — 番地を跨いだ流れ解析が要り、`Span` の escape 検査
  (`E0026`) の比ではない規模になる。A で「幅が読めば分かる」ところまでを
  取り、型混同そのものは `unsafe` の責任として残す
- **`Span<T>` を言語組み込みの slice `&[T]` にする** — todo の
  NEW-TYPE-SYSTEM にある案だが、C の範囲演算は組み込み構文でなくても
  書ける。組み込み化は独立に判断してよい

## フェーズ

コスト順。各段が単体で価値を持つ。

| # | やること | 規模 | 効果 |
|---|---|---|---|
| M0 | `mem_move` / `mem_set` を compiled レーンで lowering、`mem_set` の署名を doc に合わせる (実測 4) ✅ (2026-09-05) | 小 | 4 レーン一致。以降の土台 |
| M1 | `__builtin_ptr_read::<T>(p, off)` (A) と旧形の deprecation | 中 | 実測 1・2 の解消。側路 3 種の削除 |
| M2 | stdlib 213 箇所を `::<T>` 形へ機械移行 + `Vec::elem_size` 撤去 | 中 (stdlib) | 単位と幅が層で固定される |
| M3 | `Span<T>` の範囲演算 (C の表) を `copy_from` / `fill` / `eq` / `find` / `find_seq` から | 中 | 実測 3・5 の解消。string.t の 5 重複が 1 に |
| M4 | `chunks::<N>()` と `read_uNN_le/be` | 中 | hex / base64 / sha256 の手書き SIMD と桁合わせが runtime に移る |
| M5 | `Vec` / `String` / `Dict` / `Box` の `data: ptr` → `Ptr<T>` / `Span<T>`、`unsafe fn` の縮小 (D) | 中 (stdlib) | `unsafe` が 20 本の印に戻る |

## 実装メモ

**M0 (2026-09-05)。** `InstKind::MemMove` / `MemSet` を足し、lowering /
AOT codegen (libc `memmove` / `memset`) / IR VM host に配線した。
`mem_set` の fill value は `u8` になり、**型検査が引数を見るようになった** —
`visit_builtin_call` は署名表から戻り型を返すだけで**引数を訪問していなかった**
ので、`arg_types` は飾りだった (`__builtin_mem_set(p, undefined_name, 8u64)`
が型検査を通り実行時に落ちていた)。3 つの `mem_*` だけは
`check_memory_builtin_args` が引数を訪問して型を照合する
(`coerce_number_expr` 経由なので、サフィックス無しリテラルと char リテラルは
引数位置の型を取る)。副産物 2 つ:

- `compiler_lower` の `lower_builtin_call` から**catch-all を外した**。
  `MemMove` / `MemSet` が最後の未 lowering builtin だったので match は
  exhaustive になり、新しい builtin は実行時の
  「compiler MVP cannot lower builtin yet」ではなく**コンパイルエラー**になる
- `interpreter/example/jit_heap.t` が `AOT_UNSUPPORTED` から外れた
  (`example_consistency.rs` の両方向検査が要求してきた)

## 検証

- 各範囲演算に `compiler/tests/consistency/` の `assert_consistent` を
  1 本ずつ (意味論を 4 レーンに持つため。CLAUDE.md の横断変更の規則)
- 実測 1 の `confuse.t` を**回帰テストとして残す** — M1 後は
  「幅が呼び出しに書いてある 2 つのプログラム」になり、どちらも
  4 レーン一致するのが期待値
- 実測 3 のベンチ (20 万バイトのループ vs 範囲演算) を M3 の前後で取る。
  ここが 3 桁縮まらないなら M3 の設計が間違っている
- `grep -c "__builtin_ptr_read" core/**.t` と `grep -c "unsafe fn"` を
  各フェーズの後に記録する (213 → ~0、156 → ~20 が目標値)
