# LLM_FEEDBACK_LOOP.md — LLM の試行錯誤ループを速くする言語機能

LLM (コーディングエージェント) が toylang でコードを書くとき、その作業は
**「書く → 検証する → エラーを読む → 直す」** のループになる。このループを
速くするために toylang 側に何を実装すべきかの設計ドキュメント。
FFI_PLAN.md / ALLOCATOR_PLAN.md / DYN_TRAIT_AOT.md と同じく
「現状調査 → 論点決定 → Phase 分割 → MVP 刻みで landing」のスタイルで進める。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **P0** | 致命的な診断バグの修正 (bare-name 解決順) | ✅ 2026-08-09 |
| **P1** | 診断の一括報告 (文単位のエラー回復) | ✅ 2026-08-09 |
| **P2** | Span 化 + 全診断への location 強制 | ✅ 2026-08-09 |
| **P3** | 構造化診断出力 (`--diagnostics=json`) + 修正提案 | ✅ 2026-08-09 |
| **P4** | 言語組み込みテスト (`test` ブロック + `assert_eq`) | 未着手 |
| **P5** | 契約ベース自動プロパティテスト (`toy check`) | 未着手 |
| **P6** | 実行時の観測性 (panic backtrace / 契約違反の値キャプチャ) | 未着手 |
| **P7** | 補助 CLI (型ホール / `toy api` / エラーコード解説) | 検討のみ |

## 設計原理 — ループを速くする 3 つの手段

ループの所要時間は `往復回数 × 1 往復のコスト` で決まる。改善手段は 3 つしかない。

1. **1 回のフィードバックの情報量を増やす** — 1 往復で複数の問題を解決させる
2. **往復回数そのものを減らす** — 推測を強いる状況をなくす
3. **検証を実行前に前倒しする** — 型検査・契約で早期に落とす

toylang は既に (3) が強い (型検査 + Design by Contract + 網羅性検査)。
**手薄なのは (1) と (2)** であり、投資対効果はそちらが圧倒的に高い。

重要なのは、LLM は人間と違って **「エラーメッセージに書かれていないこと」を
補完できない** という点。人間なら「たぶん stdlib のせいだろう」と勘を働かせて
ソースを読みに行くが、LLM はメッセージに手がかりがないと同じ修正を無限に
試行する。**診断の情報量不足はループを遅くするのではなく、ループを止める。**

## 背景 — 現状の足場 (2026-08-09 実測)

### 実測 1: エラーは「関数単位」でしか回復しない

`frontend/src/type_checker/module_access.rs:13` の `check_program_multiple_errors`
は複数エラーを集める器を持っているが、粒度が **関数単位** になっている。

```rust
for func in &program.function {
    if let Err(e) = self.type_check(func.clone()) {
        self.errors.push(e);   // 1 関数につき 1 エラーで打ち切り
    }
}
```

`type_check` の内側 (`visit_stmt` / `visit_expr`) は `Result<_, TypeCheckError>`
で `?` 早期 return するため、**1 つの関数に 3 つエラーがあっても 1 件しか出ない**。

実測: 3 箇所にエラーのあるファイルで報告されたのは 1 件のみ。

```rust
fn main() -> u64 {
    val y: i64 = x              // (1)
    val z = undefined_fn(3u64)  // (2) ← これだけ報告される
    val w: bool = 1u64          // (3)
    0u64
}
```

LLM 視点では **修正 1 件につき 1 往復**。N エラーで N ラウンドかかる。

### 実測 2: location が付かない診断が多数ある

`TypeCheckError { kind, context, location: Option<SourceLocation> }` で
location が `Option` のため、**付け忘れが構造的に起きる**。

- `TypeCheckErrorKind::GenericError { message }` が事実上の catch-all
- `generic_error()` / `not_found()` などのコンストラクタは location を付けない
- `error_with_location()` で後付けする設計だが、**呼ばれているのは 35 箇所**に対し
  エラー生成は **209 箇所** — 大半が location なしで生成されうる

