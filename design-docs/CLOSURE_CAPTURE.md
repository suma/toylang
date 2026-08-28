# CLOSURE_CAPTURE.md — closure が外側の束縛をどう掴むかを決める

`ALLOCATOR_PLAN.md` / `MEMORY_PROFILING.md` / `DEBUG_OBSERVABILITY.md` と
同じ手順 (現状調査 → 論点決定 → Phase 分割 → MVP 刻みで landing) で、
closure の capture 意味論を設計する。

**きっかけは機能の不足ではなく、黙った誤答である。** 「カウンタを閉じ込めて
更新する」という closure の基本形が、今日 **5 実行系のどれでも正しく動かず、
かつ 4 つは何も言わずに間違った答えを返す** (実測 1)。todo.md が
CLOSURE-CAPTURE を ★★ で「closure の利用が増える前に決めたい」と書いていたのは
この状態を指している。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **E0** | 目標文言の固定 + 5 レーンを pin する consistency テスト | ✅ 2026-08-28 |
| **E1** | 捕捉した束縛への代入を診断する (黙った誤答を止める) | ✅ 2026-08-28 |
| **E2** | capture mode の形依存を解消する (scalar コピー / compound alias) | ✅ 2026-08-28 |
| **E3** | escape しない closure の捕捉を共有にする | ✅ 2026-08-28 |
| **E4** | HOF / escape する closure の可変捕捉 | 未着手 |
| **E5** | compiled レーンの compound capture と診断の統一 | 🟡 診断のみ 2026-08-28 |
| **E6** | docs (`docs/language.md` の Captures 節) | ✅ 2026-08-28 (E3 と同時) |

---

## なぜ設計文書が要るか

理由は 2 つあり、どちらも「先に実装すると戻れない」種類である。

1. **capture mode は言語の性格を決める判断**である。スナップショットか、
   参照捕捉か、明示 capture list か。後から変えると、それまでに書かれた
   closure の意味が変わる。今は `interpreter/example/` に closure を使う
   プログラムが 5 本しかないので、決めるなら今が最も安い。
2. **同じ意味論を 5 実行系が独立に実装している**。capture は
   tree-walker (`Object::Closure { captures }`)、compiled 側
   (`MakeClosure` + env slot)、interpreter JIT の 3 実装があり、
   実測 1 のとおり**既に食い違っている**。片方だけ直すと、
   数値が合わない状態が残る。

---

## 現状調査 (2026-08-28 実測)

プローブは `fn() -> u64 { count = count + 1u64  count }` を軸に 17 本。
レーンは IR VM (既定) / tree-walker / interpreter JIT / compiler JIT / AOT。
tree-walker は `INTERPRETER_CONTRACTS=pre` で IR VM を `NotEligible` に
落として観測した (半端な契約設定は IR で表現できないので tree-walker に回る)。

### 実測 1: 捕捉した `var` への代入は、どの実行系も正しくない

```rust
fn main() -> u64 {
    var count: u64 = 0u64
    val bump = fn() -> u64 { count = count + 1u64  count }
    println(bump())   # 期待 1
    println(bump())   # 期待 2
    println(count)    # 期待 2
    0u64
}
```

| 実行系 | 出力 | 何が起きているか |
|---|---|---|
| IR VM | `1` / `1` / `0` | env にコピーした値を書き換えて捨てる |
| interpreter JIT | `1` / `1` / `0` | 同上 |
| compiler JIT | `1` / `1` / `0` | 同上 |
| AOT | `1` / `1` / `0` | 同上 |
| tree-walker | **実行時エラー** | `Cannot assign to immutable variable: Variable count already defined as immutable (val)` |

**正しい実行系が 1 つも無い。** しかも 2 通りの間違え方をしている:

- compiled 系 4 つは**黙って誤答する**。型検査も通り、警告も出ず、
  `--all-backends` は「all 3 backends agree」と言う (比較対象に
  tree-walker が居ないため)。
