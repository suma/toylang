# ALLOC-CONTRACT-SUGAR — 設計検討

`ensures allocates(0)` のような糖衣の設計検討。**2026-08-21 に実装済み**
(`allocates` / `retains` / `allocations`、Phase 1 + Phase 2 とも)。
本文書は決定の記録として残す — 何を却下したか、なぜ 1 語に畳まなかったか
は、後から触る人が最初に知りたいことなので。

使い方は [`docs/design_by_contract.md`](../docs/design_by_contract.md)、
文法は [`language.md`](../docs/language.md) の Design by Contract 章。

前提となる既存機能は
[`docs/design_by_contract.md`](../docs/design_by_contract.md) の
アロケーション契約節と `old(...)`（2026-08-21 landing）。

---

## 1. 何を解決するのか

糖衣の動機を「短くしたい」で済ませると設計を誤る。実測すると問題は
3 つあり、**重いのは 2 番目**。

### (a) 長い

```rust
ensures __builtin_cumulative_bytes() - old(__builtin_cumulative_bytes()) <= 256u64
```

81 文字。同じ builtin 名が 2 回出て、差分を取っているという構造が
名前の長さに埋もれる。

### (b) 違反しても数字が出ない ← 本命

```
$ cargo run -q -p interpreter -- leak.t
Contract violation: `ensures` clause #1 of function `outer` evaluated to false (with n = 1, result = 1)
```

**何バイト漏れたのかが出ない。** 契約が false になったことしか分からず、
`--profile=mem` を別に走らせて突き合わせることになる。これは生の bool 式
である限り改善できない — 診断が式の構造を知らないので、
「left/right の実値」を出す `assert_eq` のような扱いができない。

糖衣にして節の**種別**を持たせれば、そこは
`retained 128 bytes, budget was 0` と書ける。**これが糖衣の最大の価値で、
長さの短縮はおまけ**である。

### (c) 意図が式から読み取れない

`live` と `cumulative` は別のことを言うが、式の見た目はほぼ同じ。
読み手は builtin 名の差 1 語から意図を復元しなければならない。

---

## 2. 数える軸

契約から読めるカウンタは 6 つあるが、`old` との差分が意味を持つのは 3 つ。

| 軸 | builtin | 差分の意味 | 答える問い |
|---|---|---|---|
| **retain** | `live_bytes` | 返さなかった量 | 漏らしていないか |
| **request** | `cumulative_bytes` | 要求した総量 | そもそも確保したか |
| **count** | `alloc_count` | 確保回数 | 1 回に抑えているか |

`peak_live_bytes` は単調増加のプロセス全体量なので、差分は
「この呼び出し中に更新した分」という弱い意味しか持たない。
`free_count` / `realloc_count` の差分は request/count の内訳で、
独立した軸ではない。

**この 3 軸は 1 語に畳めない。** `retain` 差分 0 は「確保して解放した」を
含み、`request` 差分 0 だけが「1 バイトも要求していない」。畳むと今の
生の式より意味が曖昧になり、糖衣が嘘をつく。**最低 2 語、素直には 3 語**
必要というのが出発点。

---

## 3. 構文候補

### A. 関数呼び出し風 — `ensures allocates(256u64)`

```rust
fn parse(s: str) -> Node
    ensures allocates(256u64)      # request 差分 <= 256
    ensures retains(0u64)          # live 差分 <= 0
```

- 既存の式構文に収まる。parser が `old(...)` と同じ流儀で desugar できる
- `ensures allocates(0u64) && result > 0u64` のように他の節と混ぜられる
- `allocates` は現在ユーザ関数名として使える（`fn allocates(n: u64)` は
  通る）ので、`old` と同じく **contextual keyword** にする必要がある

### B. 演算子付き — `ensures allocates <= 256u64`

- 英語としては最も読みやすい
- しかし `allocates` が値のように見えて値ではない（式の途中に書けない、
  `val x = allocates` は無意味）。パーサの特別扱いが構文の見た目に
  現れないので、**書ける形と書けない形の境界が読み手に見えない**
- 却下

### C. 独立した節 — `allocates 0`

```rust
fn parse(s: str) -> Node
    requires s != ""
    allocates 256u64
    retains 0u64
```

- `requires` / `ensures` と並び、「事後条件の特殊形」であることが構文で分かる
- 診断の種別を持たせるのが自然（節の種類がそのまま種別）
- 代償: キーワードが 2〜3 個増え、AST の `Function` にフィールドが増える
  （schema bump）。`ensures` との組み合わせ（AND）の規則も要る

