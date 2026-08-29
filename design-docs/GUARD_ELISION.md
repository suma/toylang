# GUARD-ELISION — RUNTIME-TRAP の guard を「既に分かっていること」で消す

> 実装: [`compiler_lower/src/contract_facts.rs`](../compiler_lower/src/contract_facts.rs)
> 消費側: `expr_ops.rs` (0 除算 / underflow / `MIN / -1`)、`array_access.rs` (境界)
> 仕様: [`docs/language.md`](../docs/language.md) の「Contracts and traps」以降

## 2 つの供給源

RUNTIME-TRAP (0 除算・u64 underflow・符号付き `MIN / -1`・配列境界) の
guard は、その条件が**既に確かめられている**なら不要になる。事実の出所は 2 つ:

| 出所 | 例 | `--release` |
|---|---|---|
| `requires` (CONTRACT-ELISION) | `requires b != 0u64` → `a / b` の guard | **効かない** |
| 制御フロー | `if b != 0u64 { a / b }` / `for i in 0u64..8u64 { arr[i] }` | **効く** |

`--release` の非対称性が要点:

- `requires` は release で**検査されない**。検査されていない契約を根拠に
  メモリ安全性の検査を外すことはできないので、guard は残る
  (「最適化が checked build で on、unchecked build で off」という
  普通と逆の配置は意図的)。
- **分岐はどちらの build でも評価される**。だから制御フロー由来の事実は
  release でも使える。

## 事実の領域

両者は**同じ領域** (`ContractFacts`) を共有する:

- `nonzero(x)` / `nonneg(x)` / `not_minus_one(x)`
- `below(x, N)` — `x < N` (N は具体値)
- `at_least(a, b)` — `a >= b` (記号間の関係)

読み取りは `close()` で推移閉包を取る (`a >= b` かつ `b >= c` → `a >= c`、
`a >= b` かつ `a < N` → `b < N` など)。

条件式からの読み取りは `requires` と同じ `collect` を通る。制御フロー版で
足したのは 3 つ:

1. **否定** (`negated` フラグ)。`else` 枝は条件を逆に読む
   (`if x == 0u64` の else で `x != 0`)。`!e` と、否定下の `||`
   (De Morgan) も追う。
2. **`==` の読み取り**。`x == 5u64` は値そのものを言うので上限も符号も
   非零性も出る。契約より `if` で書かれることの方が多い。
3. **`for` の範囲**。`..` も `to` も半開 (`i < end` の header に落ちる)
   ので、リテラル終端がそのまま `below` になる。

## 健全性 — 誰について事実を取ってよいか

`requires` の健全性は「**パラメータは immutable**」で担保されていた
(型検査がパラメータへの代入を拒否するので、入口で成り立った事実は
body 全体で成り立つ)。制御フローの条件はスコープ内の任意の名前について
語るので、この根拠が無い。代わりに**逆向き**の規則にした:

> guard 対象のコードがその名前に書きうるなら、事実を取らない。

「書きうる」は意図的に鈍い — 代入、`val` / `var` による再束縛、`&mut`
借用、そして**名前に対する method 呼び出し**すべてを数える (書かない
method でも数える)。closure body は追えないので、そこで**言及された**
名前は全部書かれたものとして扱う。この向きで間違えると最適化を 1 つ
逃すだけ、逆向きで間違えると guard の無い除算になる。

`for` の induction variable は型検査が代入を拒否する (`val` 束縛と同じ)
ので、body での shadowing だけ見ればよい。

事実は分岐 / ループの body の lowering の間だけ有効で、前後で復元する
(`self.facts = saved`)。

## 実測

配列読み 160M 回 (8 要素配列、内側ループ `0u64..8u64`、AOT、
cranelift `speed`):

| | 時間 |
|---|---|
| guard あり | 0.18s |
| guard なし (本変更) | 0.13s |

cranelift は precondition も分岐の含意も見ないので、この guard を自力で
消せない (contract 版の実測 — 除算+減算のループで ~2x、符号付き配列
アクセスで ~2.7x — は `docs/language.md` にある)。

## この先

今の領域は「具体値の上限 + 記号間の `>=`」まで。次に効くのは:

- **記号的な上限** (`below_sym(i, n)`)。`for i in 0u64..n { arr[i] }` の
  `n` がリテラルでないときに効く。ただし配列長はコンパイル時定数なので、
  `n` 自体が定数に解決できる場合しか繋がらない — `Vec` は生ポインタ
  経由で境界検査を持たないため、実需要は測ってから。
- **区間領域への一般化** (`x ∈ [lo, hi]`)。`val n = m + 1u64` のような
  計算結果に事実を付けられるようになる。join (分岐の合流) と
  widening (ループ) が要るので、今の「body の間だけ有効」の枠を出る。
- **契約の静的な充足判定**。呼び出し側の事実で callee の `requires` を
  証明できたら、その検査自体を落とせる (今は定数引数の場合だけ
  COMPILE-TIME-EVAL C4 が違反を警告する)。区間領域が入ってからの話。

着手条件は上と同じで、**実プログラムで測ってから**。
