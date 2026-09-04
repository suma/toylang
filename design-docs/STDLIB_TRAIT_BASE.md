# STDLIB TRAIT BASE — `Iterator` / `Clone` / `Default` と、bound が書けない 2 つの穴

> **状態: B0〜B5 すべて landing 済み (2026-09-03)。** 以下は決定の記録と
> して残す。積み残しは 2 つだけ: B3 の `Clone` は primitive 全幅 +
> `str` + `String` のみで、コンテナ (`Vec` / `Box`) は todo.md の
> VEC-CLONE-WITH-STRING-CLONE 待ち。`&T` が primitive に解決される
> generic 関数は compiled lane が拒否する (GENERIC-SCALAR-REF)。

> 対象: `core/std/iter.t` / `core/std/cmp.t` / `core/std/hash.t` と、
> stdlib の 15 個の反復子 (`vec.t` / `dict.t` / `string.t` / `set.t` /
> `deque.t` / `soa_vec.t`)
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-TRAIT-BASE**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md)
> 実測: 2026-09-03 (この文書の数値と診断文はすべてこの日に取った)

## Status snapshot

| 項目 | 状態 |
|---|---|
| `trait Iterator<T>` | **本物の trait として動く** (`fn f<I: Iterator<i64>>(it: I)` が 3 レーン一致)。ただし stdlib の反復子は**1 つも名乗っていない** |
| `Ord` | ある (`lt` のみ、全 primitive + `f64` + `String`)。`Vec::sort` が利用者 |
| `Hash` | ある (全 primitive + `str` + `String`)。`Dict` / `Set` が利用者 |
| `Display` | ある。ただし trait ではなく **method の有無**で dispatch |
| `Eq` | 無い。**要らない** (§2) |
| `Clone` | 無い。**bound 越しに呼べないので今は書けない** (実測 4) |
| `Default` | 無い。同上 |
| `&mut T` を取る generic 関数 | **書けない** (実測 3) |
| trait 継承 / associated types | parse error。診断は生のトークン名 `BraceOpen` |

## なぜ今これを設計するか

**`iter.t` は「documentation-only」と書かれたまま、実際にはもう動く。**
todo.md も CLAUDE.md も「generic trait `Iterator<T>` が未対応なので for
ループは duck typing で回っている」と書いているが、測ったら
`fn first<I: Iterator<i64>>(it: I) -> i64` は 3 レーンとも通った
(実測 1)。**この分野は「何が無いか」の把握自体が古い。**

そして分野としての本題は trait そのものではなく、**bound を書いても
その先で何もできない 2 つの穴** (実測 3・4) にある。`Ord` が例外的に
使えているのは `lt` が `bool` を返し、レシーバを `&mut` で取らないから
で、**`Self` を返す trait と `&mut` を取る trait はどちらも今日書けない**。
`Clone` も `Default` も算術 trait もそこに落ちる。

## 測ったこと (2026-09-03)

すべて `--all-backends` (interpreter / cranelift JIT / AOT) で確認した。

1. **`trait Iterator<T>` は bound として動く。**
   `impl Iterator<i64> for Counter` を書き、
   `fn first<I: Iterator<i64>>(it: I) -> i64` を呼ぶプログラムが 3 レーン
   一致 (exit=1)。**`for x in it` を generic 関数の中で回す**のも動く
   (exit=6 = 1+2+3)。`iter.t` の doc comment (「ITER-PROTOCOL-TRAIT が
   制限を外した」) が正しく、todo.md / CLAUDE.md が古い。

2. **stdlib の反復子は 15 個あって、1 つも trait を名乗っていない。**
   `fn next(&mut self) -> Option<T>` を持つ impl は
   `vec.t` 5 / `string.t` 4 / `dict.t` 3 / `set.t` 1 / `deque.t` 1 /
   `soa_vec.t` 1。全部 inherent method なので、**`Iterator<T>` を bound に
   取る関数に stdlib の反復子を渡せない**。for ループが structural
   (duck-typed) で回るので、今まで誰も困らずに来た。

3. **`&mut T` は呼び出し側で推論できない。**
   ```
   [E0010] Type inference failed for generic function 'go':
           Constraint solving failed: Cannot unify `&mut T` with `&mut P`
   ```
   `fn go<T: Bump>(v: &mut T)` を `go(&mut p)` で呼ぶと落ちる。
   **`&T` は通る** (同じ形で `fn dup<T: Clone>(v: &T)` は引数の束縛を
   越えた)。つまり穴は `&mut` の側だけ。

