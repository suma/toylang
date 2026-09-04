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
| **D0** | 出力形式の固定 + バックエンド間で診断を突き合わせるレーン | ✅ 2026-08-27 |
| **D1** | interpreter の backtrace の穴埋め (method / closure / `main` / 行番号 / 折り畳み) | ✅ 2026-08-27 |
| **D2** | `FileId` + `SourceMap` — 位置に「どのファイルか」を持たせる | ✅ 2026-08-27 |
| **D3** | IR の `SiteId` — 4 実行系すべてが panic 位置を言う (release でもコスト 0) | ✅ 2026-08-27 |
| **D4** | shadow stack — AOT / JIT の backtrace | ✅ 2026-08-27 |
| **D5** | ユーザから触れる API と機械可読出力 | ✅ 2026-08-27 |
| **D6** | 再帰深度の診断 / stdlib の境界チェック | ✅ 2026-08-27 |

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

### 実測 1: panic 診断は実行系ごとに別物 (D3 で位置まで一致)

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

### 実測 2: 豊かな診断は「IR VM を捨てて tree-walker で再実行」の副産物 (解消)

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

### 実測 5: 位置に file identity が無く、module の位置はそもそも運ばれない (D2 で解消)

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

### 実測 6: 再帰は畳まれず、無限再帰に診断が無い (D1 / D6 で解消)

深さ 7 の再帰は `f` が 7 行並ぶ (区別する情報が無い)。
無限再帰 (`fn f(n: u64) -> u64 { f(n + 1u64) }`) は **60 秒で無出力・タイムアウト** —
IR VM の `frames: Vec<CallFrame>` が伸び続けるだけで、
「stack overflow」も「recursion limit」も出ない。
ステップ予算 (`CHECK-NONTERMINATION`) は `--check` にしか無い。

### 実測 7: stdlib の範囲外アクセスがホストの Rust panic になる (D6 で解消)

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

### 実測 8: 実行時の失敗は `--diagnostics=json` に載らない (D5 で解消)

`--diagnostics=json` を付けても panic はテキストのまま出る。
JSON は型検査 / パースの診断だけを扱う。LLM ループ (P1〜P7) の観点では、
**機械可読になっていないのは実行時の失敗だけ**という状態。

### 実測 9: コンパイル済みバックエンドは panic を **stdout** に書く (2026-08-27、D0 で判明 / D3 で解消)

AOT / compiler JIT の panic 経路は `libc_puts`
(`compiler/src/codegen/mod.rs`) なので、診断が **fd 1** に出る。
つまりプログラム自身の出力の途中に診断が挟まる — リダイレクトすると
`prog > out.txt` の中に `panic: ...` が入り、`2>` では取れない。
tree-walker / IR VM / interpreter JIT は stderr。

`--all-backends` が stdout を比較しているので、**panic するプログラムでは
「診断の文言の違い」が「stdout の違い」として現れる**。これまで表面化して
いないのは、interpreter レーンが先に失敗して比較に到達しないため (実測 1)。

D0 のレーンは、この理由で**どのストリームに出たかを比較対象に含める**。
同じ文を別の fd に書く 2 つのエンジンは一致していない。

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
  ✅ 2026-08-27 landing: `HeapAlloc` / `HeapRealloc` (null リサイズ) が
  `Option<SiteId>` を持ち、リーク報告は `core/std/string.t:71:25` の形で
  ファイル名を出す。キーは相変わらず `(line << 32) | column` の packed
  位置 (全バックエンド共通) で、ファイル名は隣に持つだけ。

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

### D0 — 文言の固定と比較レーン ✅ (2026-08-27)

#### 目標文言

**実行時の失敗は、どの実行系でも次の 1 つの形で stderr に出す。**

```
Runtime error occurred:
Error at <file>:<line>:<column>:
   |
 N | <失敗した式を含むソース行>
   |   ^^^^ panic: <message>
   |
   = backtrace (innermost first):
       S::boom (called at line 12)
       go (called at line 20)
       main
```

決めたこと:

- **ストリームは stderr** (実測 9)。`puts` 経路は D3 で置き換える。
  プログラム自身の stdout に診断を混ぜない。
- **ヘッダ `Runtime error occurred:` は全実行系**。今は AOT /
  compiler JIT だけが欠いている。
- **message は tree-walker の文言が正**。最も情報量が多く、既存の
  テストが pin しているのもこちらなので、コンパイル側を寄せる。
  値を持つ文 (`u64 subtraction underflowed: 1 - 5`、
  `Contract violation: ... (with n = 0)`) は**値ごと**移す —
  「どの引数で破れたか」が契約の診断の中身であって、
  `panic: requires violation` は同じ情報を持たない。
- 例外は **`panic:` 接頭辞と位置を欠いている 2 件**で、ここだけは
  tree-walker 側を直す:

  | 失敗 | 目標文言 |
  |---|---|
  | `panic("boom in c")` | `panic: boom in c` |
  | `u64` underflow | `panic: u64 subtraction underflowed: 1 - 5` |
  | 0 除算 | `panic: integer division by zero` |
  | `MIN / -1` | `panic: integer division overflowed (most negative value divided by -1)` |
  | 配列の範囲外 | `panic: array index out of bounds: index 5, length 3` |
  | `requires` 違反 | ``Contract violation: `requires` clause #1 of function `f` evaluated to false (with n = 0)`` |

- backtrace は最内が先。**panic した関数自身も 1 フレーム**として載り
  (実測 3 で欠けているもの)、`main` で終わる。呼び出し元の行は
  `(called at line N)`、最内フレームは自身の位置が上の `Error at` に
  出ているので付けない。

#### 比較レーン

`compiler/tests/consistency/diagnostics.rs` + `harness.rs` の
`diagnostic_lanes` / `assert_diagnostic_consistent` /
`assert_diagnostic_report`。**5 レーン** — tree-walker / IR VM /
interpreter JIT / compiler JIT / AOT (実測 1 の 4 行のうち最後の行を
2 つに割った。同じ `toylang_rt` を共有しているという理由で片方を
省くと、codegen のバグがちょうどそこに落ちる)。

正規化で落とすもの: 一時ディレクトリのパス、行末の空白、前後の空行、
子プロセスの libtest 自身の出力。**落とさないもの**: ストリーム
(stdout / stderr)、行、桁、メッセージ、抜粋、フレームの並び。

実装上の要点が 2 つある。

- **process を殺すレーンは子プロセスで走らせる。** `jit_panic` と
  コンパイル済みランタイムの panic はどちらも `process::exit(1)` で
  終わるので、in-process では**テストバイナリごと落ちる**。
  interpreter JIT / compiler JIT の 2 レーンは、テストバイナリ自身を
  `--exact consistency::diagnostics::diagnostic_lane_child --nocapture`
  で再実行した子で走らせる (`--nocapture` は必須 — libtest の捕捉
  バッファは `exit` で捨てられる)。
- **IR VM レーンは「もし表に出たら」の文言**を報告する。実際には
  実測 2 の replay があるので端末には出ない。

#### 現状の食い違いは failing test にしなかった

設計時は failing test を推していたが、**pin して green** にした。
理由: `CLAUDE.md` が意図的に作った「グリーンなら 6 行」の実行結果が
本物の失敗を見つける唯一の手段で、恒常的に赤い suite はそれを潰す。
代わりに `assert_diagnostic_report` が**両方向に**検査する —
文言が動けば落ち、5 レーンが一致したときも
「`assert_diagnostic_consistent` に置き換えよ」と言って落ちる
(`example_consistency.rs` の skip リストと同じ流儀)。
pin されている 4 プログラム (panic / underflow / 配列範囲外 /
`requires` 違反) のテキストがそのまま D1〜D4 の作業リストになる。

受け入れ基準: 実測 1 のプログラムで 5 レーンの食い違いがテキストとして
出ること。→ 満たした。

### D1 — interpreter の backtrace の穴埋め ✅ (2026-08-27)