- tree-walker は実行時に**嘘の理由で**止まる。`count` は `var` で宣言されて
  いるのに「immutable (val) として定義済み」と言う。これは capture を
  `Environment::set_val` で束縛している (`interpreter/src/evaluation/call.rs:1025`)
  ことの副作用で、ユーザの書いた `var` / `val` とは無関係である。

### 実測 2: pin する仕組みは既にあり、この場合を pin していないだけ

`assert_consistent` は tree-walker を第 1 レーンに持つので、実測 1 の
プログラムを渡すと**落ちる** (一時テストで確認: `interpreter execute
(checked program): "Cannot assign to immutable variable: ..."`)。
つまり検査機構の不足ではなく、**capture への書き込みが 1 件も pin されて
いない**。`compiler/tests/consistency/dicts_closures.rs` の closure テストは
Phase 5a/5b/6b/6c の**読み取り**だけを覆っている。

### 実測 3: capture mode が値の形で変わる (scalar はコピー、compound は alias)

```rust
struct P { x: i64 }
var p = P { x: 1i64 }
val f = fn() -> i64 { p.x = p.x + 1i64  p.x }
println(f())     # 2
println(p.x)     # 2 ← 外側が書き換わっている
```

interpreter の 3 レーンは `2` / `2` を出す。**scalar の書き込みは消えるのに、
compound のフィールド書き込みは外へ通る。** 意図的な実装ではある
(`evaluate_closure_literal` の doc comment が「primitives はフレッシュな
セル、compound は既存の Rc セルを保つ」と書いている) が、**ユーザから見ると
「捕捉した値を書き換えられるかどうかが値の形で決まる」**という規則になる。
`val b = a` が compound で alias になるのと同根だが、あちらは `b` を通した
書き込みが見えるのが自然なのに対し、こちらは「消える / 消えない」の差になる。

なお compiler JIT / AOT はこのプログラムをそもそもコンパイルできない
(実測 5)。**「形で意味が変わる」上に「形でコンパイルできるかも変わる」。**

### 実測 4: 型検査は捕捉した `val` への代入だけを捕まえる

```rust
val n: u64 = 1u64
val f = fn() -> u64 { n = n + 1u64  n }
```

これは 5 レーンすべてが同じ型エラーで止まる:

```
[E0010] cannot assign to `n`: binding is immutable
        (declared with `val`; use `var` to allow reassignment)
```

**つまり型検査器は closure 本体の中の代入を見ており、外側の束縛も解決できて
いる。** 見ていないのは「その束縛が closure の外にある」という一点だけで、
`var` なら通してしまう。`record_closure_captures`
(`frontend/src/type_checker/expression.rs:1504`) が既に free variable を
列挙し、`collect_closure_free_vars` は `Expr::Assign(Identifier, _)` も
辿っている。**E1 に必要な足場は揃っている。**

### 実測 5: compiled レーンは compound を捕捉できず、診断が形で変わる

| 捕捉する値 | compiler JIT / AOT の言うこと |
|---|---|
| `str` | `compiler MVP: capturing closure can only capture primitive scalars; \`s\` has type Str` |
| struct | **`undefined identifier \`p\`**` |
| `Box<i64>` | `compiler MVP needs an explicit type annotation to instantiate generic struct \`Box\`` |

`str` は名指しの MVP メッセージだが、struct は**capture の話だと分からない
メッセージ**になる。原因は `lift_closure_binding` の capture ループが
`bindings.get(cap_name)` に `Binding::Scalar` を期待し、compound 束縛は
そこまで到達せず body の lowering 側で「未定義の識別子」として落ちること。
interpreter は 3 レーンとも動くので、**同じプログラムが「動く / 意味不明な
エラー」に割れる**。

### 実測 6: 読み取りだけなら 5 レーン一致で、docs どおり

- 捕捉した値の読み (`x + base`)、生成後に外側を書き換える
  (`n = 99u64` の後に closure が古い値を見る)、ループ内生成、
  シャドウイング、nested closure、HOF 経由 — **すべて 5 レーン一致**。