location が `None` のとき `interpreter/src/error_formatter.rs:50` は
ソース行の表示を諦めてフォールバックする。

```rust
pub fn format_type_check_error(&self, error: &TypeCheckError) -> String {
    if let Some(location) = &error.location { ... }
    else { format!("Error: {error}") }   // ← 位置情報ゼロの出力
}
```

実測される出力:

```
Type check errors found:
  Error: Type mismatch: expected Bool, but got UInt64 (in Cannot convert 'u64' to 'bool')
```

**ファイル名も行番号もない。** LLM はこれを受け取ると全文を読み直すしかない。

### 実測 3: SourceLocation が「点」であって「範囲」ではない

```rust
pub struct SourceLocation { pub line: u32, pub column: u32, pub offset: u32 }
```

範囲 (span) を持たないため caret を正しい幅で引けない。現状の
`ErrorFormatter::find_error_position_in_line` は **エラーメッセージの文字列から
識別子を推測してソース行を検索する** というヒューリスティックで凌いでいる。

```
 7 |     val y = add(x, 2i64)
   |             ^^ 7:16:101: Type error: expected Int64, found UInt64. ...
```

- caret が `^^` の 2 文字しかなく、どの部分式が悪いか分からない
- 内部座標 `7:16:101:` がメッセージ本文に混入していてノイズになっている

span を持てばヒューリスティックは不要になり、caret も正確になる。

### 実測 4: stdlib 内で発生したエラーがユーザに帰着しない (P0 バグ)

`f` という名前のトップレベル関数を定義すると、ユーザコードが完全に正しくても
コンパイルが通らない。

```rust
fn f(n: u64) -> u64 { n - 1u64 }
fn main() -> u64 { 0u64 }
```
```
Type check errors found:
  Error: Function 'f' argument count mismatch: expected 1, found 0
```

**原因**: `frontend/src/type_checker/expression.rs:1055` の `visit_call` が
**グローバル関数テーブルを先に引き、ローカル変数を fallback にしている**。

```rust
if let Some(fun) = self.context.get_fn(fn_name) {
    ... // 直接呼び出しとして扱う
} else {
    self.visit_call_indirect_fallback(fn_name, args_ref)  // ← クロージャ値はここ
}
```

`core/std/option.t:63,71` / `core/std/result.t:62,70` はクロージャ引数 `f` を
`f(v)` / `f()` の形で呼んでいる。ユーザが `f` という関数を定義すると、この
stdlib 内の `f(v)` が **ローカルのクロージャ引数ではなくユーザの `f`** に
解決され、arity が合わずにエラーになる。レキシカルスコープの原則
(内側の束縛が勝つ) に反している。

LLM 視点でこれが最悪なのは以下の 3 点が重なるため:

- エラーは自分が書いていない場所 (stdlib) で起きている
- **location が一切出ない** (実測 2 の症状)
- 出力に stdlib への言及がなく、原因を推測する手がかりがゼロ

LLM は「引数の数を変える」「関数名を変える」を延々と試行する。
**これは「ループが遅い」ではなく「ループが終わらない」ケース。**

→ P0 で修正する (詳細は後述)。

### 実測 5: panic に位置情報もバックトレースもない

```rust
fn g(n: u64) -> u64 { panic("boom") }
fn h(n: u64) -> u64 { g(n) }
fn main() -> u64 { h(3u64) }
```
```
Runtime error occurred:
panic: boom
```

どの関数のどの行で落ちたのか、どこから呼ばれたのかが分からない。
`interpreter/src/error_formatter.rs:57` の `format_runtime_error` は
`Option<&SourceLocation>` を受け取れる作りになっているが、panic 経路では
`None` が渡っている。

### 実測 6: assert が真偽しか返さない

現状の `assert(cond, "msg")` は `(bool, str) -> ()`。失敗しても
「何と何を比較して、実際の値がいくつだったのか」が出ない。
LLM は原因を知るために `print` を挿入して再実行する往復を強いられる。