4. **`Self` を返す trait method は bound 越しに呼べない。**
   ```
   [E0010] DEBUG: Method 'clone' returned unresolved Generic('T') for object type T
   ```
   `fn dup<T: Clone>(v: &T) -> T { v.clone() }` も
   `fn reset<T: Zero>(v: T) -> T { v.zero() }` も同じ形で落ちる。
   associated function 版 (`fn make<T: Default>() -> T { T::default() }`)
   は戻りが `Unknown` になり、その先で
   `[E0004] Unsupported operation 'field access' for type Unknown` に化ける。
   **直接呼びは 3 レーンで動く** — `p.clone()` も `P::default()` も一致した。
   問題は「型パラメータをレシーバにしたときに `Self` が置換されない」
   1 点だけ。診断が `DEBUG:` を含んでいるのもここ。

5. **trait 継承と associated types は parse error で、診断が生のトークン名。**
   `trait B: A { ... }` も `trait Container { type Item ... }` も
   ```
      |        ^ BraceOpen
   ```
   としか言わない。`Unexpected token` の類の文言すら無い。

6. **動くものも測った** — `impl<T> Sum for Vec<T>` (generic 型への trait
   impl) は 3 レーン一致。`unsafe fn next` で**safe な trait method を
   実装する**のも一致 (stdlib の反復子は全部 `unsafe fn next` なので
   これが通らないと §3 が成立しない)。

7. **同じ method を inherent と trait impl の両方に書くと実行時に落ちる。**
   `duplicate impl registration for `Counter::next` with same target type
   args []` — 型検査ではなく実行時。§3 は「足す」ではなく「**移す**」で
   なければならない、という制約になる。

## 既存の決定から引く制約

1. **bound は呼び出し側で強制される** (`E0010`)。`impl<T: Ord> Vec<T>` /
   `Dict<K: Hash, V>` が既にその形。新しい trait も同じ機構に乗る。
2. **`Display` は trait ではなく method の有無で dispatch する**
   (`==` → `eq` と同じ流儀、CLAUDE.md)。**trait を増やす前に、
   method の有無で足りないかを毎回訊く**のがこの言語の既定。
3. **8 レジスタの返し予算** (COLLECTIONS 制約 3)。反復子のフィールドを
   増やす変更はできない。§3 は宣言を足すだけでフィールドに触らない。
4. **`val b = a` は compound では alias で、移動ではない** (CLOSURE /
   MOVE の決定)。**深いコピーを作る唯一の方法が `Clone`** になるので、
   `Clone` は「あると便利」ではなく所有モデルの一部。
5. **受け入れは 3 レーン一致**。trait の dispatch は tree-walker と
   compiled で別実装なので、`impl` を足すたびに pin する。

## 1. `Iterator<T>` — 形を今ここで確定させる

**採用: `trait Iterator<T>` のまま。associated types (A4) が入っても
`Item` に移さない。**

todo.md は「A4 が入ると `Iterator` の形が変わるので、順序は型システム側と
揃える」と書いていた。これへの回答:

- **移す費用**: stdlib の 15 impl + user のコード全部。
- **移して得るもの**: 呼び出し側が `<I: Iterator<i64>>` を
  `<I: Iterator>` と書けること。**それだけ。**
- 型引数を明示する形には副次的な利点もある — `Iterator<(K, V)>` のように
  **同じ struct が複数の T で名乗る**余地が残る (`Item` は 1 つに固定する)。

A4 を待つ理由が無いので、**この分野は A4 と独立に進める**。

## 2. `Eq` は置かない

COLLECTIONS C0(a) で決着済みの性質をそのまま引く:

- generic な `==` は **bound 無しで動き**、`T` が `eq` を持つ struct なら
  それに dispatch する。
- `eq` を持たない型を渡した呼び出しは **`[E0010]` で落ちる**
  (`eq_requirement.rs` が全 body と全呼び出しを突き合わせる)。

つまり `Eq` を宣言しても**検査は 1 つも増えない**。増えるのは
「`impl Eq for ...` を書き忘れた型が使えなくなる」という新しい失敗だけ。
制約 2 (trait を増やす前に method の有無で足りないかを訊く) の
そのままの適用。