1. `call_expr` が引数リストにも位置を積む → `(called at line N)` が生き返る (実測 4)。
2. `call_stack.push` を method / associated function / closure / `dyn` 経路にも置く (実測 3)。
   frame 名は `S::boom` のように**型名で修飾する**。
3. `main` を frame として積む。
4. 同一 (関数, サイト) の連続フレームを折り畳む: `f (x7, called at line 3)` (実測 6)。
5. 深さ上限 + `... N frames elided`。

受け入れ基準: 実測 3 のプログラムが `S::boom` → `go` → `main` を行番号付きで出す。→ 満たした:

```
   = backtrace (innermost first):
       S::boom (called at line 3)
       go (called at line 6)
       main
```

実装で決めたこと:

- **frame を積む場所は「call 式の評価器」ではなく「user code に入る
  絞り」**。`call_method` / `call_associated_method` の 2 つに置くと、
  `s.boom()` / operator overload / `dyn` dispatch / drop glue /
  property checker が**まとめて**乗る。以前は `Expr::Call` の枝
  1 箇所だけだったので、最も知りたい最内フレームが落ちていた。
- **call site は明示の引数で運ぶ** (`call_method(.., call_site)`)。
  context に「次の call site」を持たせる案は、引数の評価中に現れる
  内側の呼び出しが先に消費するので**静かに壊れる**。9 箇所を
  コンパイラに列挙させる方を採った。
- method frame の型名は **receiver の実行時型**から取る
  (`Object::Struct { type_name }` / `EnumVariant` / primitive)。
  `dyn Trait` 越しの呼び出しが「どの impl が走ったか」を言うのは
  この選択の結果。
- **closure の frame 名は呼び出しに書かれた束縛名** (`f`)。
  fn か closure かは、その瞬間の読者が必要としない区別。
- **pop は成功時のみ**。panic は最上位まで巻き戻るので、失敗時点の
  スタックがそのまま報告に要るもの (既存の 1 箇所の方針を踏襲)。
- 折り畳みは `(関数, 呼び出し行)` が同じ**連続**フレームのみ。
  深さ上限は**折り畳んだ後**の 10 + 5 行で、超えた分は
  `... N frames elided`。tree-walker の `max_call_depth` が 30 なので
  今は相互再帰でしか発火しないが、描画は D4 の shadow stack (1024) と
  共有する。

**やらなかったこと**: 契約違反 (`ContractViolation`) は
`InterpreterError::Panic` ではないので依然 backtrace を持たない。
frame は積まれているので運ぶだけだが、エラー型に触るので D5 で
機械可読化と一緒にやる。

### D2 — `FileId` と `SourceMap` ✅ (2026-08-27)

1. `SourceLocation` に `file: FileId`、`new_in` / `in_file` を追加
   (`new` は `FileId::ENTRY`)。
2. `SourceMap` を `frontend/src/source_map.rs` に置き、
   `File` が 1 つ持つ。
3. **`integrate()` が `location_pool` を追記する** (実測 5)。位置は
   自分のファイル内の絶対位置のままで、`in_file` で id だけ付け替える。
4. `ErrorFormatter` を `SourceMap` ベースに。`Span` に `file` を追加。
5. `FULL_AST_CACHE_SCHEMA_VERSION` を 20 → 21。

受け入れ基準: stdlib の関数で失敗したとき、抜粋が
**`core/std/...` の実際の行**で描かれる。→ 満たした:

```
Runtime error occurred:
Error at core/std/option.t:57:29:
   |
57 |             Option::None => panic("Option::unwrap on None"),
   |                             ^^^^^ panic: Option::unwrap on None
   |
   = backtrace (innermost first):
       Option::unwrap (called at line 4)
       main
```

実装で決めたこと:

- **`SourceMap` はテキストを所有する** (借用しない)。エントリの
  ソースは実行中ずっと生きているが、モジュールのそれは読んで
  parse して integrate した時点で捨てられる — 抜粋を描きたいのは
  そのあとである。