### 参考: 既にある足場

- `MultipleTypeCheckResult<T> { result, errors: Vec<TypeCheckError> }` —
  複数エラーを運ぶ器は**既に存在する** (`frontend/src/type_checker/error.rs:12`)。
  P1 は器の新設ではなく回復粒度の変更で済む。
- `collect_error()` (`module_access.rs:8`) — エラーを積む API も既にある。
- `SourceLocation` は `serde` feature 対応済み — P3 の JSON 化に流用できる。
- `INTERPRETER_CONTRACTS=all|pre|post|off` で契約の実行制御が既にある —
  P5 の実行基盤として使える。
- インクリメンタルコンパイル Phase 1-4 完了 — 検証コマンドは既に速い
  (warm 0.01s vs cold 0.04s)。ループのボトルネックは**実行速度ではなく診断品質**。

## 設計論点と決定

### 論点 1: 新機能とエラー品質、どちらを先にやるか

**決定: エラー品質を先にやる。**

「LLM 向けの機能」と聞くと組み込みテストや AST API のような新機能を思い浮かべ
がちだが、実測 1〜4 が示す通り現状は **1 往復あたりの情報量が構造的に不足** して
いる。テストを増やしても、失敗の原因が読めなければ往復回数は減らない。
P1 (一括報告) は単独で往復回数を N → 1 にする唯一の変更であり、
他のどの機能よりも効く。

### 論点 2: 出力はテキストか JSON か

**決定: 両方出す。テキストが正、JSON はオプトイン (`--diagnostics=json`)。**

LLM はテキストも読めるので JSON は必須ではない。JSON が効くのは
**エージェントのツール層**が「該当行だけ抽出する」「修正提案を自動適用する」
といった前処理をできる点。テキスト出力を人間向けに保ったまま、機械向けを
別チャネルで出す。

### 論点 3: 修正提案 (suggested fix) をどこまでやるか

**決定: machine-applicable なものだけを提案する。**

「たぶんこうでは」という曖昧な提案は LLM を誤誘導する。
**そのまま適用すれば必ず通る** ものに限定する:

- 数値型の不一致 → `as i64` の挿入
- `else if` → `elif` への書き換え (toylang 固有の罠、LLM が最も踏む)
- 未定義シンボル → 編集距離が閾値内で候補が一意なときのみ did-you-mean
- f64 リテラルのサフィックス欠落 → `1.5` → `1.5f64`

曖昧なケースは提案せず、エラーコード経由の解説 (P7) に誘導する。

### 論点 4: テストは別ファイルか同一ファイルか

**決定: 同一ファイルに書けるようにする。**

別ファイル方式だと LLM は「実装ファイルとテストファイルの整合」を維持するために
余計な往復をする。生成コードと検証が**単一ファイル・単一コマンド**で完結する
ことが重要。

### 論点 5: プロパティテストを新規に作るか、契約を再利用するか

**決定: 契約 (`requires` / `ensures`) を再利用する。**

toylang は既に Design by Contract を持っている。これを
**入力生成器 (`requires`) とオラクル (`ensures`)** として読み替えれば、
LLM がテストを 1 行も書かずに反例を得られる。これは他言語に対する
明確な差別化にもなる。

---

## Phase 詳細

### P0 — 致命的な診断バグの修正 (✅ 2026-08-09 完了)

実測 4 の `f` バグを修正した。

**修正方針**: bare-name 呼び出しの解決順を逆にする。ローカル束縛が関数型
(型検査では `TypeDecl::Function(..)`、実行時では `Object::Closure`、lowering では
`closure_bindings` / `Binding::FunctionPtr`) を持つ場合は**それを優先**し、
そうでない場合のみグローバル関数テーブルを引く。関数型でない同名変数
(`val h = 5i64` 等) は shadow しないので通常の呼び出しは影響を受けない。

