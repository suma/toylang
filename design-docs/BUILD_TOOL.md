# BUILD TOOL — `toy` コマンド

> **状態: B0〜B4 landing 済み (2026-09-05)。** B5 (マニフェスト) は
> 「必要になってから」のまま。B2 は TEST_TOOL の T0/T2 まで
> (T1 = compiled レーンでの `test` は未着手)。
> 実装: [`toy/`](../toy) (`toy/src/main.rs` がサブコマンド、
> `toy/src/package.rs` が §D2 の規約探索)。
> 対象: **自分のモジュールを持つプログラム**をビルド・実行する手順。
> 前提の実装サイト: [`interpreter/src/main.rs`](../interpreter/src/main.rs)
> (`resolve_core_modules_dirs`、優先順位の連鎖)、
> [`compiler/src/main.rs`](../compiler/src/main.rs) (`--core-modules` /
> `--emit` / `-o`)、[`compiler/src/driver.rs`](../compiler/src/driver.rs)
> (`TOY_LINK_CACHE_DIR`)、
> [`interpreter/src/module_integration.rs`](../interpreter/src/module_integration.rs)
> (auto-load)。
> 関連: [`MODULE_SYSTEM.md`](MODULE_SYSTEM.md) (解決規則)、
> [`TEST_TOOL.md`](TEST_TOOL.md) (`toy test`)、
> [`INCREMENTAL_COMPILATION.md`](INCREMENTAL_COMPILATION.md) (`.toycache`)。
> 状態の正本: [`todo.md`](todo.md)。

## 1. なぜ — 12 モジュールのプログラムに 25 行のシェルスクリプトが要る

`poc/logsearch` は toylang で書かれた 5,000 行のプログラムで、
12 のモジュールを持つ。それをビルドする手順が**これ**である:

```sh
# poc/logsearch/refresh.sh — 25 行。毎回これを先に走らせる
root="$here/build/root"
rm -rf "$root"; mkdir -p "$root"
ln -s "$core/std"  "$root/std"        # stdlib を symlink で
ln -s "$here/src"  "$root/logsearch"  # 自分のモジュールも symlink で
```

```sh
./target/release/compiler --core-modules poc/logsearch/build/root \
    poc/logsearch/main.t --release -o /tmp/logread
```

**symlink 農場を手で作っている。** これは怠慢ではなく、
(B0 以前の) CLI で自分のモジュールを持つ唯一の方法だった。

> **2026-09-05 以降はこう書ける** (B0 / B1):
>
> ```sh
> toy build poc/logsearch --release          # 根は道具が組み立てる
> compiler --core-modules core --core-modules poc/logsearch/src \
>     poc/logsearch/main.t --release -o /tmp/logread   # 道具なしでも
> ```

### 原因は 1 つ — `--core-modules` は「追加」ではなく「置き換え」

`resolve_core_modules_dir` の優先順位は
`--core-modules` → `TOYLANG_CORE_MODULES` → 実行ファイル相対 (`<exe>/../../core`)
で、**最初に当たった 1 つだけ**が auto-load の根になる
([`interpreter/src/main.rs:31`](../interpreter/src/main.rs))。つまり:

> **自分のモジュールを指した瞬間に stdlib が消える。**

だから両方を含む 1 つのディレクトリを自分で用意することになる。
`refresh.sh` の正体はそれだけで、**この 1 行の制約が 25 行になっている**。

### 隣接する 3 つの穴 (すべて実測)

