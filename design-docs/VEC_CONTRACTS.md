# VEC CONTRACTS — `Vec<T>` に Design by Contract を適用する案

> **状態: 一部 landing (2026-09-04)。`requires` の 3 行 (§4 の #1 / #2 /
> #3) を `core/std/collections/vec.t` に入れた。`ensures` 系 (#4〜#7) と
> `never_allocates` 系 (#8〜#11) は未着手。**
> 以下の A〜F が候補の分類、§4 の表が選択シート。選んだ行だけを
> `core/std/collections/vec.t` に入れる。

> 対象: `core/std/collections/vec.t` (`Vec<T>` / `VecIter<T>` /
> `impl Vec<u8>` / アダプタ 4 種)
> 契約の書き方: [`docs/design_by_contract.md`](../docs/design_by_contract.md)
> 文法の正本: [`docs/language.md`](../docs/language.md) の Design by Contract 章
> 実測: 2026-09-04 (本文の「確認済み」はすべてこの日に手元で走らせた結果)

## 0. 先に結論

- **状態遷移の `ensures` (B) が本命。** `push` / `pop` / `insert` /
  `remove` / `clear` / `resize` の「長さがどう変わるか」は今コメントにしか
  書かれておらず、契約にすると `--api` に出る仕様になり、実装が破った日に
  止まる。コストは AOT で **1 呼び出し 0.8ns** (§1)。`--release` で消える
- **境界検査の `requires` 置き換え (A) は「置き換え」ではなく「併記」を推奨。**
  `requires` は `--release` で消えるが、`Vec::get` の範囲外は
  メモリ安全の問題であって、組み込み配列の `arr[i]` guard は `--release`
  でも残る設計 ([`GUARD_ELISION.md`](GUARD_ELISION.md))。`Vec` だけを
  release で unchecked にする理由は無い。`requires` を足す価値は
  **違反時に添字の実値が出る**ことと、**シグネチャに書かれる**ことで、
  panic 文を消すことではない
- **要素値についての契約は書けない** (§2 B-不採用)。`ensures
  self.get(i) == value` は generic `T` に `eq` を要求するので、`eq` の無い
  struct を要素に持つ全 `Vec` の `push` が `[E0010]` になる
  (確認済み)
- **`--check` は `Vec` の method を掃かない** (レシーバに `ptr` フィールド
  があると黙って対象外、確認済み)。`with_capacity(n)` のようなレシーバ無しの
  構築子だけは掃かれる。したがって書いた契約の検証はテストスイート全体と
  `--all-backends` が担う — `Vec` は stdlib の大半 (`String` / `Dict` /
  `Span` / net) の下に居るので、間違った `ensures` は数百のテストで即座に
  見つかる
- 調査で **言語側の穴を 3 つ**見つけた (§5)。うち 2 つ (method への
  `never_allocates` + `unsafe` の重ね書き、compound `result` のフィールド
  参照) は候補 C / B の一部を塞いでいる

## 1. 確認済みの事実

設計の前提になる挙動を、`Vec` と同じ形 (generic + `ptr` フィールド +
`&mut self` + `unsafe fn`) の struct で確かめた。

| 事項 | 結果 |
|---|---|
| `unsafe fn` に `requires` / `ensures` / `old(self.len)` | 3 バックエンド一致 (`--all-backends`) |
| generic `impl<T>` の method に契約、`T = String` (compound) でも | 一致 |
| `ensures` から `self.get(i)` を呼ぶ (`__builtin_ptr_read` に到達) | **警告なし** — 純粋性検査 (`E0018`) は alloc / free / ptr **write** / print / 追えない呼び出しだけを見る |
| `ensures self.is_sorted()` (`T: Ord` の `lt` を辿る) | **`E0018` 警告** — `lt` の impl のひとつ (`String`) が `__extern_str_cmp` に届くため。警告であって実行は通る |
| `ensures self.get(i) == value` (generic `T` の `==`) | `T` に `eq` が無いと **呼び出し側が `[E0010]`** — `contains` と同じ規則が `push` に掛かる |
| `ensures result.cap == n` (`-> Self` の構築子) | **書ける** (2026-09-21、§5-2)。`with_capacity` が実例 |
| `never_allocates fn size(&self)` (method、単独) | parse OK |
| `never_allocates unsafe fn get(...)` (method、重ね書き) | **書ける** (§5-1、2026-09-21) |
| `ensures allocations(0u64)` と `realloc` | `heap_realloc` は `alloc_count` を増やさない (`realloc_count` 側)。`push` が realloc しても `allocations(0)` は成立 |
| 違反時の文言 | `` Contract violation: `requires` clause #1 of function `get` evaluated to false (with index = 5) `` — **添字の実値が出る**。method 名は `Vec::` 無しの裸名 (backtrace には `Vec::get` が出る) |
| `--api core/std/collections/vec.t` | 契約節がそのまま出る (`requires` / `ensures` 行) |
| `--check` | レシーバに `ptr` フィールドがある method は**報告なしで対象外**。レシーバ無しの `with_capacity(n)` は掃かれる。空のプログラムでも stdlib の `isqrt_u64` が 1 件掃かれている |
| guard elision | 効かない — 消せるのはパラメータ名 vs リテラルの形だけで、`self.len` はフィールド |

**コスト** (AOT、`push` 相当の method に `requires self.len < self.cap` +
`ensures self.len == old(self.len) + 1u64`、5000 万回):

| | 実時間 |
|---|---|
| 契約なし | 0.07s |
| 契約あり (checked) | 0.11s |
| 契約あり `--release` | 0.07s |

1 呼び出しあたり **約 0.8ns**。`INTERPRETER_CONTRACTS=off` は AOT バイナリ
には届かない (コンパイル時の `--release` だけが消す)。

## 2. 適用範囲の分類

### A. 事前条件で境界を言う (`requires`)

今 `if ... { panic(...) }` で守っている 7 箇所。**panic を消すか残すか**が
選択肢で、契約自体は同じ:

| method | 節 | 今の guard |
|---|---|---|
| `get(index)` | `requires index < self.len` | `panic("Vec::get index out of bounds")` |
| `set(index, value)` | `requires index < self.len` | 同上 (`set`) |
| `pop()` | `requires self.len > 0u64` | `panic("Vec::pop on an empty Vec")` |
| `insert(index, value)` | `requires index <= self.len` | `panic("Vec::insert index out of bounds")` |
| `remove(index)` | `requires index < self.len` | 同上 (`remove`) |
| `swap_remove(index)` | `requires index < self.len` | 同上 (`swap_remove`) |
| `set_size(n)` | `requires n <= self.cap` | `panic("Vec::set_size beyond capacity")` |
| `push_char(c)` | `requires c < 0x110000u32` / `requires !(c >= 0xD800u32 && c <= 0xDFFFu32)` | `assert(...)` 2 本 |
| `grow_to(new_cap)` (内部) | `requires new_cap >= self.len` | 無し (呼び出し側が常に満たす) |

3 つの持ち方:

| | A1: `requires` のみ | A2: `requires` + panic 併記 (推奨) | A3: 現状維持 |
|---|---|---|---|
| checked ビルドの挙動 | 契約違反 (値つき) | 契約違反 (値つき、panic は死に枝) | panic (値なし) |
| `--release` の挙動 | **unchecked (範囲外 read/write)** | panic | panic |
| シグネチャに出る (`--api`) | 出る | 出る | 出ない |
| 追加コスト (checked) | 比較 1 回 | 比較 2 回 (順序: 契約 → panic) | 0 |

A1 を推さない理由は §0。`push_char` だけは違反してもメモリ安全に触れない
(不正な UTF-8 が出るだけ) ので A1 でよい。`grow_to` は private な内部
手続きで、呼び出し側は 2 箇所とも `new_cap > self.len` を保証しているので
契約は**文書として**の価値だけ。

A2 を選ぶと **panic 文を pin しているテストが 2 本変わる** (§6)。

### B. 状態遷移を約束する (`ensures` + `old`)

**長さと容量**についての節。要素値には触れない (下の「不採用」)。

| method | 節 | 備考 |
|---|---|---|
| `push(value)` | `ensures self.len == old(self.len) + 1u64` / `ensures self.cap >= self.len` | 最頻の method。§1 のコストはこの形の実測 |
| `pop()` | `ensures self.len == old(self.len) - 1u64` / `ensures self.cap == old(self.cap)` | A の `requires self.len > 0` があれば減算は安全 |
| `insert(index, value)` | `ensures self.len == old(self.len) + 1u64` | |
| `remove(index)` / `swap_remove(index)` | `ensures self.len == old(self.len) - 1u64` / `ensures self.cap == old(self.cap)` | |
| `clear()` | `ensures self.len == 0u64` / `ensures self.cap == old(self.cap)` | 「容量は保つ」がコメントから契約になる |
| `set_size(n)` | `ensures self.len == n` | |
| `resize(n)` (`impl<T: Default>`) | `ensures self.len == n` | |
| `reverse()` | `ensures self.len == old(self.len)` | |
| `sort()` / `sort_by(less)` | `ensures self.len == old(self.len)` | 整列性は E 参照 |
| `try_reserve(n)` | `ensures result.is_err() \|\| self.cap >= self.len + n` | `?` 経由の早期 return でも `result` は `Err` 全体 (design_by_contract.md) |
| `extend_bytes(src, count)` | `ensures self.len == old(self.len) + count` | `impl Vec<u8>` |
| `push_str(other)` | `ensures self.len == old(self.len) + other.size()` | 同上 |
| `push_char(c)` | `ensures self.len >= old(self.len) + 1u64` / `ensures self.len <= old(self.len) + 4u64` | RFC 3629 の 1〜4 byte |
| `iter()` | `ensures result.len == self.len` | 書けるようになった (§5-2) |
| `with_capacity(n)` / `try_with_capacity(n)` / `from_str(s)` / `clone()` | `ensures result.len == ...` / `result.cap == n` | 同上。`with_capacity` は入れた |
| `VecIter::next()` | `ensures self.index <= self.len` | イテレーション 1 回ごとに評価される。for ループの内側なので、入れるなら B の中で最後 |

**不採用: 要素値の契約。** `push` に `ensures self.get(self.len - 1u64) ==
value`、`set` に `ensures self.get(index) == value` は書けるし 3 バックエンド
で通るが、`==` が generic `T` に `eq` を要求するので、`eq` を持たない
struct を要素にした `Vec` の `push` 呼び出しが**すべて** `[E0010]` になる
(確認済み)。`Vec<T>` に事実上 `T: Eq` の bound を課すことになるので不可。

### C. メモリ挙動を約束する

2 系統ある。**静的 (`never_allocates`) の方がゼロコストで強い**。
§5-1 の parser の穴は 2026-09-21 に解消した。

| method | 静的 `never_allocates` | 実行時 `ensures` |
|---|---|---|
| `size` / `capacity` / `is_empty` / `as_ptr` / `clear` / `set_size` / `iter` | 今すぐ書ける (unsafe でない) | 不要 (静的で足りる) |
| `get` / `set` / `pop` / `remove` / `swap_remove` / `reverse` / `VecIter::next` | 書ける (§5-1、2026-09-21)。`get` は入れた | `ensures allocations(0u64)` で代用可 |
| `sort` | `unsafe` ではないが `T: Ord` の `lt` を辿れるか**未確認** (`String` の `lt` は extern に届く → 拒否される可能性) | `ensures allocations(0u64)` |
| `sort_by(less)` | 不可 (closure 呼び出しは追えない仕様) | `ensures allocations(0u64)` |
| `push` / `insert` | 不可 (確保する) | `ensures allocations(0u64)` + `ensures __builtin_realloc_count() <= old(__builtin_realloc_count()) + 1u64` (「確保は realloc 高々 1 回」。糖衣に realloc の軸が無いので生で書く。確認済み) |
| `with_capacity(n)` / `from_str(s)` | 不可 | `ensures allocations(1u64)` |
| `try_reserve(n)` | 不可 | `ensures __builtin_realloc_count() <= old(__builtin_realloc_count()) + 1u64` |
| `clone()` | 不可 | `ensures allocations(1u64)` は**成立しない** (`Vec::new()` + 逐次 `push` で成長するため realloc が log n 回)。書くなら B の長さだけ |

`never_allocates` は「確保しえない」をコンパイル時に検査し、それ以降
body に heap 呼び出しが増えた日に `[E0016]` で止まる。**`--release` でも
効き、実行時コストが無い**。`Vec` の読み取り系がこれを名乗ることの
価値は、その上に建つ `String::get` / `Span` / net の `read` ループが
「確保しない」を推移的に主張できるようになることにある。

### D. 型の不変条件 (`invariant` は未実装)

`Vec` の不変条件は 3 つ:

1. `self.len <= self.cap`
2. ~~`self.elem_size == 0u64 || self.elem_size == __builtin_sizeof::<T>()`~~
   — 2026-09-25 に `elem_size` フィールドごと消えた (MEMORY-ACCESS M5)。
   stride は型から出るので、破れる不変条件ではなくなった
3. `self.cap == 0u64 || !__builtin_ptr_is_null(self.data)`

`invariant` 節が無いので、書くなら代用形:

| | D1: 各 method に `ensures self.len <= self.cap` | D2: `fn well_formed(&self) -> bool` を足し `requires self.well_formed()` / `ensures self.well_formed()` |
|---|---|---|
| 変更する method 数 | 変更系 10 本に 1 行ずつ | 同じ 10 本 + helper 1 本 |
| 条件を増やすとき | 10 箇所 | 1 箇所 |
| コスト | 比較 1 回 | 呼び出し + 比較 2〜3 回 |
| `--api` の読みやすさ | 条件が見える | `well_formed()` の中を見に行く |

D は B と重なる (`push` の `ensures self.cap >= self.len` は不変条件 1 その
もの)。**B を選ぶなら D1 は B に吸収され、D2 は不要**。D2 が要るのは
条件 3 まで縛りたいとき (2 は消えた) で、それは `Vec` の内部実装の検査であって
利用者向けの仕様ではない。優先度は低い。言語側に `invariant` を足す
話は [`todo.md`](todo.md) の DBC 節に既出 (未実装)。

### E. 契約のために helper を足す

| helper | 使う場所 | 判断 |
|---|---|---|
| `is_sorted(&self) -> bool` (`impl<T: Ord>`) | `sort` の `ensures self.is_sorted()` | 書ける (確認済み、O(n) で `sort` の O(n²) の下)。ただし `E0018` 警告が出る (`String` の `lt` が extern に届くため)。警告を許容するか、純粋性検査に「extern の read-only 申告」を足すかの二択 |
| `well_formed(&self) -> bool` | D2 | D2 を選ぶときだけ |
| `sort_by` の `is_sorted_by(less)` | — | closure 呼び出しは純粋性検査が追えず、`never_allocates` と同じ理由で warn。不採用 |

`is_sorted` は契約と無関係に API として有用 (`Vec` に無い)。足すなら
[`COLLECTIONS.md`](COLLECTIONS.md) の `Vec` 拡張に載せる。

### F. trait 側の契約 (`Iterator<T>::next`)

`core/std/iter.t` の `trait Iterator<T>` に節を書けば `VecIter` /
`MapIter` / `FilterIter` / `EnumerateIter` / `ZipIter` / `DictIter` /
`StringIter` の全 impl に継承される (DBC-TRAIT-INHERIT)。しかし `next`
の signature は `(&mut self) -> Option<T>` で、trait はフィールドを
知らないので**書ける節が無い** (`result` だけでは「終端後は `None` の
まま」も言えない)。**対象外**。

## 3. 選ばない方がよいもの (まとめ)

| 案 | 理由 |
|---|---|
| A1 で panic を消す (`get` / `set` / `pop` / `insert` / `remove` / `swap_remove` / `set_size`) | `--release` で範囲外アクセスが unchecked になる。組み込み配列は release でも guard を残す設計と食い違う |
| 要素値の `ensures` (`self.get(i) == value`) | `Vec<T>` 全体に `eq` を要求してしまう (確認済み) |
| `-> Self` / `-> VecIter<T>` を返す method の `ensures result.field` | 書ける (§5-2、2026-09-21) |
| `never_allocates` を `unsafe fn` に重ねる | 書ける (§5-1、2026-09-21) |
| `VecIter::next` の `ensures` | for ループ 1 周ごとのコスト。B の他がすべて入って、なお欲しいときに |
| `clone` の `allocations(1)` | 実装が逐次 push なので成立しない |

## 4. 選択シート

各行を選ぶ / 選ばないで答える。既定 (推奨) は ◎。

| # | 分類 | 内容 | 推奨 | 選択 |
|---|---|---|---|---|
| 1 | A2 | 境界の `requires` を panic に**併記** (`get` / `set` / `pop` / `insert` / `remove` / `swap_remove` / `set_size`) | ◎ | **✅ 2026-09-04** |
| 2 | A1 | `push_char` の `assert` 2 本を `requires` に置換 | ○ | **✅ 2026-09-04** |
| 3 | A | `grow_to` に `requires new_cap >= self.len` (文書目的) | △ | **✅ 2026-09-04** |
| 4 | B | 長さ / 容量の `ensures` — `push` / `pop` / `insert` / `remove` / `swap_remove` / `clear` / `set_size` / `resize` / `reverse` / `sort` / `sort_by` | ◎ | 未 |
| 5 | B | `impl Vec<u8>` の `ensures` — `extend_bytes` / `push_str` / `push_char` | ◎ | 未 |
| 6 | B | `try_reserve` の `ensures result.is_err() \|\| self.cap >= self.len + n` | ○ | 未 |
| 7 | B | `VecIter::next` の `ensures self.index <= self.len` | △ | 未 |
| 8 | C | `never_allocates` を unsafe でない読み取り系 7 本に (`size` / `capacity` / `is_empty` / `as_ptr` / `clear` / `set_size` / `iter`) | ◎ | 未 |
| 9 | C | `push` / `insert` に「realloc 高々 1 回」の生の `ensures` | ○ | 未 |
| 10 | C | `with_capacity` / `from_str` に `ensures allocations(1u64)` | ○ | 未 |
| 11 | C | `get` / `set` / `pop` / ... の `ensures allocations(0u64)` (§5-1 が直るまでの代用) | △ (直ったら 8 に統合) | 未 |
| 12 | E | `is_sorted` を足し `sort` に `ensures self.is_sorted()` (`E0018` 警告つき) | △ | 未 |
| 13 | D2 | `well_formed()` 方式の擬似 invariant | × (B で足りる) | × |

「◎ だけ」で選ぶと、`vec.t` の変更は契約行 **約 35 行** と panic 文の
テスト更新 2 箇所 (§6)、コードの本体は 1 行も動かない。

### 4-1. #1〜#3 を入れた結果 (2026-09-04)

- 契約行は **11 本** (`requires` 9 本 + `push_char` の 2 本)。`assert` 2 本が
  消え、body のロジックは 1 行も動いていない
- `--api core/std/collections/vec.t` に 9 method の節が出る
- 違反の文言は 3 レーン一致で
  `` `requires` clause #1 of function `get` ... (with index = 5) ``。
  `--release` と `INTERPRETER_CONTRACTS=off` では従来どおり
  `panic: Vec::get index out of bounds` (A2 の狙いどおり)
- コスト: `Vec` 以外に何もしない AOT ループ (`with_capacity` +
  `push` × 1000 + `get` × 1000 を 20 万回 = 4 億呼び出し) で
  **0.74s (checked) vs 0.65s (`--release`)**。1 呼び出し **~0.22ns**、
  比 1.14x で §6 の閾値 (2 倍) の内側
- `--check` の対象数は変わらない (1 件) — §5-3 のとおり `ptr` レシーバの
  method は黙って飛ばされるので、入れた 9 本はどれも掃かれていない。
  検証はテストスイート (2827 件) と `example_consistency` が担っている

## 5. 調査で見つかった言語側の穴

契約とは独立に直す価値があるもの。`todo.md` に登録済み。

### 5-1. method に `never_allocates` と `unsafe` を重ねられなかった (**2026-09-21 に解消**)

`frontend/src/parser/stmt.rs::parse_method_modifiers` が「次のトークンが
`fn`」を両方の修飾子の条件にしていたので、`never_allocates unsafe fn` /
`unsafe never_allocates fn` は impl ブロック内で parse エラーだった
(自由関数の `program_parser.rs` は「次がもう 1 つの修飾子」も通す)。
並び全体を先に見てから消費する形にして解決。`Vec::get` が実例。

**ただし静的形は思っていたほど必要ではなかった**: `never_allocates` の
検査は到達可能性なので、**呼び出し側が名乗るのに被呼び出し側の宣言は
要らない** (`never_allocates fn total(v: &Vec<u64>)` は `get` が
名乗らなくても通る)。宣言の値打ちは「将来 `get` が確保したらここで
落ちる」ことと、`--api` / `--effects` に出ることにある。

### 5-2. `ensures result.field` が compiled lane で落ちていた (**2026-09-21 に解消**)

`fn with_capacity(n: u64) -> Self ensures result.cap == n` は interpreter で
通り、AOT / JIT は `field access on a non-struct value` だった。`result` が
**先頭の戻り値 1 本**にスカラーとして束縛されていたのが原因で、compound の
戻りは leaf ごとに 1 本なので「その先頭の leaf」を指していた
(DBC-RESULT-FIELD)。戻り型の形どおりに leaf を束縛して解決。
**`with_capacity` は `ensures result.capacity() == n` と
`ensures result.size() == 0u64` を持っている。**

### 5-3. `--check` が `ptr` レシーバの method を黙って飛ばす

design_by_contract.md に「黙って対象外」と明記はあるが、`Vec` のように
契約が増えるほど「検査されたつもり」の危険が増える。最低限
`SKIPPED Vec::get — receiver has a `ptr` field` の 1 行を出すか、
レシーバを `Vec::new()` + ランダムな `push` 列で**構築して**生成する
(構築子経由の生成) のが次の一手。後者は `--check` を collection に
効かせる唯一の道。

## 6. 適用時に一緒に動くもの

- **テスト**: `interpreter/tests/runtime_observability_tests.rs` の
  425 行 (`Vec::get index out of bounds`) と 439 行 (`Vec::pop on an empty
  Vec`) が panic 文を pin している。A2 では契約が先に発火するので、
  期待文言を `Contract violation: \`requires\` clause #1 of function \`get\`
  ... (with index = N)` に変える。`collections_tuple_struct_tests.rs:1527`
  は「エラーになること」だけを見ているので影響なし
- **文書**: [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) の D6 が
  「stdlib の境界チェックを panic にした」経緯を記録している。A2 なら
  「契約が先、panic は release 用の網」と追記する。`vec.t` 冒頭の API
  コメントの「panics when empty」等は契約が言うので削る
- **性能**: `push` のコストは §1 の 0.8ns/呼び出し。`interpreter/benches`
  に `Vec` を回すベンチがあれば適用前後で 1 度だけ取る。閾値は
  「checked ビルドで 2 倍を超えない」
- **確認手順**: `cargo nextest run` (stdlib の下に居るので全体) →
  `compiler/tests/example_consistency.rs` (example 全部を 3 レーンで) →
  `cargo run -q -p interpreter -- --api core/std/collections/vec.t` で
  契約が signature に出ていることを目視 → `--check` を 1 度走らせて
  `with_capacity` が掃かれる (レシーバ無しなので対象になる) ことと、
  `n = u64::MAX` で `EXHAUSTED` / `FAILED` にならないことを見る
  (現状の `with_capacity` は `checked_mul` で overflow を panic にして
  いるので、`--check` がこれを違反と数えるなら `requires n <=
  u64::MAX / sizeof` 相当の節が要る — 要確認)

## 7. 決めていないこと

- `--release` で契約が消えることを `Vec` の利用者にどう見せるか。
  `--api` は契約を無条件に出すので、release バイナリの挙動と
  signature が食い違う。これは `Vec` 固有ではなく DBC 全体の話
- `E0018` を stdlib で許容するか (案 12)。警告は毎コンパイルで出るので、
  stdlib に 1 本でも入れると全プログラムのビルドに警告が乗る。
  入れるなら純粋性検査側に「extern の read-only 申告」が先