- `docs/language.md:1861` の「Free variables in the body are captured at
  closure-creation time」は、**読み取りについては正しい**。書き込みに
  ついては何も書いていない。

### 実測 7: escape する closure は今日動く

```rust
fn make() -> fn () -> u64 {
    var n: u64 = 41u64
    n = n + 1u64
    fn() -> u64 { n }
}
```

5 レーンとも `42`。**スナップショットだからこそ成り立っている** —
参照捕捉に切り替えると、この形は「死んだスタックフレームのローカルを指す」
ことになる。論点 2 が扱う。

### 参考: 既にある足場

- `record_closure_captures` / `collect_closure_free_vars` (型検査) —
  free variable の列挙と、代入位置の識別子の走査。
- `Object::Closure { captures: Vec<(Symbol, RcObject)> }` (tree-walker) —
  capture ごとに `RcObject`。compound は Rc 共有、scalar は新しいセル。
- `MakeClosure { target, captures, capture_tys }` + env slot (compiled) —
  生成時に値を env に**コピー**し、body 入口で env から自分のローカルへ
  ロードする。**書き戻す経路は無い。**
- `CallWithSelfWriteback` (compiled) — `&mut self` メソッドが呼び出し後に
  レジスタを書き戻す既存の仕組み。E3 が下敷きにできる。
- `assert_consistent` の tree-walker レーン (実測 2)。

---

## 論点と決定

### 論点 1: capture mode をどうするか

| 案 | 内容 | 代償 |
|---|---|---|
| **A** | 現状維持 + 捕捉した束縛への代入を**型エラー**にする | カウンタは書けないまま。ただし黙った誤答は消える |
| **B** | **参照捕捉** — 捕捉した `var` への書き込みが外に通る | escape の寿命問題 (論点 2)、compiled 側は env にアドレスか writeback が要る |
| **C** | **明示 capture list** (`fn[&mut count]() -> u64 { ... }`) | 構文追加。書く側の負担。判断を毎回書かせる |
| **D** | 形に依らず全部 alias (scalar も Rc セル共有) | tree-walker は簡単だが compiled 側のコストは B と同じ。かつ「読むだけ」の closure まで参照コストを払う |

**決定 (案): A を先に landing し、その上で B を既定にする。C は採らない。**

- A は**単独で価値がある** (実測 1 の黙った誤答が止まる) 上に、B の前提でも
  ある。B が扱えない形 (escape、HOF 越え) は結局 A の診断に落ちるので、
  A で書く診断は捨てにならない。
- C を採らない理由は言語の既存の性格に合わないため。この言語は `&self` /
  `&mut self` を**書かせる**が、capture については `val` / `var` という
  既存の宣言が既に意図を持っている。`var` を捕捉して書けば可変捕捉、
  `val` なら読み取り — 追加構文なしで同じ情報が取れる。
- D を採らない理由は、**読むだけの closure に代価を払わせる**から。
  実測 6 のとおり読み取りは 5 レーン一致で正しく動いており、
  ここを触る理由が無い。

### 論点 2: escape する closure の寿命をどうするか

参照捕捉は「捕捉した束縛が生きている間だけ」健全である。実測 7 の
`make()` は closure を返すので、参照捕捉にすると死んだフレームを指す。

| 案 | 内容 |
|---|---|
| **箱にする** | 可変捕捉された束縛をヒープセルに移す (JS / Python 方式) |
| **escape したら値捕捉に戻す** | escape する closure では書き込みを型エラーにする |

**決定 (案): 後者。** この言語は `never_allocates` 契約とアロケーション
カウンタを持ち、**確保が見えることを機能にしている**。closure を書いただけで
黙ってヒープを掴む方式はその性格に反する (`never_allocates fn` の中で
closure が書けなくなる)。

escape の判定は保守的な構文規則で足りる: **`fn` の戻り値になる / struct
フィールド・配列・dict・enum payload に入る closure は escape**、
`val` に束縛してその関数の中で呼ぶだけなら escape しない。判定に迷う形は
escape 側に倒す (診断が出るだけで、誤答にはならない)。