**3 バックエンドが独立に同じ順序ミスを持っていた**ので、3 箇所すべてを修正した:

| 層 | 修正箇所 | 修正前の症状 |
|---|---|---|
| 型検査 | `frontend/src/type_checker/expression.rs::visit_call` | `Function 'f' argument count mismatch` (location なし) |
| tree-walker | `interpreter/src/evaluation/call.rs::evaluate_function_call` | 型検査を通しても**黙って別の関数本体が呼ばれる** (`map` が 10 でなく 50 を返す) |
| lowering (AOT / IR VM / compiler-JIT) | `compiler_lower/src/call.rs::resolve_call_target`、`type_inference.rs` | capturing closure では暗黙 env 引数のぶん arity がずれ、cranelift verifier error |

型検査だけ直して実行系を直さないと **型は通るが答えが間違う** 状態になる点に注意。
実測でこれを踏んだ (型検査修正直後、`o.map(...)` が 10 でなく 50 を返した)。

**回帰テスト**: `interpreter/tests/closure_tests.rs` に 4 件
(shadowing / stdlib 衝突 / stdlib HOF の正しい dispatch / 非関数値は shadow しない)、
`compiler/tests/consistency.rs` に 3-way 一致テスト 3 件
(non-capturing / capturing / 非関数ローカル)。1587 → 1594 tests pass。

### P1 — 診断の一括報告 (✅ 2026-08-09 完了)

**目標**: 1 回の実行でプログラム中の独立したエラーをすべて報告する。

**実装**: 回復粒度を関数単位から **文単位** に下げた。

- `TypeCheckerVisitor::recovery_enabled` フラグを新設。ON のとき、文の
  type check が失敗したら `recover_stmt_error` に吸収させて次の文に進む。
  OFF (デフォルト) では従来通り `Result` で fail-fast するので、
  既存の `type_check` 呼び出し側の契約は変わらない。
- **2 つの文ループ**の両方に回復点を置く必要があった:
  `visit_call` を含む関数本体のループ (`visitor.rs::type_check`) と、
  ネストしたブロックのループ (`expression.rs::visit_block`)。
  後者は `if` / `while` の body を通るので、ここを落とすと
  ブロック内のエラーで関数全体が abort したままになる。
- `visit_block` の文ごとの処理は `visit_block_stmt` に抽出した。
  内部の `?` が回復点を飛び越えて `visit_block` の外まで unwind して
  しまうため、**文全体を 1 つの fallible な単位にする**必要がある。
- 報告順はソース位置でソートする。`type_check_forward_ref` が呼び出し先の
  body を先に検査するので、収集順はファイル順と一致しない。

**カスケード抑制** — 一括報告は「本物のエラー 1 件につきノイズ N 件」を
生むと逆効果になるので、以下を同時に入れた:

1. **失敗した `val` / `var` を `Unknown` として束縛する** — そうしないと
   以降のすべての参照が「変数が見つからない」を出す。ユーザは宣言を
   書いているのだから、それは嘘の診断になる。
2. **二項演算のオペランドが `Unknown` なら `Unknown` を返す**
   (`visit_binary`)。これがないと `z + 1u64` が
   「expected Unknown, but got UInt64」を出す。**ユーザが書いていない
   内部型の名前を出す診断は最悪**で、LLM は存在しない型を「直そう」とする。
3. **body がエラーを出した関数では戻り値型の照合をスキップする** —
   body の型は回復用のプレースホルダなので、そこでの不一致は
   カスケードでしかない。

**併せて修正したパーサのバグ**: `parse_var_def` が `val` / `var` の位置を
**rhs をパースし終えた後**に取得していたため、束縛に紐づく診断がすべて
1 文だけ下を指していた。エラーに location が付いていない間は不可視だったが、
P1 で location が付くようになると**間違った行を指す診断**になる。
LLM にとっては「位置なし」より「間違った位置」の方が有害なので、
キーワード位置で取るように修正した。

