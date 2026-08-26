# DEBUG_OBSERVABILITY.md — 失敗の「どこで」と「どう至ったか」を 4 実行系で同じに言う

backtrace / 行番号 / ソースファイル名という 3 つの要求を、
`ALLOCATOR_PLAN.md` / `MEMORY_PROFILING.md` と同じ手順
(現状調査 → 論点決定 → Phase 分割 → MVP 刻みで landing) で設計する。

この 3 つは別々の機能に見えて、**同じ 1 本の依存の上に乗っている** —
「位置」を、どの層まで、どの粒度で運ぶか。順序を決めずに着手すると
D2 を飛ばして D3 だけが landing し、**ファイル名の無い行番号**が
バックエンドに焼き付く (`MEMORY_PROFILING` M2 のサイト帰属が実際にそうなっている)。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **D0** | 出力形式の固定 + バックエンド間で診断を突き合わせるレーン | 📋 |
| **D1** | interpreter の backtrace の穴埋め (method / closure / `main` / 行番号 / 折り畳み) | 📋 |
| **D2** | `FileId` + `SourceMap` — 位置に「どのファイルか」を持たせる | 📋 |
| **D3** | IR の `SiteId` — 4 実行系すべてが panic 位置を言う (release でもコスト 0) | 📋 |
| **D4** | shadow stack (`-g`) — AOT / JIT の backtrace | 📋 |
| **D5** | ユーザから触れる API と機械可読出力 | 📋 |
| **D6** | 再帰深度の診断 / stdlib の境界チェック | 📋 |

---

## なぜ設計文書が要るか

**同じ意味論を 4 実行系が独立に実装している** (tree-walker / IR VM /
interpreter JIT / AOT + compiler JIT)。診断は数値と同じで、
**バックエンドごとに違うなら無い方がまし**である。下の実測 1 は、
その差が既に観測可能な形で存在することを示す。

さらに今回の 3 機能は `SourceLocation` → `LocationPool` → IR → ランタイム →
stdlib を**縦に貫く**。どこか 1 層で欠けると、上の層は嘘をつくか黙るかしかない。

---

## 現状調査 (2026-08-26 実測)

### 実測 1: panic 診断は実行系ごとに別物

```rust
fn c(n: u64) -> u64 { if n == 0u64 { panic("boom in c") } n - 1u64 }
fn b(n: u64) -> u64 { c(n) }
fn a(n: u64) -> u64 { b(n) }
fn main() -> u64 { a(0u64) }
```

| 実行系 | 出す情報 |
|---|---|
| tree-walker | `Error at p1.t:3:9:` + ソース抜粋 + caret + `backtrace: c / b / a` |
| IR VM | **メッセージのみ** (`VmResult::Diverged { message }`)。位置も backtrace も無い |
| interpreter JIT | `panic: boom in c` のみ (`jit_panic`、`interpreter/src/jit/runtime.rs:341`) |
| AOT / compiler JIT | `panic: boom in c` のみ (`puts` + `exit(1)`、`compiler/src/codegen/mod.rs:1944`) |

`--all-backends` は両方の出力を並べて出すが、**食い違いを報告しない** —
比較しているのは終了コードと stdout であって stderr の診断ではない。
`compiler/tests/consistency/basics.rs:311` の trap テストも
「全バックエンドが非ゼロで終わる」までしか pin していない。

### 実測 2: 豊かな診断は「IR VM を捨てて tree-walker で再実行」の副産物

`interpreter/src/lib.rs:1096-1105` — IR VM が `Diverged` を返すと結果は
`None` になり、**捕らえていた stdout ごと捨てて tree-walker が同じプログラムを
最初から実行し直す**。実測 1 の豊かな出力は、この再実行が出している。

帰結が 3 つある。

1. **tree-walker が走らせられないプログラムは貧しい診断のまま**になる
   (`TREE-WALKER-NUM-W` / `TREE-WALKER-CONCRETE-IMPL` の範囲)。
2. **プログラムが 2 回走る**。stdout は捨てて replay されるので二重には
   出ない (実測済み) が、`io::random()` の系列や `io::read_file` は
   2 回目を踏む。`--profile=mem` のカウンタは `restore_profile` で
   明示的に巻き戻している — つまり**この危険は既に一度踏まれている**。