### 論点 3: compiled 側で可変捕捉をどう表現するか

env は生成時のコピーで、書き戻す経路が無い (実測 5 の参考)。2 通りある。

| 案 | 内容 | 効く範囲 |
|---|---|---|
| **sync-in / sync-out** | 呼び出しの直前に外側ローカル → env、直後に env → 外側ローカル | 呼び出し位置から外側ローカルが見える場合のみ = **直接呼び出し** |
| **アドレス捕捉** | env にローカルのアドレスを入れる (cranelift の explicit stack slot) | HOF 越しでも効く |

**実装したのはアドレス捕捉の方** (草案は sync-in / sync-out を先に置いていた)。
理由は 2 つ。(a) **足場が既にあった** — `AddressOf` +
`address_taken_locals` + `Binding::RefScalar` (`LoadRef` / `StoreRef`) が
REF-Stage-2 の `&mut` 引数のために揃っていて、closure の env スロットに
値の代わりにアドレスを入れ、body 側で `RefScalar` に束縛するだけで
読み書きが通る。(b) **論点 7 で読みも live にしたので sync-in / sync-out
では足りない** — 呼び出しの前後で同期する方式は「呼び出し中に外から
書き換わらない」ことに依存しており、読みが live という約束を
表現できない。アドレスなら読みも書きも定義どおりになる。

HOF 越し (E4) が残るのは lowering の都合ではなく**寿命の判断**の方
(論点 2) で、そこは今も診断で拒否する。

### 論点 4: 形依存 (実測 3) をどちらに寄せるか

compound のフィールド書き込みが外に通るのは、**A の下では偶然の一貫性違反**に
なる (scalar は診断で止まるのに compound は通る)。B が landing すれば
「どちらも通る」で揃うので、E2 の仕事は**「E1 の診断を compound にも同じ
規則で当てる」**ことになる。つまり `p.x = ...` も「捕捉した束縛への書き込み」
として扱う。

**移行の代償を明示しておく**: 実測 3 のプログラムは今日 interpreter で
動いており、E1 で一度**エラーになる**。ただしこれは compiler JIT / AOT で
コンパイルできないプログラムなので、**移植可能なコードは 1 行も失われない**。

### 論点 7: 共有 closure の**読み**も live にするか (E3 で追加)

書き込みを外へ通すと決めた時点で、読みをどうするかが別の判断として残る。

| 案 | 内容 |
|---|---|
| **書く capture だけ live** | 代入する capture だけ共有、読むだけの capture は従来どおり snapshot |
| **escape しないなら全部 live** | 共有 closure の capture は読み書きとも外側と同じ束縛 |

**決定: 後者** (2026-08-28、利用者判断)。前者は同じ関数の中で
「`f` は 1 を返し `g` は進む」が並ぶことになり、**closure が何をするかで
読みの意味が変わる**。後者なら規則は「escape するかどうか」の 1 本になる。

代償は既存の pin と docs を書き換えたこと: `add_n` の例が
**42 → 132** になった (`closure_phase6_capture_snapshot_...` /
`closure_capture_snapshot_...` の 2 件と `docs/language.md` の
Captures 節)。どちらも「snapshot が仕様である」と書いていたので、
文言ごと差し替えた。**escape する closure は今も snapshot** なので、
同じ `add_n` を `run(add_n, 32i64)` に渡す形は 42 のまま — その対比を
docs と example に両方載せてある。

### 論点 5: 何を先に landing するか

**黙った誤答を止めるのが最優先**で、機能追加はその後。順序は
E0 (pin) → E1 (診断) → E2 (形依存) → E3 (直接呼び出しの可変捕捉) →
E4 (HOF / escape) → E5 (compound capture) → E6 (docs)。

E1 まで入れば「間違った答えを返すプログラムは書けない」状態になる。
E3 まで入れば「カウンタを閉じ込めて更新する」が書ける。

### 論点 6: 診断コードを新設するか

