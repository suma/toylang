# COMPILER_DEV_LOOP.md — LLM で toylang コンパイラ自体を開発するための指針

LLM (コーディングエージェント) が **toylang コンパイラそのもの** を開発・
デバッグするときに、何がコストになり、リポジトリ側で何を用意すれば
サイクルが速く・省トークンになるかをまとめたドキュメント。

> **[`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) との違い**
> あちらは「LLM が **toylang でコードを書く** ときのループ」を速くする
> **言語機能** の設計。本ドキュメントは「LLM が **Rust でコンパイラを書く**
> ときのループ」を速くする **リポジトリ運用** の設計。対象読者も成果物も別。
> ただし原理は同じで、`往復回数 × 1 往復のコスト` を下げる話に帰着する。

## Status snapshot

| ID | Scope | Status |
|---|---|---|
| **D1** | テスト出力を failure-first に | ✅ 2026-08-09 |
| **D2** | 黙って効いていない設定の修正 + flaky test 解消 | ✅ 2026-08-09 |
| **D3** | 横断的な設定構造体の `#[non_exhaustive]` 化 | ✅ 2026-08-09 |
| **D4** | `CLAUDE.md` の Commands 節を検証済みの内容に更新 | ✅ 2026-08-09 |
| **D5** | 「関心事 → 実装サイト」マップ / `CLAUDE.md` から履歴を分離 | 未着手 |
| **D6** | `--all-backends` 実行 + stdin 入力 | 未着手 |
| **D7** | 意味論変更時の cross-backend 検証の機械的強制 | ✅ 2026-08-09 |

---

## 原理 — コンパイラ開発では「出力量」が支配的になる

言語ユーザのループと違い、コンパイラ開発では**実行速度は既に十分**なことが多い。
このリポジトリの全テストは **4.3 秒**で終わる。にもかかわらずループが重い理由は、
1 往復あたりに読まされる**テキスト量**にある。

LLM のコンテキストは有限で、しかも読んだ分だけ課金される。
「全テスト PASS の 1634 行」は情報量ゼロだが、コストは最大級に高い。

したがって指針は 3 つ:

1. **既定の出力は「失敗だけ」にする** — 成功は要約 1 行で足りる
2. **変更が波及する範囲を可視化する** — 探索の grep 往復をなくす
3. **嘘をつく情報源をなくす** — 古いドキュメント・効いていない設定・flaky test は、
   間違った方向への往復を生む。**無情報より有害**

---

## 実測 (2026-08-09、本リポジトリでの 1 セッション)

| 観測 | 値 |
|---|---|
| グリーンな全体テスト実行の出力 | **1641 行** (うち 1634 行は無情報な PASS) |
| 同・実行時間 | 4.3 秒 |
| ビルドエラー 1 件の既定出力 | 11 行 (`--message-format=short` なら 3 行) |
| `CLAUDE.md` | 45 KB (毎セッション読み込まれる) |
| `design-docs/todo.md` | 124 KB (**読もうとして出力上限に到達**) |
| `SourceLocation` に 1 フィールド追加 → 壊れた構造体リテラル | 10 箇所以上 |
| 同上で発生した clippy 警告 | **206 件** (`result_large_err` 閾値超過) |
| `f` バグ (レキシカルスコープ) の修正に必要だった箇所 | **4 箇所 / 3 バックエンド** |
| flaky test 1 件が消費した調査 | 1 往復まるごと |

---

## 実施済み

### D1 — テスト出力を failure-first に (✅)

**問題**: グリーンな全体実行が 1641 行。毎回 `| grep -E "FAIL|Summary"` を
書いていた。実行自体は 4.3 秒なので、ボトルネックは速度ではなく出力量。

**対応**: `.config/nextest.toml` に

```toml
[profile.default]
status-level = "fail"
final-status-level = "fail"

[profile.verbose]
status-level = "all"
final-status-level = "all"
```

**効果**: グリーンな全体実行が **1641 行 → 7 行**。失敗時は従来どおり
全文が出る (一時的な失敗テストを植えて確認済み)。全テストの一覧が要る場面
(ハングの二分探索、フィルタが意図通りか確認) だけ `--profile verbose`。

**ビルド側**: `--message-format=short` で診断が
`path:line:col: error[CODE]: msg` の 1 行形式になる (既定は 1 エラー ~11 行)。
**位置情報は保持される**ので機械的に読む場面ではこちらが適している。
cargo の config / 環境変数では設定できないためフラグで渡す
(`CARGO_BUILD_MESSAGE_FORMAT` は効かないことを確認済み)。

### D2 — 黙って効いていない設定の修正 + flaky test 解消 (✅)

「嘘をつく情報源」を 3 件除去した。

**(a) nextest の環境変数設定が読まれていなかった**

`.config/nextest.toml` の `[profile.default.env]` は **nextest に存在しない
キー**。毎回 `ignoring unknown configuration keys` を出しながら、
`TOYLANG_CRANELIFT_OPT_LEVEL = "none"` は**適用されていなかった** —
テストは production 既定の `speed` で codegen していた。
同ファイルのコメントが記録していた測定値 (150s → 54s) も現状に適用されない
状態だった。`.cargo/config.toml` の `[env]` へ移動。

> warm 実行では `TOY_LINK_CACHE_DIR` が codegen ごと skip するため実害が
> 隠れていた。cold 実行と CI では効く。

**(b) 存在しない clippy lint 名**

ワークスペース lint の `collapsible_if_let` は実在しない (正しくは
`collapsible_if` / `collapsible_match`)。毎 clippy 実行に `unknown lint`
警告を出していた。削除して **clippy 無警告が既定状態**に。

> これは単なる掃除ではない。**「警告 0 が既定」でなければ、警告は情報として
> 機能しない。** 常時 N 件出ている状態では、自分の変更が 1 件足したかどうかを
> 判別できず、毎回 diff を取る往復が要る。

**(c) flaky test**

`interpreter/tests/contract_mode_tests.rs` の 7 テストが**同一の fixture
2 ファイルを並行 `fs::write`** していた。nextest はテストごとに別プロセスで
走るため、書き込み途中のファイルを読んで truncate されたプログラムをパースし
失敗しうる。各テストが `tempfile::TempDir` を所有する形に変更。
テストが毎回上書きしていて死んでいた追跡済み fixture 2 件も削除。

> **flake は発生頻度に比して不当に高くつく。** コストは「赤くなった 1 回」
> ではなく、「今の変更が壊したのか」を切り分ける調査。LLM は人間より
> 「自分の変更を疑う」側に倒れやすいため、正しい変更を巻き戻しかねない。

### D3 — 横断的な設定構造体の `#[non_exhaustive]` 化 (✅)

**問題**: `SourceLocation` に `end_offset` を 1 つ足しただけで、テスト・
examples を含む 10 箇所以上の構造体リテラルが壊れた。機械的パッチを 1 回
失敗もした (正規表現置換が壊れた形を生成)。

**対応**: `CompilerOptions` / `RunOptions` / `SourceLocation` を
`#[non_exhaustive]` にし、構築をコンストラクタ経由に統一 (18 箇所を書き換え)。

```rust
let mut options = CompilerOptions::new(input_path);
options.emit = EmitKind::Object;

let mut options = RunOptions::default();
options.jit = true;

let loc = SourceLocation::new(line, column, offset, end_offset);
let loc = SourceLocation::point(line, column, offset);   // 範囲不明のとき
```

**効果**: フィールド追加のコストが**定義側 1 箇所**に閉じる。
副次的に呼び出し側が短くなった — 変更するフィールドだけを書く形になるため
(`compiler/tests/e2e.rs` の 9 行リテラルが 3 行に)。

**適用基準**: 「フィールドが増えていく設定バッグ」に限る。
`input` のように既定値を持てない引数はコンストラクタの必須引数に残す
(`Default` の空 `PathBuf` が黙って使われる事故を防ぐため)。

### D4 — `CLAUDE.md` の Commands 節を検証済みの内容に更新 (✅)

**記載した全コマンドを実際に実行して検証した。** 主な変更:

- `cd <crate> && cargo ...` → `-p <crate>` (cd はツール実行時に確認を挟む)
- `PROPTEST_CASES=32` の前置指示を削除 — `.cargo/config.toml` の `[env]` 既定
  になっており、指示自体が古かった
- `--message-format=short` / `cargo check` を追加
- 環境変数の置き場所と「nextest の `[profile.*.env]` は存在しないキー」を明記
- **「横断的な変更をするとき」節を新設** (下記 D7 の規約)

**併せて判明した古い記録**: 「frontend のテストは `--release` 必須」という
運用メモがあったが、根拠として記録されていた理由 (debug + proptest 256 ケースが
遅い) は既に成立していない。実測で frontend 545 テストが debug で **1.07 秒**、
ワークスペース 1634 テストで **4.3 秒**。`--release` を付けると全クレートの
release ビルドが走るぶん、編集 → テストのループはむしろ遅くなる。

> **教訓**: 「速くするための指示」は前提が変われば「遅くするための指示」に
> 反転する。根拠 (why) を書いておかないと、前提が崩れたことに誰も気づけない。

---

## 未実施 (推奨)

### D5 — 「関心事 → 実装サイト」マップ / 履歴の分離

**問題**: `CLAUDE.md` (45 KB) と `todo.md` (124 KB) は「どのフェーズが
いつ landing したか」を詳細に記録しているが、作業中に繰り返し必要になるのは
**「name resolution はどこか」**。両者が同じ場所に混ざっている。

**提案**: `CLAUDE.md` から履歴を切り離し、常時ロードされる部分を
「場所を答える」情報に絞る。最小形は 1 つの表:

| 関心事 | 型検査 | tree-walker | lowering (AOT/IR VM/JIT) |
|---|---|---|---|
| bare-name 呼び出しの解決 | `type_checker/expression.rs::visit_call` | `evaluation/call.rs::evaluate_function_call` | `compiler_lower/src/call.rs::resolve_call_target` |
| ... | | | |

この表があれば `f` バグの調査は grep 数往復ではなく読み取り 1 回で済んだ。

### D6 — `--all-backends` 実行 + stdin 入力

**問題**: 1 つの `.t` を 3 バックエンドで確認するのに 3 コマンド・3 出力。
加えて小さなスクラッチプログラムを 20 個ほどファイルとして作った。

**提案**:

```bash
toy run --all-backends f.t     # interpreter / JIT / AOT を実行し不一致だけ報告
echo 'fn main() -> u64 { 0u64 }' | interpreter --check -
```

前者は実行 3 回 → 1 回、出力も「不一致があれば表示」に絞れる。
後者は Write ツールの往復を丸ごと消す。

### D7 — cross-backend 検証の機械的強制 (✅ 2026-08-09)

**問題 (実測)**: `f` バグは **3 バックエンドが同じ意味論を独立に実装**して
いるために 4 箇所の修正を要した。しかも**型チェッカだけ直した時点で、
型は通るが答えが間違う状態**になった。これに気づいたのは、たまたま実行する
テストを書いたからで、**何も「他のバックエンドも見ろ」と教えなかった**。

**当初案とその棄却**: 「意味論に関わるディレクトリを触った差分で
`consistency.rs` に変更が無ければ CI で警告」を最小案としていたが、採らなかった。
(1) このリポジトリに CI が無い、(2) 「ファイル X を編集していない」という
警告は誤検出が多く、無視される訓練にしかならない。

**実装**: `compiler/tests/example_consistency.rs` —
`interpreter/example/` の**全プログラムを 3 バックエンドで実行して突き合わせる**。
`assert_consistent` は「誰かがテストを書いたプログラム」しか守らないが、
この sweep は**書くことを忘れても効く**。example を足せばカバレッジが自動で増える。

- interpreter / JIT / AOT の exit code と stdout を比較
- 4 shard に分けて nextest で並列実行 (直列だと約 4 倍かかる)
- 実行時間: スイート全体で 4.3 秒 → **6.0 秒**

**skip リストは台帳であって mute ボタンではない**。`ERROR_EXAMPLES`
(意図的に失敗する例) と `AOT_UNSUPPORTED` (AOT 未対応) の 2 つを持つが、
**両方向に検査する**:

- リストにある example が動くようになったら失敗する → リストは縮む
- リストに無い example が壊れたら失敗する

これが無いと skip リストは「回帰が隠れる場所」になる。
`KNOWN_CRASHES` も同様 (現在は空)。

#### この機構が即座に見つけたもの

導入した最初の実行で、**バックエンドのクラッシュ 2 件**を検出した。

| 症状 | 原因 | 状態 |
|---|---|---|
| `float64.t` が AOT で `FunctionBuilder finalized, but block block0 is not sealed` | f64 の `%` は**意図的に非対応**で `Err` を返すが、`builder.finalize()` が `result?` より**前に無条件で**呼ばれており、未 seal・未 filled のまま cranelift の assertion に化けていた。明示的なエラーメッセージが依存ライブラリの assertion に置き換わっていた | ✅ 修正 (成功時のみ finalize) |
| `struct_field_error_test.t` が**両バックエンド**で `Field name not found in string interner` | `validate_struct_fields` が宣言済みフィールド名の interner lookup に `.expect` を使っていた。補間されていない名前は「提供されていない」だけなので、本来出すべき診断が出せる。フィールド欠落を示すための example が**コンパイラのクラッシュ**になっていた | ✅ 修正 (欠落として報告) |

さらに、sweep を並列実行したときだけ落ちる現象から**3 件目**が出た。

**JIT のコンパイル済み `main` キャッシュが `File` のポインタ同一性をキーに
していた。** プログラムを parse → drop → 別のを parse すると、アロケータが
**同じアドレスを再利用**し、2 番目のプログラムが 1 番目のコードを実行する。
1 プロセス 1 プログラムなら顕在化しないので生き延びていた
(コード中のコメントはこの危険を認識しつつ、verbose ログの都合としてのみ
扱っていた)。`File` に一意 ID を持たせてキーに使うよう修正。

> **この 3 件はどれも「意味論の変更」ではなく既存の潜在バグ**で、
> テストを書くことを誰も思いつかなかった領域にあった。
> D7 の狙いはまさにそこで、規約 (D4) では届かない。

## 原則のまとめ

新しいツール・テスト・ドキュメントを足すときの判断基準。

1. **既定の出力は失敗のみ。** 成功は要約 1 行。詳細は opt-in (`--profile verbose`)。
2. **警告 0 を既定状態に保つ。** 常時 N 件出ている警告は情報として死んでいる。
3. **決定論を最優先。** flake は発生頻度に比して不当に高くつく。
4. **ドキュメントには根拠 (why) を書く。** 前提が変わったときに気づけるように。
5. **ドキュメントのコマンドは書いたら実行して確かめる。** 動かないコマンドは、
   無いより悪い。
6. **横断的な変更は横断的に検証する。** 1 箇所直して「型が通った」は
   「正しい」ではない。