3. 診断の品質が**エンジン選択の副作用**になっていて、意図した設計ではない。

### 実測 3: backtrace に載るのは「トップレベル関数の直接呼び出し」だけ

```rust
struct S { v: i64 }
impl S { fn boom(&self) -> i64 { panic("method boom") } }
fn go(s: S) -> i64 { s.boom() }
fn main() -> u64 { val s = S { v: 1i64 } val r: i64 = go(s) 0u64 }
```

```
   = backtrace (innermost first):
       go
```

**`S::boom` が居ない。** `call_stack.push` はリポジトリ全体で 1 箇所
(`interpreter/src/evaluation/call.rs:823`)、`Expr::Call` の枝だけにある。
method / associated function / closure / `dyn` 経由の呼び出しは frame を
積まない。`main` も (エントリなので) 積まれない。

つまり**最も知りたい最内フレームが落ちている**。panic の位置自体は
`location` が持っているので「どの行か」は分かるが、
「その method をどの呼び出し形で入ったか」は消えている。

### 実測 4: `(called at line N)` は到達しないコード

`render_backtrace` (`interpreter/src/lib.rs:1148`) は
`frame.call_site` があれば行番号を添える。しかし
`AstBuilder::call_expr` (`frontend/src/ast/builder.rs:97-102`) は
引数リストの `ExprRef` に対して**明示的に `None` を積んでいる**:

```rust
let args_ref = self.expr_pool.add(Expr::ExprList(args));
self.location_pool.add_expr_location(None); // args_ref location
```

`evaluate_function_call` はその `args` の位置を `call_site` にしているので、
**常に `None`**。実測 1・3 の出力に行番号が 1 つも無いのはこれ。
1 行の修正で生き返る死にコードが、機能一覧の側では「実装済み」に見えている。

### 実測 5: 位置に file identity が無く、module の位置はそもそも運ばれない

```rust
pub struct SourceLocation { line, column, offset, end_offset }   // file が無い
pub struct Span            { line, column, offset, end_offset }  // JSON 側も同じ
```

- `ErrorFormatter::new(source, file)` は**単一のソース文字列**を受け取り、
  `line` でその文字列を引いて抜粋を描く。
- `module_integration.rs:988-1013` の `integrate()` は expr / stmt プールを
  追記コピーするが、**`location_pool` を追記していない**。よって統合された
  stdlib のノードは添字だけが伸び、位置は `None` になる。

今この 2 つは**互いの被害を隠している** — stdlib に位置が無いので、
単一ソース前提のフォーマッタが他人のファイルの行を描く事故が起きていない。
**D2 を飛ばして「stdlib フレームにも位置を付ける」を実装した瞬間、
`core/std/vec.t:88` の行番号がユーザのソースに対して描画される。**

### 実測 6: 再帰は畳まれず、無限再帰に診断が無い

深さ 7 の再帰は `f` が 7 行並ぶ (区別する情報が無い)。
無限再帰 (`fn f(n: u64) -> u64 { f(n + 1u64) }`) は **60 秒で無出力・タイムアウト** —
IR VM の `frames: Vec<CallFrame>` が伸び続けるだけで、
「stack overflow」も「recursion limit」も出ない。
ステップ予算 (`CHECK-NONTERMINATION`) は `--check` にしか無い。

### 実測 7: stdlib の範囲外アクセスがホストの Rust panic になる

```rust
fn main() -> u64 { var v: Vec<i64> = Vec::new() v.push(1i64) val x: i64 = v.get(5u64) 0u64 }
```

```
thread 'main' panicked at interpreter/src/ir_vm/mod.rs:221:32:
value not defined
```

`core/std/collections/vec.t:87` の `get` は**無チェック** (コメントにもそう書いてある)。
組み込み配列の添字は `RUNTIME-TRAP` で全バックエンド panic するのに、
`Vec` はそこから外れている。しかも失敗の出方が**ホストの内部 panic**なので、
toylang 側の位置も backtrace も一切残らない。

### 実測 8: 実行時の失敗は `--diagnostics=json` に載らない