新設する。既存の `E0010` (immutable への代入) は理由が違う —
ユーザが `var` と書いているのに拒否するので、`E0010` の文言
(「`var` を使え」) をそのまま出すと**直しようのない指示**になる。
新コードは「なぜ書けないか」と「今どう書くか」を言う。

---

## Phase 分割

### E0 — 文言の固定と比較レーン

capture の書き込みについて、**何が起きるべきか**を先に表にして、5 レーンを
突き合わせるテストを置く。DEBUG-OBS D0 と同じ形 (目標を決めてから実装する)。

- `compiler/tests/consistency/dicts_closures.rs` に capture **書き込み**の
  テスト群を新設 (現状は読み取りだけ)。E1 が landing するまでは
  `#[ignore]` ではなく**現状を pin する** — 実測 1 の食い違いはテストとして
  赤である方が正しいので、E0 は「E1 の期待値」を書いて E1 と同時に緑にする。
- 目標表 (E1 後):

  | プログラム | 期待 |
  |---|---|
  | 捕捉した `var` への代入 (直接呼び出し) | E3 まで: `E0021` / E3 以降: 動く |
  | 捕捉した `val` への代入 | **`E0021`** (草案の `E0010` 現状維持から変更、下記) |
  | 捕捉した compound のフィールド代入 | 捕捉した `var` と同じ扱い (E2) |
  | 読み取りのみ | 5 レーン一致 (現状維持) |

  **草案から 1 つ変えた**: 捕捉した `val` への代入は `E0010` を維持する
  つもりだったが、`E0010` の助言 (「`var` を使え」) は capture の下では
  **行き止まり**になる — `var` に直しても `E0021` で拒否されるので、
  1 つのエラーを別のエラーに直させることになる。規則は「束縛がどこに
  あるか」であって「どう宣言されたか」ではないので、`val` / `var` を
  同じコードで拒否する。

### E1 — 捕捉した束縛への代入を診断する ✅ (2026-08-28)

- `TypeCheckContext::closure_scope_floors` — closure 本体を検査する間、
  その closure の**パラメータが載っているスコープの index** を積む。
  本体は enclosing スコープの上で検査される (capture の型を引けるのは
  そのおかげ) ので、**深さだけが local と capture を分ける**。
  `is_captured_binding` が名前を解決したフレームの index と床を比べる。
- 新コード **`E0021`** + `--explain`。caret は代入対象に付ける
  (`error_with_location` に lhs を渡す。付けないと文の recovery が
  ブロックの末尾式に打つ)。
- 既存の `val` 規則より**先**に判定する (上表の理由)。
- 5 レーンとも同じ診断で止まる (検査が共有 frontend にあるため、
  エンジンごとにずれる余地が構造的に無い)。
- tree-walker の `set_val` 由来の嘘メッセージ
  (`already defined as immutable (val)`) はこの経路が型検査で止まるので
  到達しなくなった。E3 で capture の束縛方法を変えるときに消す。
- テスト: `frontend/tests/closure_type_checking_tests.rs` に 7 件
  (counter / 捕捉した `val` / パラメータ / closure ローカルの `var` /
  読み取り / nested の床 / 床が closure より長生きしないこと)、
  `compiler/tests/consistency/dicts_closures.rs` に 2 件。

### E2 — 形依存の解消 ✅ (2026-08-28)

- `captured_assign_target` — 代入先の式を根まで辿り
  (`FieldAccess` / `TupleAccess` / `SliceAccess`)、根が capture なら
  `(書かれた形, 捕捉された束縛)` を返す。`p.x` / `o.inner.v` / `a[..]` が
  同じ規則で `E0021` になる。**規則は根について**で、書かれた path は
  文言が引用するためだけに組み立てる。
- 文言は 2 通り持つ。**理由が違うため** — bare な再束縛は捨てられる
  (snapshot)、path 越しの書き込みは 3 レーンで外に届いていた。
  「どちらも同じ理由」と書くと嘘になる。
