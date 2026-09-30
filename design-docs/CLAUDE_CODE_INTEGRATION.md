# CLAUDE_CODE_INTEGRATION.md — Claude Code から toylang を呼び出す

**状態: 提案 (2026-09-30)。** Claude Code が **toylang のプログラム**を書く・
直すときに、`toy` の診断・修正・問い合わせを道具として使えるようにするには
何が要るかの検討。処理系自身の開発 (このリポジトリで Rust を書く作業) は
[`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md) の対象で、ここでは扱わない。

前提になる道具側の機能 (JSON 診断・`toy fix`・`toy query`・`--explain`) は
[`LLM_TOOLING.md`](LLM_TOOLING.md) で landing 済み。本書は**それを Claude
Code のどの口に、どう繋ぐか**を決める。

## 1. 現状 — Claude Code から見た toylang

今 Claude Code が toylang を扱う手段は **Bash で `toy` を叩く**ことだけで、
それを促すのは CLAUDE.md の文章だけ。実測と観察 (2026-09-30):

| 観察 | 何が困るか |
|---|---|
| `toy check` は release で 30 ms、`cargo run -q -p toy --` 経由で 250 ms | 速いが、呼ぶかどうかは Claude 任せ。編集のたびに自発的に呼ぶ保証がない |
| `toy` をリポジトリの外にコピーして環境変数なしで走らせると stdlib が見つからず、**`String::from_str` の型エラーとして**報告される | 「stdlib が無い」と言わないので、Claude は自分のコードの誤りだと思って直しにいく |
| Claude Code の組み込み `LSP` ツールは `.t` に対して `No LSP server available for file type: .t` を返す | 定義へ移動・参照・hover が使えない (`toy query` は同じことができるのに) |
| Bash の `toy ...` は許可設定が無いと毎回確認が出る | ループが止まる |
| テキストの診断は 1 件あたり ~11 行、JSON は 3 件で 1.7 KB | 編集直後に毎回差し込む用途には冗長 (後述 T3) |
| `toy check tests/basic.t` はそのファイルを入口として検査する | 編集したファイル単位で検査を掛けられる (これは良い) |

## 2. Claude Code 側の口 (2026-09-30 調査)

Claude Code のドキュメントの調査と、このセッションでの観察による。
**「要確認」は、実装時に公式ドキュメントで形を確かめること。**

| 口 | できること | toylang での使い道 | 確度 |
|---|---|---|---|
| **フック** (`PostToolUse`、matcher `Edit\|Write`) | 編集直後にコマンドを走らせ、結果を Claude に返す。exit 2 で stderr が Claude に届く / JSON の `additionalContext` 等 | `.t` を編集したら自動で `toy check` し、誤りを即座に差し戻す | 仕組みは高い。JSON 出力の欄の正確な位置は**要確認** |
| **LSP** (プラグインで登録) | 組み込み `LSP` ツールが goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls を呼ぶ (操作一覧は `LSP` ツールの定義で確認) | `toy query` をそのまま LSP で出す | 操作一覧は確認済み。**プラグインでの登録書式は要確認**。プロジェクト単位での単独登録は不可とされる |
| **MCP サーバ** (`.mcp.json`、stdio) | 独自ツール `mcp__<server>__<tool>` を足す。出力は既定 25k トークンまで | check / fix / query / explain / api を構造化された道具として出す | 高い |
| **スキル** (`.claude/skills/<name>/SKILL.md`) | 関係するときだけ本文を読み込む手引き。スクリプトを同梱できる | toylang の文法の要点と `toy` の使い方を、`.t` を扱うときだけ読ませる | 高い |
| **プラグイン** (`.claude-plugin/plugin.json`) | スキル・フック・MCP・LSP・エージェントを束ねて配る | 上の全部を 1 つにして、toylang を使う別リポジトリに入れる | 高い (manifest の細部は要確認) |
| **権限** (`permissions.allow`) | `Bash(<prefix> *)` 形式で確認なしに許す。**前方一致のみ** | `toy check` / `query` / `explain` / `api` を許可 | 高い |

**toylang にとって最も効くのは LSP**: 組み込み `LSP` ツールの操作は
`toy query` の type / def / refs / callers / callees とほぼ一対一で、
documentSymbol は `toy api`、goToImplementation は trait メソッド → impl
(QUERY-REST で内部には既にある) に当たる。サーバさえあれば、Claude は
新しい道具の使い方を覚えなくてよい。

## 3. 必要な機能

### 設定だけで効くもの (toylang 側の変更なし)

- **C1 権限** — `.claude/settings.json` に読むだけのコマンドを許可:
  `Bash(target/release/toy check *)` / `query` / `explain` / `api` /
  `effects`。`fix` と `run` / `test` は書き込み・実行なので既定では許可しない
- **C2 スキル** — `.claude/skills/toylang/SKILL.md`。CLAUDE.md の
  「Language Syntax」から**LLM が踏む罠だけ** (`elif`、`f64` サフィックス、
  セミコロンなし、`&` / `-` は行頭で新しい式、所有と `borrow`) と `toy` の
  使い方 (check → fix → query の順) を抜き出す。今の CLAUDE.md は処理系
  開発者向けで長く、toylang を**使う**側には不要な部分が多い

### toylang 側に要る機能

| # | 機能 | 内容 | 規模 |
|---|---|---|---|
| **T1** | stdlib が見つからないことを言う | stdlib の root が 1 つも無い / `core/std/prelude` が無いとき、型エラーではなく「stdlib が見つからない、`TOYLANG_CORE_MODULES` を設定するか `toy version -v` で確認」と言って止まる。あわせて**配置の方針**を決める (`cargo install` 先に stdlib を置く `share/toylang/core`、または stdlib をバイナリに埋め込む) | 小 |
| **T2** | `toy hook` | Claude Code のフックの入力 (stdin の JSON、`tool_input.file_path`) を読み、`.t` なら**そのファイルを**検査し、誤りがあれば exit 2 + 簡潔な要約を stderr に出す。シェルスクリプトと `jq` を不要にし、どの環境でも同じに動く | 小 |
| **T3** | 1 行 1 診断の簡潔な形式 | `--format=short`: `main.t:3:26: E0033 else if は elif (fix: elif)` のように、位置・コード・文言・machine-applicable な修正を 1 行で。フックで毎回差し込む量を小さくする (テキストの ~11 行、JSON の数百バイトに対して ~100 バイト) | 小 |
| **T4** | `toy lsp` | stdio の LSP サーバ。`publishDiagnostics` (パース + 型検査、`diagnose_parse_failure` 込み)、`definition` / `references` / `hover` / `callHierarchy` / `documentSymbol` / `workspaceSymbol` / `implementation` を `interpreter::query::Index` から答える。常駐するので stdlib のパース結果を保持でき、ファイル変更時は `forget_discovered_modules` と再検査。**エディタ (VS Code 等) にもそのまま効く** | 大 |
| **T5** | プラグイン | リポジトリに `claude-plugin/` を置き、T2 のフック、C2 のスキル、T4 の LSP 登録、C1 の権限の雛形を束ねる。toylang を使う**別のリポジトリ**が 1 つ入れれば済む形 | 中 (T4 次第) |
| T6 | `toy mcp` (任意) | MCP サーバとして check / fix / query / explain / api を出す。T4 と役割が重なるので、LSP に無い操作 (**fix の適用、explain、api、test**) を道具として構造化したい場合に限る。Bash 許可 (C1) で足りるなら作らない | 中 |

### 進捗

- **T1 (✅ 2026-09-30)** — `toy` はパッケージを見つける時点で stdlib の root を
  確かめ (`compiler::stdlib_problem`)、無ければ探した場所と直し方を言って止まる。
  stdlib を読まないコマンド (`explain` / `version` / `new` / `init`) は影響を受けない。
  `compiler` / `interpreter` は stdlib なしで走らせる用途があるので変えていない。
  **インストールの形**: `cargo install --path toy` は `~/.cargo/bin/toy` に入る
  ので、stdlib を `~/.cargo/share/toylang/core` に置く (実行ファイルから
  `../share/toylang/core` を探す)。あるいは `TOYLANG_CORE_MODULES` で指す

### 優先順

1. **T1** — 誤った場所に誘導する不具合なので最初に。小さい
2. **C1 + C2** — 設定とスキルだけで、呼ぶ手段と書き方の罠が揃う
3. **T2 + T3** — 編集のたびに検査が自動で走り、誤りが短く返る。
   「書く → 検査を呼ぶのを忘れる → 後で大量の誤り」を消す、最も効果が大きい一手
4. **T4** — Claude が既に持っている LSP ツールがそのまま使えるようになる。
   `toy query` の Index を流用できるので、実装の大半は LSP のプロトコル層
5. **T5** — T2・C2・T4 が揃ってから束ねる
6. T6 は T4 の後で、足りない操作が見えてから判断

## 4. フックの形 (T2 の設計メモ)

```jsonc
// .claude/settings.json (C1 と一緒に置く想定)
{
  "hooks": {
    "PostToolUse": [
      { "matcher": "Edit|Write",
        "hooks": [ { "type": "command", "command": "toy hook", "timeout": 10 } ] }
    ]
  }
}
```

`toy hook` の振る舞い:

- stdin の JSON から `tool_input.file_path` を取る。`.t` 以外なら何もせず exit 0
- そのファイルを入口に `diagnose` (パースエラーなら `diagnose_parse_failure`、
  それ以外は型検査。lowering はしない = `--backend vm` 相当で ~20 ms)
- 誤りが無ければ exit 0 で何も出さない (成功を毎回差し込まない)
- 誤りがあれば **exit 2**、stderr に T3 の形式で**最大 N 件** (N = 10 程度) と
  「全件は `toy check <file> --format=json`」の 1 行。machine-applicable な
  修正がある件は「`toy fix` で直る」と添える
- 警告 (E0025 等) だけなら exit 0 で JSON の `additionalContext` に載せる
  (形は**要確認**)

## 5. 非目標

- **処理系開発の支援** — このリポジトリで Rust を書く作業は CLAUDE.md と
  COMPILER_DEV_LOOP の管轄。本書のスキルやフックは `.t` の編集にだけ反応する
- **LSP を Claude Code 専用にする** — T4 は標準の LSP として作り、Claude Code は
  その利用者の 1 つ
- **フックで `toy fix` を自動適用する** — 編集を Claude の知らないところで
  書き換えると、次の Edit の `old_string` が合わなくなる。提案は差し戻すだけにし、
  適用は Claude が選ぶ (`toy fix` を Bash で呼ぶ)