`--diagnostics=json` を付けても panic はテキストのまま出る。
JSON は型検査 / パースの診断だけを扱う。LLM ループ (P1〜P7) の観点では、
**機械可読になっていないのは実行時の失敗だけ**という状態。

### 参考: 既にある足場

- **`LocationPool` + `ErrorFormatter`** — 位置から抜粋 + caret を描く経路は完成している
- **`HeapAlloc { site: u64 }`** (`compiler_ir/src/lib.rs:1013`) — `(line << 32) | column` を
  IR からランタイムまで運ぶ**前例**。AOT 側は `prof_site_for` で集計している。
  **ファイル名を持たない**という制限もそのまま前例になっている
- **`SourceLocation` は `#[non_exhaustive]` + `new` コンストラクタ**。
  構築箇所はワークスペース全体で **22** — フィールド追加は現実的なコスト
- **`__builtin_source_file/line/column` / `__builtin_dbg`** — パーサレベルの
  マクロで、AST に落ちた時点でただの定数 / ブロック。バックエンド非依存
- **`Terminator::Panic { message: DefaultSymbol }`** — 全バックエンドが通る単一の絞り
- **`--all-backends` / `compiler/tests/consistency/`** — 比較の器はある (中身が診断を見ていないだけ)

---

## 論点と決定

### 論点 1: 「ファイル」をどう表現するか

| 案 | 内容 | 評価 |
|---|---|---|
| A | `SourceLocation` に `file: FileId (u32)` を足し、`SourceMap` が `FileId → (path, source)` を持つ | **採用** |
| B | 位置はそのままで、`LocationPool` を「ファイルごとの区間」に分割して添字から逆引き | 統合順に依存する暗黙の対応表になる。壊れ方が静か |
| C | パス文字列を位置に直接持たせる | `SourceLocation` は `Copy` で AST 全体に敷き詰められる。却下 |

**決定: A**。`FileId(0)` = エントリファイルを既定にすれば、
既存の 22 箇所は `new` の呼び出しを変えずに済む
(`new_in(file, ..)` を足し、`new` は `FileId(0)` を入れる)。
`ErrorFormatter` は `SourceMap` を受け取る形に変え、`Span` に `file` (パス文字列) を足す。

**波及**: `.toycache` は `SourceLocation` を serde で保存するので
`FULL_AST_CACHE_SCHEMA_VERSION` を上げること。

### 論点 2: 位置を IR にどう運ぶか

`HeapAlloc` の `site: u64` を素直に真似ると**ファイル名を持てない**
(実測 5 の制限がそのまま増える)。

**決定**: `compiler_ir::Module` に**サイト表**を持たせ、IR 側は `SiteId(u32)` だけを運ぶ。

```rust
pub struct Site {
    pub file: u32,     // Module::files への添字
    pub line: u32,
    pub column: u32,
    pub func: u32,     // 囲んでいる関数 (shadow stack がこれを使う)
    pub snippet: Option<StrId>,  // panic / trap サイトにだけ入れる
}
```

- `Module::files: Vec<String>` / `Module::sites: Vec<Site>` を AOT では `.rodata` に出す。
- `Terminator::Panic { message, site }` / trap guard / `PanicAllocBudget` が `SiteId` を持つ。
- **既存の `HeapAlloc { site: u64 }` も `SiteId` に寄せる** — `--profile=mem` の
  リーク報告に初めてファイル名が付く (M2 の積み残しの解消)。

`snippet` を表に埋めるのは、コンパイル済みバイナリが実行時にソースを
読みに行かなくても抜粋を描けるようにするため。**実行環境に依存しない**
(ファイルが消えていても同じ出力)。panic しうるサイトの数だけなので容量も限定的。

### 論点 3: AOT / JIT の backtrace をどう取るか

| 案 | 仕組み | 得られるもの | happy path のコスト |
|---|---|---|---|
| A | **shadow stack**: 呼び出しごとに `SiteId` をスレッドローカル配列に積む | 関数名 + **呼び出し元の行** | 呼び出しあたり store 1 + inc/dec |
| B | **frame pointer unwind**: `preserve_frame_pointers` + 関数アドレス表で symbolize | 関数名のみ (行は別途 line table が要る) | ほぼ 0 |
| C | DWARF を吐いて外部ツールに任せる | gdb/lldb が読める | 0 (生成コストのみ) |