**受け入れ基準 (達成)**: 実測 1 の 3 エラーが 1 回の実行ですべて報告される。
関数をまたぐケース、ネストブロック内のケースも同様。

**回帰テスト**: `interpreter/tests/diagnostics_recovery_tests.rs` (10 件) —
一括報告 / 関数またぎ / ネストブロック / カスケードなし / 戻り値型の
二重報告なし / ソース順 / 宣言行を指すこと / 正常プログラムが壊れないこと。

**非目標**: 式単位の回復 (1 つの式の中の複数エラー)。効果に対して
カスケードエラー抑制の複雑さが見合わない。

#### 追補 (2026-08-09): パースエラーも全件報告に

P1 は型エラーだけを対象にしていたため、**構文エラーは 1 件で止まっていた**。
`compiler_core::parse_program_all_errors` を追加して CLI 経路を切り替え。

型チェッカと違い**パーサは文境界で綺麗に回復しない**ので、そのまま全件出すと
1 つの誤りから派生した「unexpected token」の連鎖が並ぶ。以下 2 段で抑制した:

1. **1 行につき 1 件** — 同じ行の後続は同じ誤りの言い換え
2. **1 宣言につき 1 件** — パーサは宣言境界では確実に再同期するが、
   宣言内部では誤ったトークンの後を延々と引きずる。
   `Parser::begin_declaration` を**宣言キーワードを見たときだけ**呼ぶ
   (catch-all の「1 トークン読み飛ばす」経路でリセットすると、
   読み飛ばした全トークンが報告されてしまう)

結果、`;` と `else if` を 1 つずつ含むファイルはちょうど 2 件を報告する。


### P2 — Span 化 + 全診断への location 強制 (✅ 2026-08-09 完了)

**目標**: すべての診断が位置を持ち、その位置が信用できること。

実測 2 / 3 の通り、P2 以前は 3 つの異なる問題が同時に起きていた。

#### (1) メッセージ本文への内部座標の混入

`TypeCheckError` の `Display` が `line:column:offset:` を prefix していた。
formatter は既に `Error at <file>:<line>:<col>` を出しているので**重複**であり、
しかも `offset` (ソースへのバイト index) は読み手にとって無意味なノイズ。
`Display` はメッセージ本文のみを返すようにした。位置が要る呼び出し側は
`self.location` を読む。

#### (2) caret の推測をやめて span から導出

`SourceLocation` に `end_offset` を追加し、`width()` を生やした。
parser は現在トークンの `Range` の `end` を、`Node` は `node.end` を渡す。

これで `ErrorFormatter::find_error_position_in_line` を**削除**できた。
この関数は「エラーメッセージから最初のシングルクォート囲みの名前を取り出し、
ソース行を検索する」という推測をしていた。名前を引用しないメッセージ
(型不一致の大半) では固定幅 `^^` に落ち、同じ名前が行内に 2 回出れば
間違った方を指していた。

#### (3) アンカー位置の修正

位置が「付いている」だけでは足りず、**正しい構文要素**を指す必要がある。

| 診断 | 修正前のアンカー | 修正後 |
|---|---|---|
| `val x: bool = 1u64` | `val` (文の位置) | `1u64` (初期化子) |
| `foo(...)` が未定義 | `(` | `foo` (callee 名) |
| 引数の型不一致 | callee 名 | 当該引数の式 |
| method の戻り値型不一致 | 位置なし | method 本体 |

callee 名のケースは parser 側の修正。`parse_primary_after_identifier` が
識別子を消費した**後**に位置を取っていたため、`foo(...)` / `foo[...]` /
`Foo::bar(...)` のすべてが次のトークンを指していた。識別子を消費する前に
span を取って引数として渡すようにした。

#### (4) location カバレッジ

**根本原因**: `visit_expr` は失敗時に式の位置を stamp するが、
**18 箇所が `accept_expr` を直接呼んで `visit_expr` を迂回していた** —
文の本体、ループ条件、contract 節、impl block の method 本体など。
これらの下で発生したエラーは位置ゼロで報告されていた。