### D. stdlib 関数で済ませる — `ensures alloc::delta() <= 256u64`

- **不可能**。「入口からの差分」は関数では表現できない（`old` が要る）。
  `old(alloc::cumulative())` と書けるが、それは今と同じ長さ

---

## 4. 推奨

**A（関数風）を採り、名前は 3 つ。**

| 糖衣 | 展開 |
|---|---|
| `allocates(N)` | `__builtin_cumulative_bytes() - old(__builtin_cumulative_bytes()) <= N` |
| `retains(N)` | `__builtin_live_bytes() - old(__builtin_live_bytes()) <= N` |
| `allocations(N)` | `__builtin_alloc_count() - old(__builtin_alloc_count()) <= N` |

理由:

- **C より A**。節を増やすと `requires` / `ensures` の 2 語で完結していた
  契約構文が 5 語になり、`ensures` の中に書けば済むものを構文レベルに
  持ち上げることになる。診断の種別は AST の別経路（後述 Phase 2）で
  持てるので、構文を増やす必要はない
- **`<=` 固定**。`allocates(0)` は `<= 0` と `== 0` が一致する。N > 0 で
  厳密一致を要求したい状況は考えにくく、上限を課すのが実際の用途

**誤用リスクと対策**: `allocates(0)` と書いて「漏らさない」を意図する
読み手は必ず出る（語感として `allocates` が先に浮かぶ）。対策は
(1) 診断が `requested` / `retained` と語を分けること、
(2) ドキュメントで必ず 2 つ並べて対比すること。

---

## 5. 実装方式

### Phase 1 — parser desugar（小、AST 変更なし）

`old(...)` と同じ場所（`parse_primary_after_identifier`）で
`in_ensures_clause` のときだけ `allocates` / `retains` / `allocations` を
拾い、`BinOp::Le` の式木に展開する。展開後の `old(...)` 相当部分は
既存の `old_exprs` 機構にそのまま乗る。

- AST 変更なし → `FULL_AST_CACHE_SCHEMA_VERSION` の bump 不要
- 3 バックエンドは通常の bool 式として扱う → **バックエンド側の作業ゼロ**
- 型検査も既存経路（`u64` 同士の比較）

**注意**: 展開に `u64` の減算が入るので RUNTIME-TRAP のアンダーフロー
ガードが契約式の中に emit される。カウンタは単調増加なので発火しないが、
契約評価のコストとして 1 比較 + 1 分岐が乗る。契約を切れば消える。

### Phase 2 — 種別付き診断（中、schema bump あり）

`Function.ensures: Vec<ExprRef>` を `Vec<EnsuresClause>` にし、
`Plain(ExprRef)` / `Alloc { kind, budget, expr }` を持たせる。違反時に
kind に応じて実測値を出す:

```
Contract violation: `retains` budget exceeded in `outer`
  retained: 128 bytes
  budget:   0 bytes
```

- AST 変更 + schema bump
- 契約評価が 3 箇所（tree-walker / lowering / interpreter JIT の
  fallback 判定）にあるので、そこを種別対応にする
- lowering 側は `Terminator::Panic { message }` が**静的な interned 文字列
  しか運べない**ので、実測値を含むメッセージは今の仕組みでは出せない。
  AOT で同じ診断を出すには、`assert_eq` と同じく実行時に文字列を組み立てる
  ランタイムヘルパ経由にする必要がある（`toy_panic_alloc_budget(actual,
  budget)` のような形）

**Phase 1 だけを入れる価値は薄い。** 81 文字が 20 文字になるだけで、
問題 (b) は残る。着手するなら Phase 2 まで見込むこと。

---

## 6. 既存機能との相互作用

すべて実測で確認済み。

