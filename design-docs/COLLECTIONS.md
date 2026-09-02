# COLLECTIONS — Dict の hash 化・`Set<T>`・順序つきコンテナ・`Vec` の穴

> 対象: `core/std/dict.t` / `core/std/hash.t` / `core/std/collections/vec.t`
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-COLLECTIONS**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P1
> 実測: 2026-09-02 (この文書の数値はすべてこの日に取った)
> 進捗: **C0 は 2026-09-02 に完了** ((a) missing-`eq` の型検査 / (b)
> `Hash for str` + `impl Hash for String` / (c) `mix()`)。次は C1

## Status snapshot

| 項目 | 状態 |
|---|---|
| `Dict<K, V>` | 線形探索。insert / get / contains_key / remove が **O(n)** |
| `Hash` | trait と primitive / `str` / `String` の impl + 表側の `mix()` は揃った (2026-09-02)。表がまだ使っていない |
| `Set<T>` | 無い |
| Deque / PriorityQueue | 無い |
| `Vec<T>` の `insert` / `remove` / `contains` / `index_of` / `reverse` / `sort_by` | 無い |
| 組み込み `dict[K, V]` (リテラル `dict{...}`) | **interpreter のみ** (`compiler MVP cannot lower a dict literal yet`) |

## なぜ今これを設計するか

**線形探索の Dict は既に実用の外にいる。** 相異なるキーを n 個入れて n 回
引く (線形探索の最悪形) を測った:

| n | interpreter (既定エンジン) | AOT |
|---|---|---|
| 1,000 | 10.0s | — |
| 2,000 | 39.3s | — |
| 10,000 | **120s でも終わらない** | 0.06s |
| 20,000 | — | 0.26s |
| 40,000 | — | 1.03s |

AOT 側はきれいに 4 倍ずつ (n を倍にすると 4 倍) で、二次であることが
そのまま見えている。**1,000 キーの辞書が既定エンジンで 10 秒**というのは、
「実プログラムが書けない」の定義そのもの。

そして **Dict / `Set` / PriorityQueue / `Vec` 拡張は同じ 3 つの機構を
共有する** — 表 (probe とリサイズ)、generic な `==` / `hash()` の
ディスパッチ、8 レジスタの返し予算。個別に着手すると設計が割れるので、
分野としてまとめて決める。

## 測ったこと (2026-09-02)

RUNTIME_LIBRARY.md が「着手時の実測事項」としていた論点は、先に測ったら
**4 つとも決着した**。以下はすべて `--all-backends` で 3 レーン一致を
確認済み (断りのあるものを除く)。

1. **generic な `==` は動く。`Eq` bound は要らない。**
   `impl<T> Bag<T>` の中で `e == needle` (`e: T`) が 3 レーンで通り、
   `T` が `eq` method を持つ struct なら**その `eq` に dispatch する**
   (フィールドを構造的に比べるのではない — `a` だけ見る `eq` を書いて
   確認した)。`Vec::contains` / `index_of` / `Dict` の keyed 操作は
   bound 無しで書ける。
2. **ただし `eq` を持たない型を渡すと実行時に壊れる** (C0 (a) で解消済み。
   以下は着手前の記録)**。**
   `Bag<Point>` (`Point` に `eq` 無し) は**型検査を通り**、実行時に
   `Type error: expected Struct(SymbolU32 { value: 60 }, []), found
   Struct(SymbolU32 { value: 60 }, []). evaluate_eq: Bad types` で落ちる
   — **同じ型を「期待と違う」と言う** `{:?}` 生出力の診断。
   これは C0 で先に直す (下記)。
3. **`K: Hash` bound + `key.hash()` は動く。**
   `struct Table<K: Hash, V>` / `impl<K: Hash, V>` の中の `key.hash()` が
   primitive でも user struct の `impl Hash` でも 3 レーンで解決する。
   open addressing に必要な機構は既にある。
4. **`Set<T>` を `Dict<T, ()>` では書けない。**
   interpreter は通るが JIT / AOT が
   ``parameter `value` cannot have type Unit`` で拒否する
   (UNIT-STRUCT-FIELD と同じ「`()` が値として通らない位置」の一種)。

ついでに 2 つ、既存の記述が現実と食い違っているのを見つけた:

5. **`dict.t` の「insertion order」は既に嘘。** `remove` が swap-remove
   なので、`1,2,3` を入れて `1` を消すと反復は **`3, 2`** になる (実測)。
   反復順の仕様は「変える / 変えない」ではなく **今も未規定に近い**もの
   として決め直す必要がある。