`check_expr_located` ヘルパを新設して該当箇所を差し替えた
(`visit_expr` 自体は type cache 参照と `Expr::Try` の書き換えも行うため、
そこには通さない)。加えて method 戻り値型と `const` 宣言に個別に位置を付けた。

**実測**: 代表的な 10 種のエラーで位置ありが **5/10 → 10/10**。

#### (5) import した module 由来のエラー

**最も危険な問題**。integrated module の location は user のファイルと
同じ pool に入り、区別する情報がない。したがって `core/std/option.t` 内で
発生したエラーが**ユーザのファイルに対して描画され**、同じ offset に
たまたま居た無関係なコードを自信満々に指す。

実測 (P2 前):

```
# エラーの実体は modules/helper.t:3 の `pub fn broken(a: u64) -> bool`
Error at main.t:3:6:
 3 | fn main() -> u64 {
   |      ^ Type mismatch: expected Bool, but got UInt64 ...
```

`main.t` は完全に無実。**位置なしより有害** — 読み手を具体的な間違った
場所に送り込む。これは P0 の `f` バグを解けなくしたのと同じ構図。

`TypeCheckError::origin_module: Option<String>` を追加し、`type_check` を
薄い wrapper (`type_check` → `type_check_body`) にして、
imported function の body から出たエラー (return 経路・収集経路の両方) に
module 名を stamp する。formatter は origin があればソース行を引用せず、
module 名を明示する。

```
Error in imported module `helper` (line 3 of that module): Type mismatch: ...
   = note: this comes from module `helper`, not from the file being compiled
```

qualifier の判定は**名前ではなく `Rc::ptr_eq` による同一性**で行う
(user 関数と import 関数は同名になりうる。混同すると診断が別ファイルを
誤って名指しする — まさに避けたい失敗)。lookup は error path でのみ実行する。

#### 副産物: `TypeCheckError` の縮小

`end_offset` (+4) と `origin_module` (+24) で構造体が 120 → 152 バイトになり、
`Result<_, TypeCheckError>` を返す関数が clippy の `result_large_err`
(閾値 128) に 206 件引っかかった。`kind` を `Box<TypeCheckErrorKind>` に
変更して **80 バイト** まで縮小 — 元の 120 バイトより小さい。
kind は診断を描画するときにしか読まないので、cold data を box するのは
レイアウトとしても正しい。

**回帰テスト**: `interpreter/tests/diagnostics_location_tests.rs` (12 件) —
内部座標の非混入 / caret 幅 / 4 種のアンカー / 位置カバレッジ 7 種。

**未実施 (P3 に送り)**: エラーコード体系 (`E0308` 等)。

### P3 — 構造化診断出力 + 修正提案 (✅ 2026-08-09 完了)

**目標**: テキスト出力を正としたまま、機械可読なチャネルと
「適用すれば必ず通る」修正提案を追加する。

#### 構造 — テキストは Diagnostic の射影

`frontend/src/diagnostic.rs` に `Diagnostic` / `Span` / `Suggestion` /
`Applicability` / `Severity` を新設。**テキスト出力は `Diagnostic` を
描画したもの**という関係にした (`ErrorFormatter::format_diagnostic`) ので、
人が見るものと tool が読むものが乖離しない。

driver (`check_typing_diagnostics`) が `Vec<Diagnostic>` を返し、
既存の `check_typing_with_core_modules` はそれを描画して `Vec<String>` を
返す薄い wrapper になった (既存 caller の signature 不変)。

**この過程で二重描画のバグを 1 件検出** — impl block の経路が内部で
formatter を通した String を返していたため、`Error: [E0010] Error at ...`
のように自身の描画結果に包まれていた。`process_impl_blocks_extracted` を
`Vec<TypeCheckError>` 返却に変更。

#### エラーコード

`TypeCheckErrorKind` の variant に `E0001`〜`E0010` を対応付けた。
**Rust の番号は流用しない** — 見た目が同じで意味が違う識別子は、
無いより悪い。