- **パーサが entry スロットにテキストを入れる。** パーサはパスを
  知らない (与えられるのは文字列だけ) ので名前は空のまま、
  ドライバ (`check_typing_diagnostics`) が名付ける。テキストを
  ここで運ぶことの効き目は **warm cache** に出る: `.toycache` から
  復元したモジュールは誰もファイルを読み直さないので、
  `File.source_map` が唯一のテキストの出どころになる。
  cold / warm が同じ抜粋を出すことは実測した。
- **エントリファイルの名前はフォーマッタの呼び出し側が決める。**
  `ErrorFormatter::with_source_map(source, filename, map)` の
  `filename` が `FileId::ENTRY` を名指し、map はそれ以外
  (= import されたファイル、呼び出し側が渡しようのないもの) に
  答える。map 側にも entry のパスは入っているが、**2 つの名前が
  争う**状況を作らないためにこの順にした (consistency harness が
  型検査時 `test.t` / 実行時 `<stem>.t` と 2 通りに名乗っていて、
  最初の実装はそれで揺れた)。
- **core モジュールの表示名は modules root からの相対パス**
  (`core/std/option.t`)。絶対パスはマシンごとに違う文字列を
  診断に埋めるので、同じプログラムの診断が環境で変わる。
- `Span::file` は **JSON に出さない**。`FileId` はこのプログラムの
  `SourceMap` への添字でしかなく、外の読み手には意味がない。
  パスを載せるのは D5 (実行時診断の機械可読化) と一緒にやる。

**副産物**: prelude が `<module>` という名無しで整合されていたのを
`<prelude>` に。あと、EOF の位置は「最終行 + 1」を指すことがある
(パーサが `input_len` に錨を打つ) — D2 のテストが最初に落ちた理由で、
バグではないが知っておく値打ちがある。

### D3 — IR の `SiteId` ✅ (2026-08-27)

1. `compiler_ir` に `Site` / `SiteId` と `Module::files` / `sites`。
2. `Terminator::Panic` / RUNTIME-TRAP guard / `PanicAllocBudget` が `SiteId` を持つ。
3. IR VM: `VmResult::Diverged { message, site }` で**自分で**位置付き診断を出す。
4. AOT / compiler JIT: **診断を丸ごと `.rodata` に置き**、
   `toy_panic_at(text)` が **stderr** に書いて exit する。
5. interpreter JIT: `jit_panic` にフレームの前後半を渡す。

受け入れ基準: D0 のレーンが**位置まで**一致して通る。→ 満たした。
5 レーンすべてが `Error at f.t:2:38:` + 抜粋 + caret + `panic: ...` を
**stderr** に出す。残る差は backtrace (D4) と、値を持つ 2 つの文言
(`1 - 5` / 契約の実引数) だけ。

実装で決めたこと:

- **サイト表ではなく「描画済みテキスト」を `.rodata` に置いた。**
  `Terminator::Panic` の message は interned literal で、位置は
  コンパイル時に確定している — **診断全体が静的**なので、実行時に
  組み立てるものが何も無い。サイト表 + フォーマッタをランタイムに
  持たせるより小さく、速く、そして「コンパイル済みバイナリが実行時に
  ソースを読みに行かない」という論点 3 の要求をそのまま満たす。
  `Site::snippet` が運ぶのはそのためのソース行。
- **例外は `PanicAllocBudget` だけ** — 数値が実行時にしか分からない。
  フレームを **prefix / suffix の 2 つの静的ブロブ**に割り、
  `toy_panic_alloc_budget(..., prefix, suffix)` が間に計算した文を書く。
  `compiler_ir` の unit test が「prefix + message + suffix ==
  render_stderr_text」を pin している (C ABI 越しに崩れると気づけないため)。
- **`puts` をやめて stderr に**した (実測 9)。AOT の panic が
  プログラムの stdout を汚さなくなり、`2>` で取れるようになった。
  `e2e.rs` は stdout が**空である**ことも確認する。
- **フレームの描画は 1 箇所** (`compiler_ir::format_diagnostic_frame`)。
  interpreter の `ErrorFormatter` もこれを呼ぶ — この書式が 2 箇所に
  あったことが、そもそも診断が実行系ごとに割れた原因だった。