6. **`hash.t` の `Hash for str` のコメントが陳腐化している。**
   「`BuiltinMethodCall::Len` は AOT で lower されないので `self.len()` は
   呼べない」と書いてあるが、`s.len()` は今 AOT で動く。0 定数を返して
   いる理由は**もう無い**。

## 既存の決定から引く制約

1. **純 toylang で書く** (`__builtin_heap_*` 経由)。そうすれば
   `--profile=mem` / `ensures allocates(N)` / REGION 検査 (E0022) が
   コレクションにも自動で効く。表も probe も toylang 側に置く。
2. **str のバイト走査は Rust 側** — RUNTIME-PORT R3/R4 で「str 系
   ヘルパの toylang 化は interpreter で 20〜1000 倍遅い」と実測済み。
   加えて `str::as_ptr()` は **interpreter では呼ぶたびに len+1 バイト
   確保する** (`core/std/str.t` の記述)。ハッシュは lookup ごとに走る
   ので、`Hash for str` を toylang のバイトループにすると
   「線形探索をやめて確保を増やした」で終わる。→ **extern**。
3. **8 レジスタの返し予算** — `DictIter` が `key_size`/`val_size` を
   1 フィールドに pack し、adapters が `count` を `index` の上位 32bit に
   詰めているのはこの予算のため (`dict.t` のコメント)。表の形を変える
   ときは**反復子のフィールド数を増やさない**ことが設計制約になる。
4. **bound は呼び出し側で強制される** (STDLIB-ORD-BOUND、`E0010`)。
   `impl<T: Ord> Vec<T>::sort` が既にその形なので、`K: Hash` も同じ
   機構に乗る。
5. **受け入れは 3 レーン一致** (`compiler/tests/consistency/`)。
   反復順を仕様にするなら、順序も pin する。

## 1. `Dict` の open addressing

### 1.1 layout — 採用: entries + slots (IndexMap 形)

```
entries:  keys[]   vals[]   (挿入順、現在の並列配列そのまま)
slots:    u32 の表  (EMPTY / DELETED / entries への index)
```

`slots` だけを power-of-two で持ち、`entries` は今の並列配列を**そのまま
残す**。probe は `slots` を叩き、当たった index で `entries` を読む。

採用理由は 3 つ:

- **反復子が無変更で済む。** `DictIter` / `DictMapIter` /
  `DictFilterIter` は `entries` を順に舐めるだけなので、制約 3 の
  レジスタ予算を触らずに済む。表を直接舐める形にすると反復子が
  `slots` と `entries` の両方を持つことになり、予算の再設計が要る。
- **反復順を挿入順に保てる** (下記 1.2)。
- **キーと値の並列配列という現行 layout を捨てない** — `key_size` /
  `val_size` を実幅で持つ扱い、`__builtin_ptr_read/write` の per-leaf
  展開、drop glue がそのまま効く。

`slots` を `u32` にするのは、`entries` の index であって値ではないから。
2^32 エントリを超える辞書はこの言語の用途外。

### 1.2 反復順 — 採用: **挿入順を維持し、仕様に書く**

RUNTIME_LIBRARY.md が「挿入順を維持するか未規定に引き下げるかを
`docs/language.md` で決めてから着手する」としていた論点。**維持**を採る:

- `entries` 形にすれば維持のコストは `slots` のメモリだけで、
  probe の速度には効かない。
- 決定性の規約 (時刻は UTC 固定、乱数は seed 再現、print はソート順) と
  揃う。**未規定にすると 3 レーン一致テストで順序を pin できなくなる**
  — 反復順が pin できない collection は、テストを書く側から見て
  一段使いにくい。
- 今の `remove` は swap-remove で既に順序を壊している (実測 5)。
  維持を選ぶと**むしろ現在の doc コメントに実装が追いつく**。

削除は `entries` に tombstone を立て、反復でスキップする。
live/total が 1/2 を切ったら `entries` を詰め直し (compaction)、
`slots` の index を張り替える。`docs/language.md` に
「`Dict` の反復は挿入順。削除されたキーは現れない。再挿入は末尾に付く」
と書く。

### 1.3 mixer — 採用: **表側で掛ける。`Hash` impl は素のまま**

`hash.t` の impl は identity (u64) / 符号ビット無し cast (narrow) の
ままにして、**表が `mix(key.hash())` を掛ける**。

理由: mixer を各 `impl Hash` に埋めると、**user が書いた `impl Hash` は
mixer を持たない**ので、power-of-two 表で下位ビットだけを見た瞬間に
分布が崩れる。「良い hash を書く」責任を user に押し付けない。
`hash()` の契約は「等しい値は等しい u64」だけに保ち、分散は表の仕事にする。