#### CLI

`--diagnostics=json` を interpreter / compiler の両方に追加。
出力先は **stderr** (プログラム自身の `print` 出力を同一実行で
使えるように)。`--diagnostics=text` が既定。

```json
{
  "severity": "error",
  "code": "E0003",
  "message": "Function 'calculate_totl' not found",
  "file": "typo.t",
  "span": { "line": 2, "column": 20, "offset": 59, "end_offset": 73 },
  "origin_module": null,
  "suggestions": [
    { "message": "a function named `calculate_total` exists",
      "replacement": "calculate_total",
      "span": null,
      "applicability": "machine-applicable" }
  ]
}
```

`Suggestion::span` が `null` のときは **診断自身の span** を対象にする。
名前解決の失敗は「エラーに位置が stamp される前」に提案が作られるので、
この形が自然。

#### 修正提案 — 確実なものだけ

論点 3 の方針通り、**適用すれば必ず通るもの**に限定した。

| 提案 | 条件 |
|---|---|
| `as <T>` キャスト挿入 | 両辺が `as` を受け付ける数値型のときのみ (`castable_type_name` が `Some`) |
| did-you-mean (関数名) | 編集距離が閾値内の候補が**唯一**のときのみ |

`u64` → `bool` のような `as` で書けない不一致には**提案を出さない** —
出せば読み手を「2 つ目のエラー」に送り込むだけになる。

did-you-mean の tie 判定は重要で、`printn` は `println` と `print` の
両方から距離 1 なので**どちらも提案しない**。片方を選ぶのは推測であり、
間違えると 1 往復失うだけでなく**以降の提案への信頼も失う**。

#### カスケード抑制の追加修正

P3 のテストが `as` キャストで P1 と同種の漏れを検出した:

```
val c = helper(1u64)   # 本物のエラー
c as u64               # → "Cannot cast Unknown to UInt64"
```

`Unknown` を cast の source 型として受理し、宣言された target 型を
返すようにした (binary operator と同じ poison 伝播規則)。

#### テスト方針

`interpreter/tests/diagnostics_json_tests.rs` (11 件)。
検証しているのは「JSON にこのキーがある」ではなく、consumer が実際に
依存する 2 つの性質:

1. **span がバイト単位でソースに解決される** — 各診断の
   `source[offset..end_offset]` が非難対象の文字列と一致すること
2. **machine-applicable な提案を適用すると通る** — 提案を実際に
   ソースへ適用し、型検査を再実行して成功を確認する

(2) が提案を出す価値の根拠なので、テキストを眺めるのではなく
適用して再実行する形にした。

**未実施**: `toy explain <code>` (P7 に送り)、`maybe-incorrect` 提案
(schema には存在するが現状 emit しない)。

### P4 — 言語組み込みテスト

`design-docs/todo.md` の「検討中の機能 → 言語組み込みテスト機能」を具体化する。

```rust
test "add handles zero" {
    assert_eq(add(0i64, 3i64), 3i64)
}
```

- `test "name" { ... }` を新しいトップレベル宣言として parser に追加
- 通常実行 (`main` の実行) では `test` ブロックを walk しない
- `toy test <file>` で全件実行、結果を P3 の構造化診断で返す
- テスト間は独立・決定論的

**最重要は `assert_eq` の失敗時出力**:

```
✗ add handles zero (main.t:2)
  assert_eq failed
    left:  0i64
    right: 3i64
```

実測 6 の通り、値が出ないと LLM は `print` デバッグの往復を強いられる。
`assert_ne` / `assert` も同様に、失敗時に評価された部分式の値を出す。

**Phase 分割**: P4-A (interpreter のみ) → P4-B (AOT / JIT)。
テスト実行は interpreter だけで実用上足りるので、P4-A で一旦止めてよい。

### P5 — 契約ベース自動プロパティテスト