| 相手 | 挙動 | 設計上の含意 |
|---|---|---|
| **呼び出し先の確保** | カウンタはプロセス全体なので、`outer` の契約は `inner` の確保も捕まえる | 「確保しない」が**推移的**に効く。静的検査版では自前で呼び出しグラフを辿る必要があるのに対し、動的カウンタは無料でこれを得ている — 糖衣がカウンタ方式に乗る強い理由 |
| **`with allocator = arena`** | scope 内の `__builtin_heap_alloc` も数える | 「arena に取ったから 0」にはならない。arena を使う関数は `retains` ではなく `allocations` で縛るのが自然 |
| **`--profile=mem`** | 同じ数字（request 単位） | 違反時に `--profile=mem` で裏を取れる。診断の文言も同じ語を使うべき |
| **contracts off / `--release`** | 契約ごと消える | カウンタ読み出しのコストも消える |
| **CONTRACT-ELISION** | 無関係（`requires` 側の機構） | 相互作用なし |
| **`--check`** | `ensures` はオラクルなので、アロケーション契約も自動でプロパティテストされる | ただし生成できる引数型は 4 つ（bool/i64/u64/f64）に限られる |

---

## 7. 未解決の判断点

1. **語の選定** — `allocates` / `retains` / `allocations` で確定か。
   `allocations` だけ品詞が違う（回数）ので、`allocates_at_most` /
   `allocation_count` のような別案もある
2. **`requires` に書けるか** — 入口では差分が必ず 0 なので常に真。
   書けても無意味なので `ensures` 限定にすべきだが、その診断が要る
   （`old(...)` と同じ扱い）
3. **静的検査版との名前衝突** — 将来「heap builtin を呼ぶ関数を推移的に
   禁止する」検査を入れるなら、`allocates(0)` と同じ綴りを使ってはいけない
   （実行時契約と静的検査は別物）。`no_alloc` 修飾子など別の綴りを
   予約しておくか、今決めておく
4. **Phase 2 の AOT 診断** — `Terminator::Panic` が静的文字列しか運べない
   制約をどう回すか。`assert_eq` の前例（ランタイムヘルパ）に倣うのが素直
5. **`retains` の負値** — 関数が入口より live を**減らした**場合
   （引数で渡されたポインタを解放する関数など）、u64 減算が
   アンダーフローする。実測:

   ```
   ensures __builtin_live_bytes() - old(__builtin_live_bytes()) <= 0u64
   → panic: u64 subtraction underflowed
   ```

   展開を `live_bytes() <= old(live_bytes()) + N` にすると同じ意味で
   通る（実測で確認）。**Phase 1 の展開式はこちらを採る。**

> 5 は実装前に気づけてよかった類の落とし穴で、糖衣の展開式を
> 「差分 <= N」ではなく「現在値 <= 入口値 + N」と書くだけで消える。
> 生の式を手で書いている限り、同じ罠を各自が踏むことになる —
> 糖衣を入れる理由がもう一つ増えた。

---

## 8. 実装の結果 (2026-08-21)

着手条件は「生の式を何度も書いてから」としていたが、判断点 5
(アンダーフロー) が**生の式を書く人全員が踏む罠**だと分かった時点で、
頻度と無関係に優先度が上がったので実装した。

決定どおりに入ったもの:

- 構文は案 A (関数風)、語は `allocates` / `retains` / `allocations`
- 展開は `counter() <= old(counter()) + N` (判断点 5 の安全な形)
- Phase 1 (desugar) と Phase 2 (種別付き診断) を両方
- 診断は 3 バックエンド同文言
  (`retained 128 bytes, budget 0 bytes`)

検討時に見えていなかった実装上の要点:

- **`Terminator::PanicAllocBudget` を新設した** — 判断点 4 で予想した
  とおり `Terminator::Panic` は静的文字列しか運べないので、読み取り値
  3 つ (entry / current / limit) を運ぶ終端子を足し、`toylang_rt` の
  `toy_panic_alloc_budget` が整形して exit する。JIT にはシンボル登録が
  別途要る (`compiler/src/jit.rs`)
- **文言が 2 箇所に増えた** — `compiler_ir::format_alloc_budget_violation`
  (interpreter / IR VM) と `toylang_rt` の複製 (no_std で依存を持てない)。
  `frontend::format_spec` と `toylang_rt` の `Spec` と同じ関係で、
  drift は `consistency.rs` の
  `an_allocation_budget_reports_the_same_numbers_on_every_backend` が
  stderr を突き合わせて検出する
- **複合述語は budget 節にしない** — `ensures retains(0u64) && result > 0u64`
  は「budget を含む普通の節」であって budget 節ではない。parser が
  「糖衣の展開結果が節の根そのものか」を見て判定する

残っている判断点: 3 (静的検査版との名前衝突) は未決のまま。
将来 `no_alloc` 相当を入れるなら別の綴りを使うこと。

関連: [`todo.md`](todo.md) の完了済み節。