mixer は splitmix64 の finalizer 相当 (乗算 + xorshift 3 段) を
`hash.t` に `pub fn mix(h: u64) -> u64` として置く。u64 の乗算は wrap
するので (RUNTIME-TRAP の「`+` / `*` は wrap」) 追加の guard は要らない。

### 1.4 `Hash for str` — extern にする

制約 2 のとおり。`__extern_str_hash(s: str) -> u64` を 4 箇所セット
(stdlib 宣言 / `toylang_rt` 実装 / interpreter registry / 3 レーン一致
テスト) で入れる。**3 レーンで同じ値を返すこと**がテストの中身
(FNV-1a か wyhash — 実装を決め打ちして pin する。seed は入れない:
決定性規約と、seed の入手経路が無いため)。

`String` にも `impl Hash` が要る (現状は無い)。`String` は
`core/std/string.t` が型を所有しているので impl はそちらに置く
(`impl Ord for String` と同じ規約)。バイト走査は `String` が既に
`get(i)` を持つので純 toylang で書けるが、interpreter の速度が問題に
なるなら `as_ptr()` 経由で同じ extern に流す。

### 1.5 tombstone と成長閾値

- `slots` の状態は `EMPTY` / `DELETED` / index の 3 値。`u32` の
  最大 2 値を予約する。
- probe は linear probing (キャッシュ局所性、実装の単純さ)。
  `DELETED` は**探索では通過し、挿入では最初に見つけたものを再利用する**。
- 成長は **load factor 7/8** で `slots` を倍に。`slots` の再構築時に
  `DELETED` は消える。
- `entries` の tombstone 比率が 1/2 を超えたら compaction (1.2)。

### 1.6 `K: Hash` bound は **breaking change**

今日 `Dict<Key, u64>` は `Key` に `eq` があれば動く (実測)。
`impl<K: Hash, V> Dict<K, V>` にすると、`impl Hash for Key` を書いて
いないコードが `E0010` で落ちる。この言語に derive は無いので、
移行は手書き。

それでも bound を**付ける**。理由: bound 無しで `key.hash()` を呼ぶと、
測定 2 と同じ「型検査を通って実行時に壊れたメッセージで落ちる」形に
なる (`==` の側は C0 (a) で塞いだが、`hash()` は同じ機構に乗っていない —
`Hash` は**実在する trait** なので、bound を書けば既存の E0010 で済む)。`E0010` は呼び出し位置を指して「`Key` は `Hash` を実装していない」
と言える。`docs/language.md` と todo に移行手順 (`impl Hash for Key` を
書く) を明記する。

### 1.7 組み込み `dict[K, V]` はどうするか

言語には **stdlib `Dict<K, V>` とは別に**、リテラル構文を持つ組み込みの
`dict[K, V]` がある (`val d = dict{"a": 1u64}`)。これは
**interpreter でしか動かない** (実測: JIT / AOT が
`compiler MVP cannot lower a dict literal yet`)。

この文書では触らない。ただし「hash 化した `Dict` を入れると、
2 つある dict のうち速い方が compiled レーンで動かない方になる」ので、
**組み込み `dict` を deprecate して `Dict` に一本化するかは別途決める**
(todo に項目を立てる)。

## 2. `Set<T>`

`Dict<T, ()>` は compiled レーンが拒否する (実測 4)。`Dict<T, bool>` は
1 バイト/エントリの無駄に加えて `insert` の意味が「上書き」になり、
`Set` の `insert -> bool` (新規なら true) と食い違う。

**採用: `core/std/collections/set.t` に独立 struct**。`slots` + `keys`
だけを持ち、値の列を持たない。probe / mixer / 成長閾値は **1.3〜1.5 と
同じ数式**を使い、「同じ入力列で `Dict` と `Set` の反復順が一致する」
ことを 3 レーン一致テストで交差確認する (実装は 2 箇所にあるが、
**振る舞いの同一性はテストが持つ**)。

API: `new` / `insert(v) -> bool` / `contains(v) -> bool` /
`remove(v) -> bool` / `size` / `is_empty` / `clear` / `iter`。
集合演算 (`union` / `intersection`) は要求が出てから。

## 3. `Vec<T>` の拡張

`insert(i, v)` / `remove(i) -> T` / `contains(v) -> bool` /
`index_of(v) -> Option<u64>` / `reverse()` / `sort_by(cmp)`。

- **bound は要らない** (実測 1)。`contains` / `index_of` は `==` を
  そのまま書く。
- **free function 版は書けない。** `fn find<T>(v: &Vec<T>, ..)` は
  `Cannot unify &Vec<T> with &Vec<u64>` + `Method 'get' returned
  unresolved Generic('T')` で落ちる (実測)。**全部 `impl<T> Vec<T>` の
  method として書く**。user が同じものを自分で書けないという意味でも
  あるので、todo に型検査側の項目として残す。