同じ理由で **`PartialOrd` / `PartialEq` の区別も置かない**。`f64` の
NaN は `Ord` の doc comment で言う (`lt` は IEEE の `<`、`sort` は
NaN を含む配列で順序を保証しない)。

## 3. stdlib の 15 個に `Iterator<T>` を名乗らせる

実測 2 の穴を塞ぐ。**inherent の `next` を trait impl へ移す**
(実測 7 より、両方に書くと実行時に落ちるので「足す」ではない):

```
impl<T> VecIter<T> {          →   impl<T> Iterator<T> for VecIter<T> {
    unsafe fn next(...) ...           unsafe fn next(...) ...
}                                  }
```

- **for ループは structural なので互換** — desugar は `next` の有無しか
  見ない (`frontend/src/parser/stmt.rs`)。既存プログラムは 1 行も
  変わらない。
- **`unsafe` は impl 側に残せる** (実測 6)。trait 宣言は safe のまま。
- 対象は 15 個: `VecIter` / `MapIter` / `FilterIter` / `EnumerateIter` /
  `ZipIter` (vec.t)、`StringIter` / `StringMapIter` / `StringFilterIter` /
  `StringEnumerateIter` (string.t)、`DictIter` / `DictMapIter` /
  `DictFilterIter` (dict.t)、`SetIter` / `DequeIter` / `SoaVecIter`。
- **得るもの**: `fn sum<I: Iterator<i64>>(it: I) -> i64` のような
  「反復子を取る関数」が user 空間で書けるようになる。今は
  `Vec<i64>` を取る関数しか書けず、`v.iter().filter(...)` の結果を
  渡す先が無い。

**受け入れ**: 15 個それぞれについて (a) for ループが従来どおり回ること、
(b) `<I: Iterator<T>>` の関数に渡せること、を 3 レーンで pin。

## 4. `Self` 戻りの穴 (実測 4) — 何を直すか

これが `Clone` / `Default` / 将来の算術 trait すべての前提。

現象は「型パラメータをレシーバにしたとき、method の戻り型 `Self` /
trait の型引数が**呼び出し側の実型で置換されない**」。2026-09-01 の
**SELF-IN-TYPE-ARG / GENERIC-IN-ENUM-PAYLOAD** で直したのと同じ層の
話で、あのときは「注釈の一番外側しか見ない」欠陥が型検査・lowering・
tree-walker の 3 層にあった。`TypeDecl::substitute_self` は
frontend にあるので、**置換の道具は既にある** — 使われていない経路が
残っている形。

順序:

1. 診断から `DEBUG:` を消す (実測 4 の文言はそのまま user に出ている)。
2. `v.m()` (レシーバが型パラメータ、戻り `Self`) を置換する。
3. `T::assoc()` (型パラメータ経由の associated function) を解決する。

`T::assoc()` は 2 と別物 — レシーバの値が無いので、型引数の**推論元**が
戻り位置しかない。`Default` はこの形なので、**`Clone` (2 で足りる) を
先に、`Default` (3 が要る) を後に**する。

## 5. `&mut T` の穴 (実測 3)

`Cannot unify &mut T with &mut P` は**単一化のテーブルに `&mut` の行が
無い**形に見える (`&T` は通るので構造自体はある)。TYPE-NAME-SPELLING
(2026-08-23) で `unify_types` に arm を足したのと同じ作業。

これが無いと書けないもの:

- `fn advance<I: Iterator<T>>(it: &mut I)` — **反復子を借りて進める関数**。
  今は by value しか書けないので、呼び出し側の反復子は使い終わる。
  `collect` が `self: Self` (by value) なのは 8 レジスタの都合という
  別の理由だが、**by value しか選べない**という結果は同じ。
- `fn fill<T: Sink>(dst: &mut T, n: u64)` のような generic な出力先。

## 6. `Clone`

```
pub trait Clone {
    fn clone(&self) -> Self
}
```

`core/std/clone.t` に置く (`cmp.t` / `hash.t` と同じ 1 trait 1 ファイル)。

- **深いコピー**。`Drop` を持つ型 (`Vec` / `String` / `Box` / `Dict` /
  `Set` / `Deque`) は**新しい確保**を持ち、元とは独立に free される。
  `String::to_string()` が既にその実装 (バイトを push し直す) なので、
  `impl Clone for String` はそれに委譲する。