**決定: A を D4 で、B は release 向けの将来オプション、C は非目標。**

理由: A は**インタプリタの出力と構造的に同じもの**を出せる
(frame ごとに呼び出し元サイトを持つ) ので、4 実行系で 1 つの文言に
できる。B は名前しか出ず、行を足すには結局 line table (= C の一部) が要る。
cranelift 0.131 に `preserve_frame_pointers` はあるので B の道は塞がっていない。

深さ上限を設ける (既定 1024)。溢れたら `... N frames elided` と言う —
**黙って切らない**。

### 論点 4: 何が既定で、何がコストを持つか

**行番号とファイル名は静的**である。panic サイトの位置は
コンパイル時に確定していて、実行時に払うのは `.rodata` の数バイトだけ。
**backtrace だけがランタイムコストを持つ**。

| ビルド | 位置 (行 / ファイル / 抜粋) | backtrace | 契約 |
|---|---|---|---|
| 既定 (debug) | あり | あり (shadow stack) | あり |
| `--release` | **あり** | 無し | 無し |

`--release` で位置まで落とすのは筋が悪い (ゼロコストのものを削る) ので落とさない。
既存の `--release` = 契約を切る、と同じ軸に `-g` 相当を乗せる形にする。

### 論点 5: 出力形式は 1 つ

D0 で**先に**文言を固定し、`compiler/tests/consistency/` に
`assert_diagnostic_consistent(source, stem)` を足す
(終了コードだけでなく **stderr を正規化して突き合わせる**)。
これを最初にやらないと、以降の Phase が「バックエンドごとに違う診断」を量産する。

正規化で落とすもの: 絶対パス (ファイル名だけにする)、フレーム深さ上限の表記ゆれ。
落とさないもの: 行 / 桁 / メッセージ / フレームの並び。

### 論点 6: ユーザから触れる API をどこまで足すか

| API | 内容 | 判断 |
|---|---|---|
| `__builtin_source_file/line/column()` | 既存。パース時定数 | 変更なし |
| `__builtin_function_name() -> str` | 囲む関数名。パース時に確定する | **足す** (D5、コスト 0) |
| `__builtin_backtrace() -> str` | その場のスタックを文字列で得る | **足す** (D5)。shadow stack が無いビルドでは 1 フレーム分だけ返す |
| `#[track_caller]` 相当 | assert ヘルパの呼び出し元を指す | **非目標**。属性構文自体が無い |
| 例外 / catch | — | **非目標** (`CLAUDE.md` の言語方針) |

---

## Phase 分割

### D0 — 文言の固定と比較レーン

- 4 実行系が出す panic 診断の**目標文言**をこの文書に書き、
  `assert_diagnostic_consistent` を `harness.rs` に足す。
- 現状の食い違い (実測 1) を **failing test として先に置く**か、
  D3 まで `#[ignore]` で置くかを決める。前者を推す —
  「今どれだけずれているか」がテスト出力に出る。
- 受け入れ基準: 新レーンが実測 1 のプログラムで**落ちる**こと。

### D1 — interpreter の backtrace の穴埋め (依存なし・低コスト)

1. `call_expr` が引数リストにも位置を積む → `(called at line N)` が生き返る (実測 4)。
2. `call_stack.push` を method / associated function / closure / `dyn` 経路にも置く (実測 3)。
   frame 名は `S::boom` のように**型名で修飾する**。
3. `main` を frame として積む。
4. 同一 (関数, サイト) の連続フレームを折り畳む: `f (×7, called at line 3)` (実測 6)。
5. 深さ上限 + `... N frames elided`。

受け入れ基準: 実測 3 のプログラムが `S::boom` → `go` → `main` を行番号付きで出す。

### D2 — `FileId` と `SourceMap`

1. `SourceLocation` に `file: FileId`、`new_in` を追加 (`new` は `FileId(0)`)。
2. `SourceMap { files: Vec<(PathBuf, Rc<str>)> }` を frontend に置き、
   パーサ / `ModuleResolver` が 1 ファイル 1 id を割り当てる。