- **二項演算の trap は「左オペランドの位置」**に付ける。式全体の方が
  caret としては良いが、tree-walker が昔からそこを指しており、
  D0 が正としたのは tree-walker の文言。エンジンごとに違う caret を
  作るより合わせる方を採った。
- **`panic: ` の接頭辞は VM の終端子ではなく消費側**で付ける。
  同じコードを CTFE の fold が共有していて、そちらは
  `[E0017] ... evaluating it failed: <message>` と自分の言葉で報告するため。

**やらなかったこと** (理由つき):

- **`HeapAlloc { site: u64 }` の `SiteId` 移行** — `--profile=mem` の
  リーク報告にファイル名を付ける件。診断とは独立の経路で、
  コンパイル済みランタイムが自前でレポートを書く (`TOY_PROFILE_MEM=1`)
  ため、ファイル名表をバイナリに出して起動時に登録する仕組みと、
  MEMORY_PROFILING M4 の JSON スキーマ変更が要る。D3 の受け入れ基準に
  は掛からないので分けた。
- **実測 2 の replay** — IR VM は自分で位置付き診断を出せるように
  なった (item 3 は満たした) が、interpreter は依然 diverge 時に
  tree-walker で再実行する。tree-walker の方が backtrace と契約の実値を
  持つからで、これを落とすのは VM が backtrace を出せる D4 と一緒。

### D4 — shadow stack ✅ (2026-08-27)

1. `toylang_rt` に frame ポインタ配列 + 深さ (`toy_shadow_stack` /
   `toy_shadow_depth`)。**スレッドローカルではなく素の static** —
   この言語にスレッドは無い。増えたらここが `#[thread_local]` になり、
   codegen のアドレッシングも一緒に変わる。
2. codegen が呼び出しの前後で push/pop を出す (`--release` では出さない)。
3. `toy_panic_at` / `toy_panic_alloc_budget` が shadow stack を D1 と
   同じ規則 (折り畳み・上限) で描く。
4. コストを測った (下記)。

受け入れ基準: D0 のレーンが backtrace まで含めて全レーン一致。→ 満たした。
`panic_three_calls_deep` / `panic_inside_the_stdlib` は
**5 レーン完全一致**になり、pin が両方向検査で「一致したので
`assert_diagnostic_consistent` に置き換えよ」と言って落ちた (D0 の設計どおり)。
`--release` では backtrace 行が消え、位置は残る (`e2e.rs` が pin)。

実装で決めたこと:

- **frame は呼び出し側で積む。** 積む中身は「呼ばれる関数の名前 +
  呼び出しの行」で、後者は callee には分からない。おかげで
  レンダリングは D1 の tree-walker と同形になる
  (`c (called at line 3)` → ... → `main`)。
- **`Instruction` に `frame: Option<FrameId>` を 1 つ足した。**
  call の IR variant は 11 個あり、そのどれかで push を忘れるのは
  「backtrace からフレームが 1 つ消えるが誰も気づかない」という、
  この Phase がまさに潰したい壊れ方。1 フィールド 1 箇所にすれば
  忘れようがない。IR の印字も変わらない。
- **名前は `Module::frame_name`** — `display_name` が set されていれば
  それ、無ければ export 名を戻す (`toy_` を剥がして `__` → `::`)。
  method は monomorph の型引数が名前に入るので **宣言時に
  `display_name` を set** している。
- **IR VM は shadow stack を使わない** — 自前の `frames` を持っているので、
  各フレームが「どの call で入ったか」を覚えるだけで済む。
- **interpreter JIT は同じ runtime globals を書く。** レンダラは 1 つ。
- **`main` は自分でフレームを積む** (誰も呼ばないので)。

#### コスト (実測、2026-08-27)

`--release` (frame 無し) を基準に、契約を持たないプログラムで比較。

