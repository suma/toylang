# NEVER-ALLOCATES — 静的「確保しない」の設計検討

`ensures allocates(0u64)` は**実行時**に「確保しなかった」を確かめる。
本文書はその静的版 — 「この関数は確保**しえない**」をコンパイル時に
検査する仕組み — の設計検討。**2026-08-21 に実装済み**（自由関数のみ、
メソッドは未対応）。使い方は
[`docs/design_by_contract.md`](../docs/design_by_contract.md)、
文法は [`language.md`](../docs/language.md)。本文書は決定の記録として残す。

実行時版は [`docs/design_by_contract.md`](../docs/design_by_contract.md)
と [`ALLOC_CONTRACT_SUGAR.md`](ALLOC_CONTRACT_SUGAR.md)。同文書の
未解決判断点 3「静的検査版と名前を衝突させない」が本文書の出発点。

---

## 1. キーワード名 — `never_allocates`

他言語を調べた結果、**関数単位の静的検査を実用化しているのは実質 D だけ**
だった。

| 言語 | 綴り | 単位 | 静的/実行時 |
|---|---|---|---|
| **D** | `@nogc` | 関数 | 静的 (テンプレートでは推論、static 変数は例外) |
| **Ada/SPARK** | `pragma Restrictions (No_Allocators)` | **パーティション全体** | 静的 (post-compilation check) |
| **Rust** | 標準には無し | — | — |
| Rust の crate | `#[no_alloc]` (QADAPT)、`assert_no_alloc` | 関数 / スレッド | **実行時** |

名前の選定に効いたのは 3 点:

1. **`@nogc` は "GC" という語に縛られている。** この言語に GC は無いので
   綴りをそのまま借りられない。借りられるのは語形 (`no` + 対象) だけ。
2. **Ada はパーティション単位**で、関数ごとに宣言する形ではない。粒度の
   前例としては使えない。
3. **Rust で `no_alloc` と綴られたものは、調べた限りすべて実行時チェック**
   だった (QADAPT はアロケータに触れたらスレッドを panic、
   `assert_no_alloc` はスレッド単位で確保を一時禁止)。この言語では
   その役割を既に `ensures allocates(0u64)` が担っているので、静的版に
   `no_alloc` を当てると **Rust から来た人の直感と逆**になる。

採用したのは **`never_allocates`**:

- 実行時の `allocates(N)` と語幹を共有し、二つが同じ事柄の静的版/動的版
  だと読める
- `never` が「測った結果 0 だった」ではなく「そもそも起こりえない」を
  表す。これが静的検査と実行時契約の差そのもの
- Rust の `no_alloc` の含意 (実行時) を避けられる

構文位置は前置修飾子。既存の `pub` → `extern` → `fn` に続く形:

```rust
pub never_allocates fn triangle(n: u64) -> u64 { ... }
```

**`requires` / `ensures` の位置には置かない。** 契約は実行時に検査される、
という規約を静的検査と混ぜると、読み手が「いつ検査されるのか」を
綴りから判断できなくなる。

---

## 2. 何を「確保」とみなすか

実行時カウンタと同じ定義 — **現在の allocator に要求すること**。
具体的には `__builtin_heap_alloc` / `__builtin_heap_realloc` への到達。
`__builtin_heap_free` は確保ではないので許す。

「heap」ではなく「allocator」なのが要点で、`with allocator = arena { ... }`
の中の確保も同じく確保として数える (実行時カウンタもそう数える)。

---

## 3. 検査方式 — 属性の伝播か、到達可能性か

### (a) 属性の伝播 (D 流)

`never_allocates` 関数は `never_allocates` 関数しか呼べない。

- 単純で、エラーメッセージが局所的 (「この呼び出しが違反」)
- **stdlib 全体に注釈を付ける作業が発生する。** この言語の stdlib は
  toylang で書かれており (`core/std/*.t`)、`Vec` / `String` / `Dict` の
  どのメソッドが確保しないかを一つずつ宣言することになる

### (b) 呼び出しグラフの到達可能性 ← 推奨

`never_allocates` 関数から到達できる関数の中に heap builtin の呼び出しが
あるか、を探索する。

- **注釈が要らない。** 既存の stdlib に一切手を入れずに機能する
- 前例がある: `Module::reachable_from` が demand-driven lowering で
  同じ探索をしている (ただし IR レベル。こちらは frontend の AST 上で
  行う必要がある)