| # | 症状 | 出典 |
|---|---|---|
| 1 | **エントリを根の中に置けない** — `src/main.t` を根に入れると auto-load で二重取り込みになり、複製が自分の `const` を失って `[E0003] Identifier 'BUF_BYTES' not found` | todo の ENTRY-IN-MODULE-ROOT |
| 2 | **bare 名が全モジュールで 1 つの名前空間** — `fn is_digit` (`pub` でない) が `std::json::is_digit` と衝突して `[E0010] ambiguous module path` | todo の BARE-NAME-COLLISION |
| 3 | **クエリ系が `--core-modules` を落とす** — `--effects` は指定を捨てて既定に倒れるので、ユーザのモジュールを持つプログラムには使えない | todo の EFFECTS-CORE-MODULES |
| 4 | **`main` の無いファイルの AOT が別のエラーを出す** — `compile error: \`log::level_from_rank\` is neither a variant of \`Level\` nor an associated function returning it` (実際の原因は「main が無い」)。テストファイルをコンパイルすると必ず踏む | 本文書、2026-09-05 実測 |

1 と 4 は「テストをどこに置くか」に直接効く ([`TEST_TOOL.md`](TEST_TOOL.md))。

## 2. 速さは問題ではない

先に測っておく。**遅いから道具が要るのではない。**

| 対象 | 実測 (release ビルド、warm) |
|---|---|
| 空に近いプログラムの AOT | **90 ms** |
| 同上、`TOY_LINK_CACHE_DIR` が効いたとき | **30 ms** |
| `poc/logsearch` (5,067 行 + stdlib) の AOT | **170 ms** |
| 同じファイルをインタプリタで実行 | 30 ms |

90 → 30 ms の差 60 ms は `cc` の呼び出しである。つまり**リンクキャッシュは
既にあり、効き幅も分かっていて、ただ既定で有効になっていない**
(環境変数を知っている人だけが速い)。道具の仕事は速度ではなく:

1. **モジュールの根を人間に作らせない**
2. **同じ根をビルド・実行・テスト・クエリのすべてに配る**
3. **既に在る速い経路 (`.toycache` / リンクキャッシュ) を既定にする**

## 3. 設計

### D1. 先に処理系を直す — 根は複数取れるべき

道具で symlink 農場を隠すこともできる (`.toycache/root/` に作る) が、
**それは refresh.sh を移動しただけ**で、直接 CLI を叩く人には何も
効かない。この repo は LLM が CLI を直接叩く前提で作ってあり
([`COMPILER_DEV_LOOP.md`](COMPILER_DEV_LOOP.md))、道具越しでしか
使えない機能を増やすのは筋が悪い。

```
compiler --core-modules <stdlib> --core-modules <pkg>/src prog.t -o prog
```

**`--core-modules` を繰り返し指定可能にする** (指定順に探索、後勝ちで
名前衝突を検出する)。`resolve_core_modules_dir` は
`Option<PathBuf>` → `Vec<PathBuf>` になり、既定の 1 つは
「明示指定が 1 つも無いときだけ」入る。これで:

- `refresh.sh` は**消える** (道具無しでも書ける)
- 道具は「根の並びを組み立てて渡すだけ」の薄い層になる
- 既存の 1 個指定は**そのまま動く** (`Vec` の長さ 1)

同時に**エントリの重複取り込みを絶つ**: auto-load の walker が、
コンパイル対象そのものと同じ正規化パスのファイルを見つけたら
飛ばす。上表の 1 が消え、`src/main.t` が普通に書けるようになる。

### D2. 規約が先、マニフェストは後

```
mypkg/
  main.t         エントリ (プログラム。src/ の中でも外でもよい)
  src/           モジュール。src/foo.t -> foo::
  tests/         テスト専用ファイル (TEST_TOOL.md)
  build/         出力。git に入れない
```

**`build/` の中身** (2026-09-05 に決めた):

```
build/
  .gitignore          "*" を初回に書く。出力はソースではない
  .link/              リンクキャッシュ (profile 共通)
  debug/
    mypkg             toy build の成果物 — 残す・配る側
    .run/mypkg        toy run --backend aot — 使い捨て
    tests/main        toy test (テストファイルごとに 1 本)
  release/
    ...               同じ形
```

3 つとも理由がある:

- **profile で分ける** — `--release` は契約を消すので debug と release は
  **別のプログラム**である。同じパスに置くと、ディスク上のファイルが
  どちらなのかを言わなくなり、しかも次のビルドで答えが変わる
- **`run` と `build` を分ける** — `build` の出力は「残す結果」、
  `run` の出力は使い捨て。ビルドしたバイナリを誰かに渡したあとで
  `toy run` して、渡した物が書き換わるのは事故
- **リンクキャッシュは profile 共通** — content-addressed
  (オブジェクトのバイト列が鍵) なので、debug と release は
  そもそも衝突しない

`toy` は引数のパス (既定はカレント) から**上に歩いて** `main.t` か
`src/` を持つディレクトリを探し、そこをパッケージ根とする。

**マニフェストは作らない。** 今のところ宣言することが無い —
名前は要らない (ディレクトリ名でよい)、バージョンは要らない
(依存が無い)、ビルドフラグは 3 つ (`--release` / `--backend` / `-o`)
しかない。**マニフェストは依存が来た日に来る**のであって、
その前に置くと「空のファイルを作らせる儀式」になる。

> 判断の根拠: Cargo の `Cargo.toml` が最初から要るのは、パッケージ名が
> クレート名になり crates.io の名前空間に属するから。toylang の
> モジュールパスは**ファイルシステムがそのまま**なので
> ([`MODULE_SYSTEM.md`](MODULE_SYSTEM.md))、宣言する先が無い。

### D3. サブコマンド

```
toy build [--release] [--backend aot|jit|vm] [-o PATH]
toy run   [--release] [--backend ...] [-- ARGS...]
toy check                         # 型検査だけ。コード生成をしない
toy test  [FILTER] [--backend all] [--check] [--seed=N]
toy api <module>                  # 既存の --api を根つきで
toy effects [FILE]                # 既存の --effects を根つきで (穴 3 の解消)
toy explain <CODE>
```

- **`toy run` の `--` 以降はプログラムの引数**。今は `RunOptions.args`
  への注入が CLI から見えにくく、`compiler` 側には無い
- **`--backend` の既定は `aot`** (`toy run` だけ `vm`)。
  `build` / `check` / `test` はどれも AOT を既定にする —
  `check` は型検査に加えて **lowering まで** やるので、
  型は通るが AOT が拒否する形 (式位置の compound 戻り method 等) を
  ここで捕まえる。`--backend vm` でより安い問いに落とせる。`vm` は
  IR VM = 既定のインタプリタ、`tree` は tree-walker
  (**オラクルが要る場面はこれ**、CLAUDE.md の注意書きと同じ)
- **`-v` は実際に走らせたコマンドを 1 行で出す。** 道具が処理系を
  隠さないための最低条件で、これがあれば「道具を捨てて手で叩く」に
  いつでも戻れる

### D4. 既定で速い

- `TOY_LINK_CACHE_DIR` を**既定で `build/.link/`** に向ける
  (90 ms → 30 ms が既定になる)
- `.toycache` は既に既定で効いている。道具は消さない・場所を変えない
- **常駐サーバは作らない。** [`PARALLEL_FRONTEND.md`](PARALLEL_FRONTEND.md)
  §5 が測っていて、プロセスあたり 7.2 ms のうち大半は
  統合済みスナップショット 1 ファイル (cold 0.95 ms / 319 KB) で落ちる。
  サーバの複雑さを払う理由が無い
- **`toy` は 1 バイナリで、compiler / interpreter を crate として使う**
  (プロセスを起こさない)。テストを 50 ファイル走らせるとき、
  プロセス固定費 ~30 ms × 50 = 1.5 秒がまるごと消える

### D5. 名前の衝突は道具が先に言う

穴 2 (BARE-NAME-COLLISION) は**処理系側の設計判断**が要るので、
道具は解決しない。ただし**検出はできる**: 根の並びを組み立てる時点で
リーフ名の重複が分かるので、コンパイル前に