| プログラム | 既定 | `--release` | 差 |
|---|---|---|---|
| `fib(32)` (呼び出しだけ) | 26.1 ms | 13.3 ms | **+96%** |
| collatz 30 万件 (再帰 + 算術) | 262 ms | 212 ms | **+24%** |
| `Vec` push/get 20 万件 (呼び出しが薄い) | 4.2 ms | 4.2 ms | **+1%** |

**5% の予算は呼び出し密度の高いコードでは達成できない。** 文書の指示
どおり push/pop の形を見直し、不変部分 (スロットのアドレス、深さの
2 値) を関数プロローグに巻き上げて呼び出しあたり 7 命令 → 2 ストアに
した — が、**測定値はほぼ変わらなかった** (26.08 → 25.93 ms)。
コストは算術ではなく、**グローバルへの store と、次の callee が
それを load することで生じる直列な依存鎖**だからで、これは
「callee が自分の深さを知る必要がある」という shadow stack の
定義そのものから出てくる。B 案 (frame pointer unwind) に替えても
行番号のために line table が要る (論点 3)。

なので**既定 on のまま**にした。debug ビルドの目的は診断であり、
「クラッシュしてから `-g` を付けて取り直す」は D1〜D4 が潰そうとした
失敗そのもの。速度が要る場面には `--release` がある。

**次に効く手** (未実装): **panic に到達しえない関数へのフレームは
積まない**。backtrace に現れようのないフレームは誰も読まない。
到達可能性の歩行は `effects.rs` に既にあるので、IR の
`Terminator::Panic` を sink にすれば同じ形で書ける。

### D5 — ユーザ API と機械可読出力 ✅ (2026-08-27)

1. `__builtin_function_name()`、`__builtin_backtrace()`。
2. `ContractViolation` にも位置と backtrace を付けた。
3. `--diagnostics=json` に**実行時の失敗**を載せた (実測 8 の解消)。
4. `--explain` にコードを 2 つ足した: **`E0019`** (panic / assert /
   RUNTIME-TRAP)、**`E0020`** (契約違反)。

実装で決めたこと:

- **`__builtin_function_name()` はパーサ置換** — `__builtin_source_*`
  と同じ。実行時コスト 0 で、名前は backtrace のフレームと同じ流儀
  (`S::boom`)。2 つが別々に名乗ったら、どちらを信じるかという
  問いが生まれてしまう。
- **`__builtin_backtrace()` だけは本物の builtin。** 自分の呼び出し
  スタックは走っているプログラムしか知らない。各エンジンが自前の
  スタック (tree-walker の frames / VM の frames / shadow stack) を
  読み、**1 つの共有フォーマッタ**で描く。
  `toylang_rt` 側は sink 抽象 (`ErrSink` / `ByteCounter` /
  `BufWriter`) を足して、stderr へ書く経路と `str` を作る経路で
  折り畳み規則を 2 度書かずに済ませた。長さは 2 パス — 一発の広めの
  malloc は深いスタックを黙って切るので採らない。
- **interpreter JIT はこの builtin を断る** (silent fallback)。
  shadow stack は持っているが str を作るヘルパが無い。tree-walker が
  答えるので観測可能な差は速度だけ。
- **JSON は既存の `Diagnostic` に相乗り**。`backtrace` を
  `skip_serializing_if = "Vec::is_empty"` で足したので、
  型検査の診断を読んでいるツールの形は変わらない。`file` は
  **失敗した側のファイル** (stdlib の panic なら stdlib) — D2 が
  位置に持たせた identity が、ここで初めて外に出る。
  `Span::file` は依然 JSON に出さない (`FileId` は外の読み手に
  意味がない)。
- **`InterpreterError::ContractViolation` を Box にした。** backtrace と
  位置を足したらこの enum が `Err` 型として大きくなりすぎ、clippy が
  127 箇所で鳴った。一番幅の広い variant を箱に入れるのが正解。

### D6 — 再帰深度と stdlib の境界 ✅ (2026-08-27)

1. 再帰深度の上限と `panic: recursion limit exceeded (N frames deep)`
   (実測 6)。backtrace は折り畳み済みで出す。
2. `Vec` / `String` の `get` / `set` / `pop` に境界チェックを入れた
   (実測 7)。

