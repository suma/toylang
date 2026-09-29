# LLM_TOOLING.md — コンパイラ / `toy` を LLM が使う道具として仕上げる

[`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) (P0〜P7、2026-08 に完了) の
**第 2 ラウンド**。P0〜P7 は「1 往復で問題が読める」状態を作った。本書は
その上で、次の 7 性質を**道具の保証**として満たすのに何が足りないかを
実測で洗い出し、Phase に割る。

| # | 性質 | 一言で |
|---|---|---|
| 1 | 機械適用可能な修正提案 | 差分をそのまま当てれば通る |
| 2 | 安定したエラーコード + 説明への参照 | コードで分岐できる |
| 3 | 正確なスパン + 関連箇所 | 「どこ」と「どこと食い違うか」 |
| 4 | 意味情報の問い合わせ | 型・定義・参照・呼び出し関係を聞ける |
| 5 | 決定的な出力 | 同じ入力 → 同じバイト列 |
| 6 | 1 つのエラーに引きずられない回復 | 1 回で全部返す |
| 7 | 高速なフィードバック | 1 往復が安い |

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **L0** | 重複定義が ICE になるバグ → 診断 (`E0031`) | ✅ 2026-09-29 |
| **L1** | `E0010` (catch-all) の分割とコード台帳の append-only 化 | ✅ 2026-09-30 (一部。残り 211 か所) |
| **L2** | 診断 JSON に `related` と `edits` (ファイル付きの編集) を足す | `edits` ✅ 2026-09-29 / `related` 提案 |
| **L3** | 修正提案の拡充 + `toy fix` | ✅ 2026-09-29 |
| **L4** | パースエラーがあっても型検査まで進む / `Unknown` 由来の連鎖を抑える | 提案 |
| **L5** | 出力順序の明示的な正規化 + 決定性のテスト | 提案 |
| **L6** | `toy query` (型 / 定義 / 参照 / 呼び出し関係) | 提案 |
| — | インクリメンタル型検査 | **非目標** (§7) |

---

## 実測 (2026-09-29、release ビルドの `compiler` / `toy`)

以下のプログラムはすべて使い捨てのスクラッチファイルで、出力は
`--format=json` のものを抜粋した。

### 1. 修正提案 — 出している場所は 2 か所だけ

`Suggestion` を作っているのはワークスペース全体で次の 2 か所:

- `frontend/src/type_checker/error_helpers.rs` — 数値型不一致への `as T` 挿入
- `frontend/src/type_checker/expression.rs` — 関数名の did-you-mean

LLM_FEEDBACK_LOOP 論点 3 が候補に挙げた残り 2 つは**実装されていない**:

```
# `else if` — 文言は直し方を言うが、suggestions は空。コードも catch-all
E0010 "`else if` is not supported; write `elif` instead (`} elif cond {`)"   suggestions: []

# `val z = 1.5` — タプル添字として読まれ、見当違いの文言になる
E0010 "Cannot access index 5 on non-tuple type Number"                        suggestions: []
```

スキーマ側の制約も 2 つある:

- **`Span::file` を JSON に出していない** (`#[serde(skip)]`) — 提案の
  対象ファイルは「診断の `file`」と暗黙に決まる。別ファイルを直す提案
  (呼び出し側のエラーを定義側で直す等) を表せない
- **1 提案 = 1 置換** — `&` を引数とパラメータの両方に足す、のような
  複数箇所の編集を 1 つの提案にできない

一方で**「適用して再検査すると通る」ことをテストで確かめる**という
正しい性質は既にある (`interpreter/tests/diagnostics_json_tests.rs`)。
これを提案すべてに対する不変条件に広げるのが L3 の芯。

### 2. エラーコード — 30 個あるが、`E0010` が catch-all

`E0001`〜`E0030` と `--explain` / `toy explain` は揃っている。問題は
`E0010` (`UNCATEGORISED`) の守備範囲:

- **字句エラー以外のパースエラーはすべて `E0010`**
  (`Diagnostic::from_parser_error`)
- **`TypeCheckError::generic_error(` の呼び出しが frontend に 274 か所**
  あり、これが全部 `E0010`

つまり「`E0010` なら X を直す」という分岐が書けない。E0010 の文言には
内部表現も漏れている:

```
"parse_type_declaration: unexpected token Some(Equal)"       # 関数名と Debug 表示
"unexpected token in primary expression: Some(BraceClose)"
```

コードの**安定性**を守る仕組みも無い — 番号を再利用しない、という規約は
LLM_FEEDBACK_LOOP P3 にあるが、それを落とすテストは無い。

### 3. スパン — 主スパンは正確、関連箇所が無い

主スパンがソースのバイト範囲に解決されることはテストで固定済み
(P3 の性質 1)。欠けているのは**2 つ目以降の位置**:

```
fn g(a: u64) -> u64 { a }
...  val x = g(true)
→ E0001 "expected u64, but got bool (in argument 1 of function 'g')"
   # g の宣言 (パラメータ a の型) を指す位置が無い

...  val b = B { v: v }
     println(v.size())
→ E0014 "`v` was moved on line 8 and cannot be used again"
   # 移動した位置は本文の文字列の中だけ (列も範囲も無い)
```

`Diagnostic` に関連箇所の欄が無いので、情報を持っている検査も
メッセージ文字列に埋めるしかない。

**重複定義はバグ**。関連箇所どころか診断にならない:

```
fn f() -> u64 { 1u64 }
fn f() -> u64 { 2u64 }
→ 型検査は通り、lowering で panic:
  thread 'main' panicked at compiler_ir/src/lib.rs:504:13:
  function_index collision for symbol=SymbolU32 { value: 71 } ...
```

(`interpreter` でも同じ panic。LLM がコードを追記・複製するときに
最も踏みやすい形で、しかも出力が JSON でもコード付きでもない。)

### 4. 意味情報の問い合わせ — シグネチャと効果は聞ける、式は聞けない

既にあるもの:

| 問い | 道具 |
|---|---|
| モジュールが公開するシグネチャ (契約込み) | `toy api` |
| 宣言ごとの効果 | `toy effects` |
| ある式の型 | 型ホール `val x: _ = expr` (ソースを書き換える必要がある) |

無いもの: **位置 → 型**、**名前 → 定義位置**、**定義 → 参照一覧**、
**関数 → callers / callees**。

材料は揃っている:

- 型検査後の `expr_types: HashMap<ExprRef, TypeDecl>`
  (`const_fn_check.rs` / `effects.rs` が既に受け取っている)
- `ExprPool::expr_locations` / `stmt_locations` (範囲付き)
- `effects.rs` の到達性解析 — 呼び出し辺をたどって witness 経路を
  出す仕組みが既にある (`render_path`)

### 5. 決定性 — 実測では安定、ただし保証は構造にない

30 個の struct / impl / fn にエラーを仕込んだ 60 診断のプログラムを
6 回走らせ、stdout+stderr の md5 が 6 回とも一致した。順序はソース順。

ただしこれは「今の走査順がたまたまソース順」なだけで、出力直前に
ソートしている箇所は無い。frontend は `HashMap` を多用しており
(`type_cache` 等)、Rust の `HashMap` は**プロセスごとに seed が変わる**ので、
将来どこかの pass が HashMap を回して診断を積んだ瞬間に揺れ始める
(JIT の採択関数一覧は実際に run ごとに順序が変わる —
COMPILER_DEV_LOOP D8)。JSON の `file` が**起動時の引数どおりの絶対パス**
なのも、マシン間で出力を比べる用途では揺れの元。

### 6. 回復 — 型エラーは文単位で回復、パースエラーで全停止

- 型エラー: 4 関数に散らした 4 件がまとめて出る (P1 の成果)
- パースエラー: 別々の関数の 2 件はまとめて出る
- **パースエラーが 1 件でもあると型検査に進まない**。`else if` を
  1 つ混ぜると、同じファイルの型エラー 4 件が消え、出るのは 1 件だけ。
  LLM は「直した → 次の 4 件が出る」でもう 1 往復する
- **`Unknown` 由来の連鎖が残っている**:

```
val p = P { x: 1u64, z: 2u64 }   # E0010 Unknown field 'z'   (本物)
p.w                               # E0004 "... field access 'w' for type Unknown"  (連鎖)
```

### 7. 速度 — 既に速い。遅いのは link

`toy new` の雛形 (entry 1 + stdlib 48 モジュール、12,885 行) に対して:

| 操作 | 実測 |
|---|---|
| `toy check --format=json` (warm) | **26 ms** |
| `compiler --profile=compile` 全体 | 73 ms (うち **link 38 ms = 53%**、typecheck 15 ms) |

AST は `.toycache` に載っている。型検査を差分化しても 15 ms のうち
数 ms しか取れず、プロセス起動 (~数 ms) と同じ桁になる。
**1 往復を支配しているのは型検査ではない**。

---

## 論点と推奨

### 論点 A: 「関連箇所」と「編集」をどう運ぶか

**推奨: 既存キーは残したまま `related` と `edits` を足す。**

```json
{
  "code": "E0014",
  "message": "`v` was moved and cannot be used again",
  "file": "main.t",
  "span": { ... },
  "related": [
    { "file": "main.t", "span": { ... }, "message": "moved here" }
  ],
  "suggestions": [
    { "message": "borrow instead of moving",
      "applicability": "maybe-incorrect",
      "edits": [
        { "file": "main.t", "offset": 120, "end_offset": 121, "replacement": "&v" }
      ],
      "replacement": "...", "span": null }
  ]
}
```

- `edits` は**ファイル付き・複数可**。1 提案 = 1 原子的な変更集合
- `offset` / `end_offset` は**バイト**、`file` は診断の `file` と同じ
  正規化を受ける (論点 C)
- 旧 `replacement` / `span` は `edits` が 1 件のときだけ出す互換用。
  消すのは consumer が移ってから
- `Span::file` (`FileId`) を直接シリアライズしない理由 (プログラム内部の
  index でしかない) は正しいので、**path への解決は `anchor_in` と同じ
  場所で行う**

### 論点 B: machine-applicable の基準

LLM_FEEDBACK_LOOP 論点 3 を維持する: **当てれば、その診断が消え、
新しい診断が増えない**ものだけを `machine-applicable` と呼ぶ。
推測を含むものは `maybe-incorrect` として出してよい (スキーマには既に
あるが emit していない) — LLM は `maybe-incorrect` を「候補」として
扱えるので、**黙るより候補を出す方が往復が減る**場面がある
(E0014 の `&` 化など)。ただし種類は分けて、機械適用は前者に限る。

この基準は**テストで強制する** (L3): 診断コーパス (`ERROR_EXAMPLES` と
`diagnostics_json_tests` の入力) の全 machine-applicable 提案を実際に
当て、再検査して「その診断が消え、件数が増えない」ことを確かめる。

### 論点 C: 決定性をどこで担保するか

**推奨: emit 直前の 1 か所で正規化する。** 各 pass に「順序に気をつけて」
と求めるのは守られない。

- ソートキー: `(file, offset, end_offset, code, message)`
- 完全重複は落とす
- `file` は `toy` 経由ならパッケージ root からの相対、`compiler` 直なら
  引数どおり (勝手に絶対化しない)
- テスト: 診断を出すフィクスチャを**別プロセスで 2 回**走らせてバイト
  比較する。`HashMap` の seed はプロセスごとに変わるので、これだけで
  順序依存を捕まえられる

### 論点 D: 意味情報を LSP で出すか CLI で出すか

**推奨: CLI (`toy query`) を先に。** todo.md の LSP 項目の結論と同じで、
エージェントは 1 コマンド 1 応答で使う。LSP サーバの常駐・初期化手順は
エージェントにとってコストでしかない。`toy query` の実装は後で LSP の
ハンドラからそのまま呼べる形にする。

注意点が 1 つ: **型検査器は AST を書き換える** (`?` / `??` / `Display` /
char narrowing)。「位置 → 型」は**ユーザが書いた式のうち、位置を含む
最小のもの**を返す必要があり、desugar で生えた節点 (位置を元の式から
継いでいる) を選んではいけない。書き換えで生えた節点に印を付けるか、
書き換え前の `ExprRef` → 型の対応を残すかを L6 で決める。

### 論点 E: インクリメンタル検査をやるか

**推奨: やらない (非目標)。** §7 の実測どおり `toy check` は 26 ms で、
型検査は 15 ms。差分化の複雑さ (依存追跡、キャッシュ無効化、
`FULL_AST_CACHE_SCHEMA_VERSION` 級の罠の増殖) に見合わない。
速度で投資するなら:

- **往復回数を減らす** (L4 — パースエラーで止まらない) 方が 1 往復
  (26 ms) を削るより桁で効く
- **1 プロセスで複数の問いに答える** (`toy query` の一括指定) —
  プロセス起動と stdlib 読み込みを償却する
- AOT の `toy test` は link が支配的なので、そちらは TEST_PARALLEL /
  `TOY_LINK_CACHE_DIR` の延長で扱う

---

## Phase 詳細

### L0 — 重複定義を診断にする (✅ 2026-09-29)

実装: `frontend/src/type_checker/duplicate_defs.rs`。キーは
(名前空間, ファイル, 名前) で、名前空間は fn / 型 (struct と enum 共通) /
trait / const。主スパンは 2 つ目の**名前**、1 つ目の行は本文に入れた
(構造化した関連箇所は L2)。2 つ目以降の型宣言は登録から外すので、
残りの検査は 1 つ目に対して走り、他のエラーも同じ回で出る。
同じ impl 内の method の重複は既存の検査 (`find_duplicate_impl_method`、
まだ `E0010`) のまま。以下は着手前の計画:


- 同じスコープの `fn` / `struct` / `enum` / `trait` / 同じ impl 内の
  method の重複を型検査で拒否する新コード (`E0031`)
- 主スパンは 2 つ目、**関連箇所に 1 つ目** (L2 の `related` を先取り。
  L2 前はメッセージに行番号)
- `compiler_ir` の `function_index collision` panic は「型検査を
  すり抜けた」ことを示す内部不変条件として残す
- テスト: `compiler/tests/consistency/` ではなく診断テスト側
  (実行前に止まるので)

### L1 — `E0010` を分割し、コード台帳を append-only にする (✅ 2026-09-30、一部)

入ったもの:

- パースエラー: `E0032` 構文 / `E0033` `else if` / `E0034` サフィックスなし
  小数 (字句エラーは従来どおり `E0012`)。文言から `Some(BraceClose)` の
  ような `Debug` 表記と内部関数名 (`parse_var_def:` 等) を除いた
  (`Kind::describe` / `DescribeToken`)
- 型検査: 汎用エラーに**コード付き**の種別 `Coded` を足し、系統ごとに
  移した — `E0035` 網羅性・到達性 (13) / `E0036` パターンの形 (37) /
  `E0037` `?`・`??` の被演算子 (6) / `E0038` トレイト境界・impl 適合・
  未定義トレイト (10) / `E0039` 共有借用への書き込み (2)。同じ impl の
  同名 method は重複定義として `E0031`
- 台帳テスト (`diagnostic.rs`): 出荷済みコードの列が `codes::ALL` の
  先頭と一致すること、番号が連続で重複しないこと
- JSON の各診断に `explain` (`"toy explain E0035"`)

**残り**: `generic_error` / `TypeCheckError::new` の呼び出しが 211 か所
`E0010` のまま。2026-09-30 にテスト全体で出る E0010 を数えたところ
737 件中 255 件が `match scrutinee ... got Unknown`、83 件が
`desugar_null_coalesce: left operand was never typed` で、どちらも
**先行エラーの連鎖** (L4 の対象) だった。以下は着手前の計画:


1. **パースエラーにコードを振る** — `ParserErrorKind` ごと。まず件数の
   多いもの (予期しないトークン、`else if`、型注釈の欠落、閉じ括弧) から。
   文言から内部の関数名と `Some(...)` の Debug 表示を除く
2. **`generic_error` の 274 か所をトリアージ** — `ERROR_EXAMPLES` と
   診断テストで実際に発火するものを数え、上位から既存コードへ寄せるか
   新コードを切る。発火しないものは後回し
3. **台帳テスト** — `codes::ALL` の (コード, 定数名) をスナップショット
   ファイルに固定し、**追加は許すが変更・削除は落とす**。廃止するコードは
   `explain` に「retired」として残す
4. 診断 JSON に `"explain": "toy explain E0014"` を足す — 参照先を
   URL ではなくコマンドにするのは、オフラインで・版の一致した説明が
   引けるため

### L2 — `related` と `edits`

論点 A の形。`related` を最初に埋める候補 (情報を既に持っている順):

| コード | related |
|---|---|
| E0031 (L0) | 1 つ目の宣言 |
| E0014 | 移動した位置 (今は本文の「line 8」) |
| E0001 (引数) | 呼ばれた関数のパラメータ宣言 |
| E0003 (did-you-mean) | 候補の定義位置 |
| E0023 | trait 側のシグネチャ |
| E0022 / E0026 | allocator / コンテナの束縛位置 |

テキスト出力は rustc の `note:` と同じく、関連箇所ごとにスニペットを
1 つ足す (「テキストは Diagnostic の射影」を崩さない)。

### L3 — 修正提案の拡充と `toy fix` (✅ 2026-09-29、`edits` も)

入ったもの:

- **`edits`** — 提案は編集の列 (`file` / `span` / `replacement`)。
  1 件のときは旧キー `replacement` / `span` も出す。型検査器は
  SourceMap を持たないので、「主スパン内のこの語」という未解決の編集
  (`WordInSpan`) を作り、`Diagnostic::anchor_in` が本文から位置を
  決める。**見つからなければ提案ごと捨てる** (主スパン全体を置換しない)
- 提案: `else if` → `elif` / `1.5` → `1.5f64` (パーサが専用エラー
  `UnsuffixedFloat` で検出。`f32` の宣言の初期化子なら `1.5f32`) /
  E0024 に位置と `unsafe ` の挿入 (以前は**位置なし**だった) /
  E0028 に `get` → `borrow` + 注釈の `&` (2 編集) / フィールド
  (リテラル・アクセス)・メソッド・モジュール修飾関数の did-you-mean
- 構造体リテラルは**未知のフィールドを欠けたフィールドより先に**報告
  する (typo が「欠けている」と言われていた)
- パーサが完全に回復できる誤り (`1.5`) は宣言ごと 1 件・行ごと 1 件
  の畳み込みの対象外 (`report_recovered_error`)
- モジュールのパースエラーを文字列 1 本ではなく**全件の構造化診断**
  (モジュールのファイル・提案つき) で運ぶ
- `toy fix [--dry-run]` — 当てて再検査を最大 8 巡。パッケージ外
  (stdlib) は書かない。同一プロセスで再検査するので、モジュール
  発見のプロセス内キャッシュを捨てる口 (`forget_discovered_modules`)
  を足した
- 不変条件テスト: `diagnostics_json_tests.rs` の `FIXABLE` — 全提案を
  当てると検査が通る

入れなかったもの: `maybe-incorrect` の提案 (E0014 の `&` 化、網羅性の
腕の挿入)。以下は着手前の計画:


追加候補 (machine-applicable と言い切れるもの):

| 状況 | 編集 |
|---|---|
| `else if` | `else if` → `elif` |
| `1.5` (サフィックス無しの小数) | `1.5f64`。**パーサで検出して専用のエラーにする** (今はタプル添字として読まれ文言が壊れる) |
| E0028 (`val e: T = v.get(i)`) | `val e: &T = v.borrow(i)` |
| E0024 (unsafe が要る) | 関数宣言の前に `unsafe ` |
| E0015 (`null`) | 生ポインタ文脈なら `__builtin_null_ptr()` |
| E0003 (フィールド / メソッド名) | 関数名と同じ一意性条件での did-you-mean |

`maybe-incorrect` として出すもの: E0014 の `&` 化、網羅性エラーへの
欠けた腕の挿入 (`=> panic("todo")` は意味を変えるので機械適用しない)。

**`toy fix [PATH] [--dry-run] [--format=json]`** — machine-applicable な
提案だけを当て、再検査し、当てた編集と残った診断を返す。編集が重なる
場合は後の提案を捨てて次の周回に回す (最大 N 周)。LLM が自分で差分を
組み立てる往復を消す。

### L4 — 回復の強化

1. **パースエラーのある item を飛ばして型検査に進む** — パーサは既に
   関数をまたいで回復している。壊れた item を「シグネチャだけ登録
   (本体は `Unknown`)」または「丸ごと除外」して残りを検査する。
   シグネチャが読めた関数を登録しておけば、呼び出し側が E0003 の連鎖に
   ならない
2. **`Unknown` を含む診断を emit 段で落とす** — 今は個々の検査が
   poison 伝播を実装している (P1 / P3 で漏れを 1 件ずつ塞いだ)。
   「メッセージの型に `Unknown` が現れる診断は、先に本物の診断が
   出ていれば落とす」を 1 か所で掛ける。本物が 0 件なのに `Unknown` が
   出るのは型検査器のバグなので、その場合は残す (隠さない)

### L5 — 決定性の保証

論点 C の実装。正規化 1 か所 + 2 プロセス比較テスト。`toy test` の
報告は既に plan 順 (TEST-PARALLEL) なので対象外。`--profile=compile` の
時間は本質的に揺れるので対象外 (構造 — フェーズの木 — だけ比べる)。

### L6 — `toy query`

```bash
toy query type    main.t:12:9  [main.t:30:5 ...]   # 位置を含む最小の式の型
toy query def     main.t:12:9                      # 名前の定義位置
toy query refs    main.t:3:4                       # 定義 → 参照一覧
toy query callers pkg::f                           # 直接の呼び出し元
toy query callees pkg::f                           # 直接の呼び出し先 (dyn / closure / extern は "opaque")
```

- すべて `--format=json` で `{query, results: [{file, span, ...}]}`
- **位置を複数取れる** (1 プロセスで一括。§7)
- 型検査が失敗しても、**検査できた範囲で答える** (L4 と同じ方針)。
  答えられない位置は `results` に理由付きで入れる
- callers / callees は `effects.rs` の辺を流用する。opaque な辺
  (`dyn` / closure / `extern`) を黙って落とさず `opaque` として出す
  — 「呼び出しは無い」と「追えない」を区別できないと LLM は誤る

## 推奨実装順

1. **L0** — ICE はループを止める。小さい
2. **L1** — コードで分岐できないと、以降の提案・related の価値が半減する
3. **L4-1** — 往復回数に直接効く (パースエラー混じりで 1 → N 件)
4. **L2** — L0 / L1 の上に乗る形の変更
5. **L3** — `edits` ができてから提案を増やす。コーパス不変条件テストと同時に
6. **L5** — 小さいが、L2 / L3 で出力が増える前に入れておくと安全
7. **L6** — 独立に進められる。最も大きい

## 非目標

- **インクリメンタル型検査 / `--watch`** — 論点 E
- **LSP を先に作る** — 論点 D
- **推測の提案を machine-applicable と呼ぶ** — 1 度の誤適用で以降の提案
  すべての信頼を失う (LLM_FEEDBACK_LOOP 論点 3)