- 欠点: 「呼ぶ可能性がある」であって「呼ぶ」ではない。
  `if false { alloc() }` も違反になる。保守的な方向なので安全側
- 欠点: エラーが遠い。「`f` が違反」ではなく「`f` → `g` → `h` →
  `__builtin_heap_alloc`」という経路を出す必要がある (出せば (a) より
  親切ですらある)

---

## 4. 静的に追えないもの

| 対象 | 扱い |
|---|---|
| **closure / 関数値の間接呼び出し** | `never_allocates` 関数内で**禁止**。呼び先が実行時に決まるので追えない |
| **`dyn Trait` の動的ディスパッチ** | 同上。禁止 |
| **`extern fn`** | 実装が Rust / C 側にあり追えない。既定は禁止。ただし `io::` 系が丸ごと使えなくなるので、宣言側で「確保しない」と申告できる逃げ道が要る: `extern never_allocates fn getchar() -> i32 from "c"`。これは**信頼ベース**で、検査ではなく宣言 |
| **再帰** | 到達可能性の探索では自然に扱える (訪問済み集合で止める) |
| **generic 関数** | 呼び出しグラフは monomorph 前でも辿れる。ただし型引数によって呼び先が変わる場合 (trait method) は、bound から到達しうる impl をすべて見る必要がある。**保守的に全 impl を見る**のが安全 |

---

## 5. 実行時版との関係

| | `never_allocates` | `ensures allocates(0u64)` |
|---|---|---|
| いつ | コンパイル時 | 実行時 |
| 何を保証 | 確保しえない | その呼び出しでは確保しなかった |
| extern の先 | 保証外 (信頼ベース) | **数える** (カウンタはプロセス全体) |
| コスト | ゼロ | カウンタ読み + 比較 |

両方書けるが冗長。**`extern` を呼ぶ関数では実行時版の方が強い**という
関係になるので、「静的版があれば実行時版は不要」とは言い切れない。

---

## 6. 実装の見積もり

1. lexer / parser: contextual keyword を 1 語、`Function` に bool フィールド
   1 つ (schema bump)
2. 呼び出しグラフの構築: frontend に新パス。関数名 → 呼び出す関数名 の
   マップを AST から作る。`Expr::Call` / `MethodCall` /
   `AssociatedFunctionCall` / `BuiltinCall` を拾う
3. 到達可能性の探索 + 診断 (経路を出す)
4. 間接呼び出し / extern の拒否
5. テスト、docs

バックエンドの変更は**不要** — 検査だけで、生成コードは変わらない。
これは実行時契約の実装 (3 バックエンド + ランタイムヘルパ) より
はるかに軽い。

---

## 7. 未解決

1. ~~**文字列補間・`to_string` が確保するか**~~ — **解決 (2026-08-21、
   MEM-COUNTER-INTERP-DRIFT)**。カウンタが数えるのは「プログラムが
   要求した確保」だけと定義が固まり、`str` を保持するランタイム内部の
   メモリは数えない。したがって **`println("{x}")` を含む関数も
   `never_allocates` になれる**。静的検査が禁止すべきなのは
   `__builtin_heap_alloc` / `__builtin_heap_realloc` への到達のみ
2. **`never_allocates` を最適化に使うか** — CONTRACT-ELISION と同じ発想で、
   「確保しない」と分かっている関数から drop glue を省ける可能性がある。
   検査が入ってから測る話
3. **推論するか** — D はテンプレートで属性を推論する。この言語でも
   「注釈が無い関数の到達可能性を計算しておき、`--api` で表示する」ことは
   できる。宣言を強制せずに情報だけ出す形

### 7-1. 前提だった不一致 (解決済み)

検討中の実測で、`println("n = {n}")` が interpreter で 24 バイト・AOT で
0 バイトと数えられているのを見つけた。カウンタは契約から読めるので、
**同じ契約が engine によって通ったり落ちたりする**状態だった。

2026-08-21 に解消 (`MEM-COUNTER-INTERP-DRIFT`)。カウンタの定義を
「プログラムが要求した確保」に固定し、`str` を保持するためのランタイム
内部確保は数えないようにした。**この定義が固まったことが本機能の前提**
だったので、着手可能になっている。