- **`a[i] = v` は `Assign` ではなく `SliceAssign` という別の式**なので
  `visit_slice_assign_impl` にも同じ判定が要る。これが無いと
  バックエンドまで到達し、**全レーンで内部エラー**
  (`Expr::Number should be transformed to concrete type`) になっていた —
  closure の外なら同じ書き込みが普通に動くのに。
  index 式を caret の anchor に使う (添字される識別子は位置を持たないので
  そのままだとブロックの末尾式に打たれる)。
- `evaluate_closure_literal` の「primitive は新セル / compound は Rc 共有」
  という分岐は、**capture が読み取り専用である限り観測できない**ので
  E3 まではそのまま。E2 が閉じたのは**診断の穴**。
- 移行の代償は 0 だった: 全 2348 テストが緑のまま (stdlib も example も
  capture 越しに書いていない)。
- 実測 5 の「compound を捕捉すると `undefined identifier`」は**読み取りにも
  当たる**ので、compiled レーンでの compound capture は E5 のまま。

### E3 — escape しない closure の捕捉を共有にする ✅ (2026-08-28)

- **escape 判定** (`frontend/src/type_checker/closure_escape.rs`):
  `val NAME = fn(...)` でこの関数の本体に束縛され、`NAME` の言及が
  **すべて同じ入れ子での直接呼び出し**なら共有。callee は
  `Expr::Call(symbol, args)` で symbol なので、**`Identifier(NAME)` が
  1 つでも現れたら値として使われている** = escape。closure の中からの
  呼び出しも escape (その closure 自体がフレームより長生きしうる)。
  同名の 2 回束縛も escape。
- **答えの置き場所は `Expr::Closure::captures_by_ref`** (AST)。5 実行系
  すべてが要るうえ、**capture 走査の独立実装が 3 つあることが
  そもそも今回のずれの原因**なので、解析まで 3 重にはしない。
  型検査器は同じ 1 回の走査で body ref の集合も返し、自分はそれを使う
  (visitor は closure node ではなく body を渡されるため)。
- **tree-walker**: 共有 closure は **capture を 1 つも snapshot しない**。
  shadow しなければ名前は外へ解決し、`Environment::set_var` は
  スコープスタックを遡って**その束縛のフレームに書く**。
  ただしこれだけでは**動的スコープになる**: closure literal と呼び出しの
  間で同名を宣言すると、その束縛が body を捕まえる。実測したところ
  tree-walker が 199、コンパイル側 (外側ローカルのアドレスを持つので
  正しい) が 101 に割れた。closure に**生成時のスコープ深さ**を持たせ、
  呼び出しの間だけそれより上のスコープを退避する
  (`detach_scopes_above` / `restore_scopes`) ことで lexical に戻した —
  「closure は書かれた場所のスコープで走る」という定義そのもの。
  `compiler/tests/consistency/dicts_closures.rs` に読み / 書き 2 件で pin。
- **compiled (IR VM / compiler JIT / AOT)**: env スロットに値ではなく
  **`AddressOf(outer_local)`** を入れ (`address_taken_locals` に登録)、
  body 側は `Binding::RefScalar { pointee_ty, is_mut: true }` で束縛する。
  読み書きが `LoadRef` / `StoreRef` になり、外側のローカルが**そのまま
  記憶域**になる。`value_scalar` が `RefScalar` を pointee 型として
  答えるようにする必要があった (narrow int の `as` cast が落ちた)。
- **`.toycache` の schema version を 24 に**。古いキャッシュは
  `captures_by_ref = false` として読めるので、失うのは共有であって
  誤って共有することはない。
- 例: `interpreter/example/closure_counter.t` (`example_consistency`
  が自動で 3 バックエンド突き合わせに載せる)。

**landing 後に実測で 2 件出た** (どちらもプローブを全レーンで流し直して
見つけた。片方はテストが緑のままだった)。

1. **共有 capture の中で作った closure が compiled で落ちた** —
   共有 capture は body 側で `RefScalar` になるので、その中の closure が
   同じ名前を捕捉しようとしたとき capture 走査が `Scalar` しか見ておらず
   黙って落とし、body が `undefined identifier` になった。`RefScalar` を
   pointee 型で記録し、内側も共有ならポインタをそのまま渡し、コピーなら
   `LoadRef` で現在値を読むようにした。