```
warning: `is_digit` is defined in both src/record.t and <stdlib>/std/json.t
  a bare call resolves to neither -- qualify it, or rename one
```

と言える。処理系が `[E0010] ambiguous module path` を出すのは
**呼び出しに到達したとき**なので、道具の方が早い。

## 4. フェーズ

| | 内容 | 依存 | 状態 |
|---|---|---|---|
| **B0** | `--core-modules` を複数指定可能に + エントリの二重取り込みを飛ばす | 処理系。これだけで `refresh.sh` が消える | ✅ 2026-09-05 |
| **B1** | `toy build` / `run` / `check` (規約の探索、根の組み立て、リンクキャッシュ既定) | B0 | ✅ 2026-09-05 |
| **B2** | `toy test` | [`TEST_TOOL.md`](TEST_TOOL.md) の T0〜T2 | ✅ 2026-09-05 (T0〜T2)。**既定は AOT** |
| **B3** | クエリの通し (`api` / `effects` / `explain`) — 穴 3 の解消 | B1 | ✅ 2026-09-05 |
| **B4** | 衝突の事前検出 (D5) | B1 | ✅ 2026-09-05 |
| **B5** | マニフェストと依存 | **必要になってから** | — |

### B4 が最初に見つけたもの

**stdlib 自身が衝突している。** `encode` / `decode` が
`std::base64` と `std::hex` の両方にあり、**同じ rank なので
bare な `encode(...)` は今も曖昧**である。道具はこれを報告しない —
パッケージの側に直せるものが無く、毎回出て手も打てない警告は
読み飛ばしを教えるだけなので、**第 1 root (stdlib) の中だけで
閉じた重複は落としている**。言語側の台帳 (todo の
BARE-NAME-COLLISION) の話。

`poc/logsearch` に対しては 2 件出た — `decode` (`src/lsz.t`) と
`parse` (`src/record.t` vs `std::json`)。**これは穴 2 を直す
きっかけになった**: B0 が module パスに入れた「後の root が勝つ」を
bare 名にも適用し、パッケージの定義が stdlib を shadow するように
した (2026-09-05)。だから上の 2 件は**もう壊れない** — 警告は
「意図した shadow か」を尋ねるものに変わった。

### landing 時に分かったこと

- **穴 3 は道具の外でも直した。** `--effects` はクエリなので main の
  引数解析より前に走り、`--core-modules` を見ていなかった。argv から
  root を拾う 15 行で、`interpreter --effects` 単体でも自分のモジュールを
  持つプログラムに使える
- **`--backend tree` は受理するが今は `vm` と同じ経路に落ちる。**
  tree-walker を名指しで選ぶ入口が library API に無い
  (`execute_program` は適格性で選ぶ)。オラクルが要る場面のために
  名前は先に取ってあるが、**別物として効いているわけではない**
- **B0 の代償は 40 ファイル。** `RunOptions.core_modules_dir:
  Option<&Path>` → `core_modules_dirs: &[PathBuf]` が
  ワークスペース中のテストに波及した。機械的だが、
  設計文書が「`Vec` の長さ 1 でそのまま動く」と書いていたのは
  **意味論の話**であってソース互換の話ではなかった

**B0 だけでも価値がある。** `poc/logsearch` の 5 手順が 3 手順になり、
`refresh.sh` と `build/root/` が消える。B1 で 1 手順になる。

## 5. 非目標

- **パッケージレジストリ / バージョン解決** — 依存が 1 つも無い段階で
  設計すると、使われないまま形式だけ残る
- **クロスコンパイル** — AOT は baseline ISA 固定で可搬なので
  ([`SIMD.md`](SIMD.md))、まず必要になる場面が無い
- **ビルドサーバ / デーモン** — D4 の通り測って捨てた
- **処理系の隠蔽** — `toy` を使わない経路は常に等価に動くこと。
  道具は引数を組み立てるだけで、意味論を持たない