3. **`integrate()` が `location_pool` を追記する** (実測 5)。位置は
   自分のファイル内の絶対位置のままでよい — `FileId` が曖昧さを取る。
4. `ErrorFormatter` を `SourceMap` ベースに。`Span` に `file` を追加。
5. `FULL_AST_CACHE_SCHEMA_VERSION` を上げる。

受け入れ基準: stdlib の関数で失敗したとき、抜粋が
**`core/std/...` の実際の行**で描かれる。ユーザソースの同じ行番号ではない。

### D3 — IR の `SiteId` (ここで「行番号表示」が 4 実行系で揃う)

1. `compiler_ir::Module` に `files` / `sites`、`SiteId` を導入。
2. `Terminator::Panic` / RUNTIME-TRAP guard / `PanicAllocBudget` が `SiteId` を持つ。
   `HeapAlloc { site: u64 }` を `SiteId` に移行 (`--profile=mem` にファイル名が付く)。
3. IR VM: `VmResult::Diverged { message, site }` →
   **tree-walker の再実行に頼らずに**位置付き診断を出す (実測 2 の解消)。
4. AOT / compiler JIT: サイト表を `.rodata` に出し、`toy_panic(site_id, msg)` が
   `Error at file:line:col:` + 埋め込み抜粋を書く。
5. interpreter JIT: `jit_panic` に `SiteId` を渡す。

受け入れ基準: D0 のレーンが**位置まで**一致して通る。backtrace はまだ差がある。

### D4 — shadow stack (`-g`)

1. `toylang_rt` にスレッドローカルの `SiteId` 配列 + 深さ。
2. codegen が呼び出しの前後で push/pop を出す (`-g` のときだけ)。
3. `toy_panic` が shadow stack を D1 と同じ規則 (折り畳み・上限) で描く。
4. **コストを測って本文書に記録する** — `fib` / `example/` の代表 3 本で
   `-g` 有無の実行時間。5% を超えるなら push/pop の形を見直す。

受け入れ基準: D0 のレーンが backtrace まで含めて全レーン一致。
`--release` では backtrace 行が消え、位置は残る。

### D5 — ユーザ API と機械可読出力

1. `__builtin_function_name()`、`__builtin_backtrace()`。
2. `ContractViolation` にも backtrace を付ける (P6-2 の値キャプチャと並べる)。
3. `--diagnostics=json` に**実行時の失敗**を載せる (実測 8):
   `{ "severity": "error", "kind": "panic", "message", "span" { file, line, column }, "backtrace": [...] }`。
4. `--explain` に panic / trap のコード (`E0xxx`) を足すかを判断。

### D6 — 再帰深度と stdlib の境界

1. 再帰深度の上限と `[E00xx] recursion limit exceeded` (実測 6)。
   backtrace は折り畳み済みで出す。IR VM / tree-walker / AOT で同じ上限。
2. `Vec::get` / `String::get` 等に境界チェックを入れるか、
   `at()` (チェック有り) と `get_unchecked()` に分けるかを決める (実測 7)。
   **少なくともホストの Rust panic で落ちるのはやめる。**

---

## 非目標

- **DWARF 生成 / gdb・lldb 連携**、ステップ実行デバッガ。D4 の B 案が入れば
  ネイティブ側の名前解決は外部ツールでも可能になるが、今回の射程ではない。
- **例外機構** (`try` / `catch` / `throw`)。`CLAUDE.md` の言語方針どおり導入しない。
  backtrace は panic 経路の観測手段であって、回復手段ではない。
- **`#[track_caller]` 相当**。属性構文が無く、入れるなら言語機能の追加になる。
- **release ビルドでの backtrace**。論点 3 の B 案として道は残すが、
  今回は「release では位置だけ」を意図した設計とする。

---

## 関連

- [`LLM_FEEDBACK_LOOP.md`](LLM_FEEDBACK_LOOP.md) P6 — 本文書の出発点 (panic 位置 + backtrace の第 1 版)
- [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md) M2 — サイト帰属。D3 でファイル名が付く
- [`BACKEND.md`](BACKEND.md) / [`JIT.md`](JIT.md) — 各実行系の守備範囲