- `sort_by(cmp: fn (T, T) -> bool)` は comparator 引数なので `Ord`
  bound が要らない。`fn (T) -> U` の field は adapters で実証済み。
- `remove(i)` は「順序を保つ shift」。swap-remove が要るなら
  `swap_remove(i)` を別名で足す (`Dict::remove` が黙って swap して
  順序を壊した件を繰り返さない)。

## 4. Deque と PriorityQueue

- **`Deque<T>`** — ring buffer (`ptr` + `head` + `len` + `cap`)。
  `Vec<T>` を内部に持たない: `Vec` は先頭削除を持たないので、
  持っても得が無い。`push_front` / `push_back` / `pop_front` /
  `pop_back` / `get(i)` (論理 index) / `iter`。
- **`PriorityQueue<T: Ord>`** — `Vec<T>` 上の binary heap。
  `impl<T: Ord>` の形は `Vec::sort` で実証済み (STDLIB-ORD-BOUND)。
  `push` / `pop -> Option<T>` / `peek -> Option<T>`。
  **max-heap と min-heap のどちらを既定にするか**だけ決める
  (`Ord` は `lt` しか無いので、逆順は comparator を取る
  `PriorityQueueBy` か、`lt` を反転する wrapper 型になる)。

## 5. Phase 分割

| Phase | 内容 | 受け入れ基準 |
|---|---|---|
| **C0** ✅ | 前提の掃除: (a) ✅ 2026-09-02 generic `==` の missing-`eq` を**型検査で**捕まえる (`E0010`、呼び出し位置)、(b) ✅ 2026-09-02 `Hash for str` を extern 化 + `impl Hash for String`、(c) ✅ 2026-09-02 `hash.t` に `mix()` | (a) は `Bag<Point>` がコンパイルエラーになること。(b) は 3 レーンで同値 (`compiler/tests/consistency/collections.rs` が値ごと pin) |
| **C1** | `Dict` の open addressing (1.1〜1.6) | 既存 dict テストが**意味論不変で** green + 反復順を `docs/language.md` に明記 + 順序の 3 レーン pin + 性能実測 (この文書の表と同じ形で前後比較) |
| **C2** | `Set<T>` | `Dict` と同じ入力列で反復順が一致する交差テスト、3 レーン一致 |
| **C3** | `Vec` 拡張 | method ごとの consistency テスト。`remove` と `swap_remove` の順序差を pin |
| **C4** | `Deque` / `PriorityQueue` | 3 レーン一致。PQ は「同値要素の順序は未規定」を明記 |

C0 の (a) は型検査側の修正なので、C1 と**並行して進められる**
(むしろ C1 の前に入っていないと、`K: Hash` を付けたときの
「bound 違反は E0010、`eq` 欠けは実行時」という非対称が残る)。

## 非目標

- **B-tree / 順序つき map** — `Ord` があれば書けるが、`Dict` の反復順を
  挿入順で保証する以上、「キー順で舐めたい」は `iter().collect()` +
  `sort_by` で足りる。要求が出てから。
- **カスタム hasher の注入 / seed 付き DoS 耐性** — seed の入手経路
  (乱数) が決定性規約と衝突する。`random_seed` で固定できるとはいえ、
  「同じプログラムが毎回同じ順序で反復する」を壊す価値がまだ無い。
- **並行コレクション** — 並行性そのものが未着手 (todo CONCURRENCY)。
- **iterator invalidation の検査** — 反復中に `insert` すると `entries`
  が realloc されうる。現状の `DictIter` も同じ穴を持っている
  (ポインタのコピーを握る)。REGION / move 検査の系で扱う話であって、
  コレクション側で塞ぐものではない。**docs に「反復中の変更は未定義」と
  書く**に留める。
- **小サイズ最適化 (inline storage)** — `Vec` / `Dict` が
  `__builtin_heap_alloc(0)` から始まる今の形を壊す割に、得るものが
  測れていない。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — stdlib 全体の俯瞰。この
  文書は P1 を展開したもの
- [`todo.md`](todo.md) — STDLIB-COLLECTIONS が状態の正本
- [`../docs/language.md`](../docs/language.md) — 反復順・bound 違反の
  診断・決定性規約の正本
- [`RUNTIME_PORT.md`](RUNTIME_PORT.md) — R3/R4 (str ヘルパを toylang 化
  しない判断) が 1.4 の根拠
- [`../core/std/dict.t`](../core/std/dict.t) —
  現行の並列配列 layout と反復子のレジスタ予算の記述