2. **添字代入の添字リテラルが `Number` のままだった** —
   `visit_slice_assign_impl` が **dict の枝でしか添字を visit していない**
   ので、`a[0] = v` の `0` は誰にも見られず placeholder のまま
   バックエンドに届いていた。関数直下では既定値に倒れて生き延びていたが、
   closure 本体では内部エラーになる。E2 の診断がこれを隠していて、E3 で
   共有 closure が通るようになった瞬間に露出した。添字を visit するよう
   直したところ、**TREE-WALKER-NUM-W (未実装節にあった別項目) も同時に
   解消**した — 両方向 pin の `assert_consistent_without_tree_walker` が
   「tree-walker が通るようになったので除外を外せ」と落ちて教えてくれた。

### E4 — HOF / escape 越しの可変捕捉

- env にアドレスを持たせる。cranelift 側で対象ローカルを explicit stack
  slot に落とす判断が要る。
- escape する closure は**この Phase でも拒否のまま** (論点 2 の決定)。
  E4 が広げるのは「別の関数に渡した closure が捕捉を書き換える」形だけ。
- **着手条件**: 実プログラムで踏んでから。E3 で書ける範囲がどれだけ実用に
  足りるかを見てから決める。

### E5 — compiled レーンの compound capture 🟡 (最低ラインのみ 2026-08-28)

- **診断は landing 済み。** `walk_closure_for_captures` の `record` は
  Scalar 以外の束縛を**黙って捨てて**おり、body の lowering がその名前を
  「未定義の識別子」として落としていた (実測 5)。capture 集合を
  `Option<Type>` にして、compound を `None` で記録し、
  `collect_closure_captures` が名前つきで拒否する。
- **残り**: env に compound を載せる (leaf 展開)。`AOT-COMPOUND-PTR-RW` と
  同じ手が使えるか実装時に確認する。interpreter は読み取りなら動くので、
  塞がっているのは compiled レーンのカバレッジだけ。

### E6 — docs ✅ (E3 と同時 2026-08-28、E5 診断ぶんを追記)

- `docs/language.md` の Captures 節に**書き込みの規則**を書いた
  (E3 まで読み取りしか書いていなかった、実測 6)。共有 / コピーの
  両方に走る例を置き、コピーへの書き込みが `E0021` であることを示す。
- `CLAUDE.md` の closure の行を更新した。
- あとから 3 点を追記した (E3 の landing 後に landing した挙動):
  - 共有 capture は**書かれたスコープで解決する** (literal と呼び出しの
    間で同名を宣言しても持っていかれない) — `72d6156` の修正。
  - **compound capture はバックエンドで差がある** — interpreter は任意の
    shape を捕捉でき、compiled レーンは scalar だけで、それ以外は
    名前つきで拒否する (E5 の診断そのまま)。
  - 共有 closure の中で書いた closure も動く (共有ならポインタを渡し、
    コピーなら `LoadRef` で読む)。
- ついでに正本側の食い違いを 2 つ直した: 診断コードの範囲が
  `E0001`…`E0020` のままだったのと、複合代入の節が算術 5 種しか
  無いこと (ビット系 5 種は parse error) と LHS の 4 形を
  書いていなかったこと。

---

## 非目標

- **capture list 構文** (論点 1 の C)。
- **closure の生存期間を追う本物の借用検査**。escape は保守的な構文規則で
  判定し、迷えば拒否する。
- **可変捕捉のための暗黙のヒープ確保** (論点 2)。
- **generic closure** — 既存の制限 (`generic-parameterised closures are not
  yet supported`) はこの設計の範囲外。

## 関連

- [`todo.md`](todo.md) の CLOSURE-CAPTURE (★★)
- [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) — 「5 レーンで同じことを
  言う」ための足場 (`assert_consistent` / `assert_diagnostic_consistent`)
- [`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) — 横断的変更の確かめ方
- `docs/language.md` の Closures / Captures 節 (正本)