- **primitive 全幅に impl** (`Hash` / `Ord` と同じ)。`self` を返すだけ。
- **`impl<T: Clone> Clone for Vec<T>`** — 要素ごとに `clone()`。bound は
  impl block に書く (`impl<T: Ord> Vec<T>::sort` の先例)。
- **所有モデルとの関係を doc comment に書く**: `val b = a` は compound
  では alias で、`a` を container に入れる (`E0014` の移動) と `a` は
  読めなくなる。**`a.clone()` はその回避手段**であって、`Clone` を
  持つ型が移動しなくなるわけではない。
- **`Copy` は置かない** — 「移動しない型」を型で言う仕組みは
  move 検査 (E0014) 側の話で、`impl Drop` の有無で既に決まっている。

## 7. `Default`

```
pub trait Default {
    fn default() -> Self
}
```

- §4 の 3 (型パラメータ経由の associated function) が前提。
- primitive: 数値 0 / `false` / 空の `String`・`Vec`・`Dict`。
- **利用者**: `Vec::resize(n, T::default())` / `Dict::get_or_default(k)` /
  将来の SERIALIZE (JSON の欠けたフィールド)。**利用者が無い trait は
  置かない**ので、`Default` は resize / get_or_default と同じ Phase で
  landing させる。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **B0** | 記述の是正 (実測 1) と診断 3 件 (`DEBUG:` / 生トークン名 `BraceOpen` / duplicate impl が実行時) | todo.md / CLAUDE.md / `iter.t` が一致。診断の文言 pin |
| **B1** | `Self` 戻りの置換 (§4 の 1・2) | `fn dup<T: Clone>(v: &T) -> T` が 3 レーン一致 |
| **B2** | `impl Iterator<T>` を 15 個に (§3) | 15 個 × (for ループ / bound 経由) の 3 レーン pin |
| **B3** | `Clone` (§6) | primitive 全幅 + `String` / `Vec` / `Box` の 3 レーン一致 + `--profile=mem` で二重 free が無いこと |
| **B4** | `&mut T` の単一化 (§5) | `fn advance<I: Iterator<T>>(it: &mut I)` が 3 レーン一致 |
| **B5** | `T::assoc()` (§4 の 3) + `Default` (§7) | `Vec::resize` / `Dict::get_or_default` と同時 |

B0 → B1 → B3 が主線 (`Clone` まで)。**B2 は B1 と独立**なので順不同に
できるが、B2 が landing すると「反復子を取る関数」が書けるようになり、
その関数のほとんどが B4 (`&mut I`) を欲しがるはずなので、B2 の直後に
B4 を置くのが自然。

## 非目標

- **trait 継承 (A3) / associated types (A4)** — 型システム側の項目。
  §1 でこの分野を A4 と切り離したので、待つ必要はない。ただし
  **parse error の診断だけは B0 で直す** (実測 5 の `BraceOpen` は
  「未対応」ではなく「何も言っていない」)。
- **`Copy` / `PartialEq` / `PartialOrd`** — §2・§6。
- **higher-kinded / generic associated types** — 要る場面が無い。
- **`dyn Trait` への generic trait** — `Iterator<T>` を `&dyn` で持つ形は
  enum が `dyn` に載らないのと同じ制約群に属する。ERROR_MODEL が
  「共通の `Error` trait を置かない」と決めたのと同じ理由で、
  必要になるまで開けない。
- **`Iterator` の default method** (`map` / `filter` を trait 側に置く)
  — 今のアダプタは struct ごとに実装されていて 3 レーンで動いている。
  default body に移すと 8 レジスタ予算と AOT の closure 制約
  (scalar しか capture できない) に一斉に当たるので、**測ってから**。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — 分野の俯瞰と優先順位
- [`COLLECTIONS.md`](COLLECTIONS.md) — `Eq` を置かない根拠 (C0(a)) と
  8 レジスタ予算
- [`STDLIB_TEXT.md`](STDLIB_TEXT.md) — `Ord for str` (この文書の
  `Ord` の利用者が 1 つ増える)
- [`ERROR_MODEL.md`](ERROR_MODEL.md) — 共通 trait を置かない判断の先例
- [`docs/language.md`](../docs/language.md) — trait / bound の正本