#### 「全実行系で同じ上限」は諦めた (実測してから)

設計時は同じ上限を置くつもりだったが、**tree-walker のホストスタックが
深さ 200 で溢れる** (debug ビルドで実測。100 は通り 200 は
`fatal runtime error: stack overflow`)。既存の `max_call_depth = 30` は
паранойアではなく妥当な保守値だった。

同じ数字にするなら全実行系を 30 にするしかなく、それは
**IR VM や AOT なら走りきれる正当な再帰を落とす**。上限は言語の性質では
なく**そのエンジンが載っているスタックの性質**なので、
**文言は 1 つ、上限はエンジンごと**にした。数字を文言に入れているのは、
読み手が「自分のプログラムが深いのか、ループしているのか」を
判断できるようにするため。

| 実行系 | 上限 | 何に律速されるか |
|---|---|---|
| tree-walker | 30 | ホストスタック (1 toylang call = 1 host frame) |
| IR VM | 1024 | 無し (frame はヒープ) — 明示的に置いた |
| AOT / compiler JIT / interpreter JIT | 1024 | shadow stack の深さカウンタ |

`--release` は shadow stack を持たないのでカウンタも無く、
**無限再帰は今も SIGSEGV** (C と同じ)。契約や backtrace と同じ軸。

#### コスト

AOT の検査は **1 活性化につき 1 比較** — D4 のプロローグに既に深さが
レジスタで載っているので、そこに `icmp` と分岐を足すだけ。呼び出し
ごとではない。呼び出しを持たない関数 (= 再帰しえない) はプロローグ
ごと出ない。fib(32) で D4 の +88% が **+90%** になった (実測、+2 points)。
interpreter JIT だけは呼び出しごと (巻き上げるプロローグが無いため)。

#### stdlib の境界

`Vec::get` / `set` / `pop`、`String::get` / `set` / `pop` を
`panic` するようにした。**`get_unchecked()` との分割はしていない** —
組み込み配列は既に RUNTIME-TRAP で落ちるのに `Vec` だけが外れていた、
というのが実測 7 の中身で、まず既定を安全側に揃えるのが順番。
逃げ道は「速度が要ると実測してから」足す。`push` は生ポインタ経由で
書くので追加のチェックは掛からない。`Dict::get` は元から
`Option<V>` を返すので対象外。

**2026-09-04 (VEC-CONTRACTS) で「契約が先、panic は網」になった。**
`Vec` の 7 本 (`get` / `set` / `pop` / `insert` / `remove` /
`swap_remove` / `set_size`) は同じ条件を `requires` にも書いた。
checked ビルドではそちらが先に発火するので、文言は固定文字列ではなく
**破った値**を出す (`(with index = 5)`)。`panic` は消していない — 契約は
`--release` で落ちるので、消すと `Vec` だけが release で unchecked な
indexed read になり、組み込み配列が release でも guard を残す設計
([`GUARD_ELISION.md`](GUARD_ELISION.md)) と食い違う。

---

## 値を持つ文言 ✅ (2026-08-27)

D0 の目標表が「値ごと移す」と決めていた 3 件。位置と backtrace が
D3/D4 で揃ったあと、**5 レーンで唯一残っていた差**だった。

| 失敗 | 全実行系の文言 |
|---|---|
| `u64` underflow | `panic: u64 subtraction underflowed: 1 - 5` |
| 配列の範囲外 | `panic: array index out of bounds: index 5, length 3` |
| `requires` 違反 | ``Contract violation: `requires` clause #1 of function `f` evaluated to false (with n = 0)`` |

**pin 4 件すべてが `assert_diagnostic_consistent` になった** —
`assert_diagnostic_report` (両方向 pin) は呼ばれていない。
D0 で「一致したら落ちる」向きを入れておいたおかげで、3 件とも
「一致したので置き換えよ」というテスト失敗として現れた。

実装で決めたこと:

- **形の違う 2 つを別の仕組みにした。** trap は
  `Terminator::PanicValues { kind, a, b }` — 形が固定 (2 値) なので
  ランタイムヘルパ 1 本で足り、**compiler_lower を通らない
  interpreter JIT も同じシンボルを呼べる**。契約違反は
  `Terminator::PanicStr { message }` — 引数の数も型も関数ごとなので、
  **失敗ブロックで文字列補間と同じ `ToString` / `StrConcat` を使って
  組み立てる**。契約が満たされる限り 1 命令も走らない。
- **値を出すのは scalar 引数だけ**、という規則を**両エンジンに**入れた。
  lowering の制限としてではなく規則として — 片方が struct の
  フィールドを並べ、もう片方が省く診断は、どちらも出さない診断より悪い。
- **`panic: ` を付けるかは終端子が決める** (`needs_panic_prefix`)。
  契約違反はそれ自体が完結した文で、
  `panic: Contract violation: ...` は同じことを 2 度言う。
- 配列の範囲外は tree-walker 側も**位置を持つ panic**にした
  (以前は位置も backtrace も無い別のエラー型)。位置は
  `arr[i]` 全体 — 型検査が書き換えたノードは位置を持たないので、
  評価器に**式そのものの `ExprRef`** を渡すようにした。

**これで tree-walker への replay を落とせる** — 残していた理由は
「tree-walker だけが値を持つ文言を出す」ことだったので。実測 2 の
「プログラムが 2 回走る」はこれで着手可能になった (未実施)。

---

## tree-walker への replay を落とした ✅ (2026-08-27)

実測 2 の解消。IR VM が diverge したとき、interpreter は
**プログラム全体を tree-walker で走らせ直して**豊かな診断を得ていた。
D3/D4 と「値を持つ文言」で VM が位置も backtrace も値も自分で出せる
ようになったので、戻る理由が消えた。

- `run_main_via_ir_vm` の `Option<RcObject>` を **3 状態**
  (`Ran` / `Diverged` / `NotEligible`) に割った。`None` が
  「走らせられない」と「走って失敗した」を同じ顔で返していたのが元凶。
- `compiler_vm::Divergence` が message / site / frames を**構造のまま**
  運ぶ。replay がある間はレンダ済み文字列で足りていたが、いまはこれが
  `--diagnostics=json` の出どころでもある。
- 途中まで出た stdout は**プログラムの出力**として印字する
  (replay 前提の「捨てて再実行」ではなくなった)。

**ついでに出てきたこと** (どれも replay が隠していた):

- `INTERPRETER_CONTRACTS` は tree-walker のつまみで、VM は常に
  契約を検査していた。`off` が効いていたのは「VM が diverge →
  replay が契約なしで走る」という**偶然**。lowering の
  `release` に対応付け、半端な設定 (`pre` / `post`) は IR で
  表現できないので `NotEligible` にして tree-walker に渡す。
- closure のフレーム名が `main::closure_f_0` (合成関数名) だった。
  宣言時に `display_name` を束縛名に設定。間接呼び出し経路には
  `pending_frame_name` を足した。
- **`dyn` dispatch の thunk がフレームに出ていた。** `Function` に
  `hide_frame` を足して backtrace から外す — thunk は vtable の
  スロットに置くための配管で、読者が書いたのはその上の method。

**もう 1 つ隠れていた**: `ensures allocates(N)` 違反の文言。
tree-walker は `Contract violation: \`ensures\` clause #1 of function
\`leaky\`: retained 128 bytes, budget 0 bytes` と言うのに、VM と
コンパイル側は `panic: retained ...` しか言えなかった (replay が
隠していた)。静的な前半を `Terminator::PanicAllocBudget` の `head` で
運び、`.rodata` の prefix ブロブに足して閉じた。この節だけ
`(with ...)` の引数一覧を落とす — **実測値そのものが答え**で、
引数の値はそこに足すものが無く、しかも他のエンジンには出せない。

**残った差**: `dyn` 越しの method フレームは呼び出し行を持たない
(thunk からの呼び出しは合成なので site が無い)。tree-walker なら
`(called at line N)` が付く。VM とコンパイル側は一致している。

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