`requires` を入力生成器の制約、`ensures` をオラクルとして扱い、
ランダム入力で反例を探す。

```
$ toy check src/math.t
✗ fn divide (math.t:15)
  ensures result * b == a  violated
  minimal counterexample: a = 1i64, b = 2i64  (result = 0i64)
  seed: 0x8f3a91  (replay: toy check --seed 0x8f3a91)
```

- `requires` を満たす入力のみを生成 (満たさない入力は捨てる)
- 違反したら **shrinking で最小反例まで縮小** する。
  縮小されていない反例は LLM が読み解くのに追加の往復が要るので、
  shrinking は「あれば嬉しい」ではなく**必須**
- seed 固定でリプレイ可能に
- 対象は当面スカラー引数 (`i64` / `u64` / `f64` / `bool`) の関数に限定

`INTERPRETER_CONTRACTS` の既存機構と、`interpreter/` の proptest 資産
(生成器・shrinker の考え方) を流用できる。

### P6 — 実行時の観測性

1. **panic のバックトレース** — `panic: boom at g (main.t:1)` +
   呼び出し元チェーン。実測 5 の通り現状は位置情報ゼロ。
   `format_runtime_error` は既に `Option<&SourceLocation>` を受けられるので、
   panic 経路で `Some` を渡すところから始める。
2. **契約違反時の実引数値のキャプチャ** —
   `requires b != 0i64 violated (b = 0i64)`。
   値が出れば LLM は再現コードを書かずに原因を特定できる。
   DbC を持っている強みを活かせる、コストの低い改善。
3. **u64 アンダーフローの trap** — `0u64 - 1u64` は LLM の頻出バグ。
   黙って wrap すると原因究明に何往復もかかる。デバッグビルドで明示的に
   落とす (リリースでの挙動は別途決定)。

### P7 — 補助 CLI (検討のみ)

| 機能 | 効果 |
|---|---|
| **型ホール `val x: _ = expr`** | 推論結果を表示。LLM が型を推測して外す往復を 1 回で潰せる |
| **`toy api <module>`** | stdlib のシグネチャ一覧を機械可読出力。`core/std/*.t` を grep するコストを削減 |
| **`toy explain E0308`** | エラーコードから原因カテゴリと典型的な修正例を引ける |
| **`must_use` / 未使用 Result 警告** | `design-docs/todo.md` の `NEW-TYPE-SYSTEM` に既出。LLM は戻り値を捨てがち |
| **`toy check --watch`** | インクリメンタルコンパイル済みなので低コスト |

`design-docs/todo.md` の「検討中の機能 → LSP 対応」も同じ目的に効くが、
**エージェントは LSP より CLI クエリを使いやすい** ため、
LLM ループの観点では `toy api` / 型ホールの方が優先度が高い。

## 非目標

- **LLM 専用の構文** — 言語を LLM 向けに歪めない。ここで挙げた機能は
  すべて人間の開発者にとっても素直に有用なものに限る。
- **式単位のエラー回復** — P1 の非目標として上述。
- **自動修正の無条件適用** — 提案は出すが、適用の判断は呼び出し側に委ねる。

## 推奨実装順

1. **P0** — バグ修正。ループが止まる問題なので最優先
2. **P1** — 往復回数を N → 1 にする唯一の変更
3. **P2** — location なし診断の撲滅。P1 と合わせて「1 往復で全問題が読める」状態に
4. **P4-A** — 組み込みテスト + 値を出す `assert_eq`
5. **P6-1 / P6-2** — panic 位置と契約違反値。どちらも低コスト・高効果
6. **P3** — 構造化出力。ここまでの情報が揃ってから機械可読にする方が設計が固まる
7. **P5** — 契約ベースプロパティテスト。差別化機能
8. **P7** — 個別に判断

P1 + P2 + P4-A + P6 が揃うと、**「LLM が書く → 自分で失敗の原因を特定する →
自分で直す」が言語機能だけで閉じる**。ここが最初の到達目標。
