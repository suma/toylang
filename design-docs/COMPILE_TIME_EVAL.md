# COMPILE_TIME_EVAL.md — 関数をコンパイル時に走らせる (`const fn`)

C++ の `constexpr` / D の `pure` + `enum` / Rust の `const fn` / Zig の
`comptime` に相当する仕組みを、この言語に**どう入れるか**の設計検討。
動機は 2 つあり、片方は最適化、片方は Design by Contract である。

1. **最適化** — 定数引数の呼び出しはコンパイル時に済ませ、結果を埋める
2. **DbC** — 契約述語に「副作用が無く、コンパイル時にも評価できる」を
   要求できるようにし、定数引数の呼び出しでは `requires` を
   **実行時 panic ではなく型検査エラー**にする

`ALLOCATOR_PLAN.md` / `MEMORY_PROFILING.md` / `NEVER_ALLOCATES.md` と同じ手順
(現状調査 → 論点決定 → Phase 分割) で進める。

## Status snapshot

| Phase | Scope | Status |
|---|---|---|
| **C0** | 用語と意味論の固定 + バックエンド比較レーン | ✅ 2026-08-26 |
| **C1** | `const fn` の宣言と適格性検査 (評価はまだしない) | ✅ 2026-08-26 |
| **C2** | IR の定数畳み込み (CTFE に依存しない最適化) | ✅ 2026-08-26 |
| **C3** | driver 層の CTFE — 定数引数の呼び出しと `const` 初期化子 | ✅ 2026-08-26 |
| **C4** | DbC 接続 — 述語の純粋性強制 + 定数引数での `requires` 静的検査 | ✅ 2026-08-26 |
| **C5** | 型の中の値 — 配列長に `const` / `const fn` を許す | 📋 |
| **C6** | 評価器の一本化 — CTFE を IR VM に載せ替える | 📋 |

> **実装時に変えた決定が 1 つある** — 論点 6 の「定数同士の trap は
> コンパイルエラー」と「`if false { 1u64 / 0u64 }` はエラーにしない」は
> 到達可能性を知らない畳み込みでは両立しない。C3 は代わりに
> **強制位置 (forced) と任意位置 (opportunistic)** で線を引いた:
> `const NAME = ...` は値が無いと先に進めないので失敗はコンパイルエラー、
> ふつうの呼び出しの fold は最適化なので失敗したら**畳まないだけ**。
> C++ の `constexpr` と同じ規則で、`if false` の例はこちらに落ちる。
> 詳細は下の C3 節。

---

## なぜ設計文書が要るか

**この言語は既に同じ意味論を 4 実行系で実装している。CTFE は 5 つ目になりうる。**
`2u64 * 3u64` の答えがコンパイル時と実行時で違う言語にはしない、というのが
本設計の最上位の制約であり、難所は評価器を書くことではなく
**どの評価器を使うかを決めること**である。

そして下の実測 1・2 が示すとおり、**その分岐は既に一度踏まれている** —
const 初期化子の評価器が 2 つあり、能力が違い、同じプログラムが
インタプリタで動いて AOT でコンパイルできない。

---

## 現状調査 (2026-08-26 実測)

### 実測 1: 同じプログラムが interpreter で動き、AOT でコンパイルできない

```rust
fn double(n: u64) -> u64 { n * 2u64 }
const D: u64 = double(21u64)
fn main() -> u64 { D }
```

| 実行系 | 結果 |
|---|---|
| interpreter | **42** |
| JIT / AOT | `compiler MVP cannot evaluate the initialiser for `const D`: only literal values and references to earlier consts are supported` |

つまり「const 初期化子で関数を呼ぶ」は**もう半分実装されている**。
片側だけが。

> **✅ C3 で解消**。`const fn double` と書けば 4 実行系すべてで 42、
> 書かなければ 4 実行系すべてでコンパイルエラー (`E0017`)。

### 実測 2: const 評価器が 2 つあり、能力が違う

| 場所 | いつ | 何ができるか |
|---|---|---|
| `interpreter/src/lib.rs:1050` 付近 | **実行時** (起動時に 1 回) | 普通の式評価。関数呼び出しも通る |
| `compiler_lower/src/consts.rs` | **コンパイル時** (lowering) | リテラル / 先行 const / 単純な算術・比較・`!` のみ |

`CLAUDE.md` は const を「起動時に 1 回評価」と書いており、それは
tree-walker については正しい。IR を通る 3 実行系では**コンパイル時に畳まれている**。
能力差がそのまま実測 1 の食い違いである。

### 実測 3: IR に定数畳み込みが無い

```rust
fn main() -> u64 { 2u64 * 3u64 + 1u64 }
```

```
%v0: u64 = const 2u64
%v1: u64 = const 3u64
%v2: u64 = mul %v0, %v1
%v3: u64 = const 1u64
%v4: u64 = add %v2, %v3
ret %v4
```

畳まれていない。ネイティブでは cranelift の最適化に任せているが、
テストは `TOYLANG_CRANELIFT_OPT_LEVEL=none` で走り、**IR VM は毎回計算する**。
「最適化でコンパイル時に結果が埋まっている」は、現状**期待できない**。

### 実測 4: 型レベルの CTFE は既に 1 種類ある

```rust
struct P { x: i64, y: i64 }
__builtin_sizeof(p)   →   %v2: u64 = const 16u64
```

`__builtin_sizeof` は lowering で定数に畳まれている。
**コンパイル時評価の器は既にある** — 値レベルが無いだけ。

### 実測 5: 配列長はリテラルのみ

```rust
const N: u64 = 3u64
val a: [i64; N] = [1i64, 2i64, 3i64]
```

```
Expected array size or underscore
```

パーサが識別子を受け付けない。**CTFE の代表的な用途 (型の中の値) が
入口で塞がっている**。

### 実測 6: 契約述語に純粋性の要求が無く、副作用の出方がエンジン依存

```rust
fn noisy(n: u64) -> bool { println("checking") n > 0u64 }
fn f(n: u64) -> u64 requires noisy(n) { n }
fn main() -> u64 { f(3u64) }
```

| 実行 | 出力 |
|---|---|
| 既定 | `checking` |
| `INTERPRETER_CONTRACTS=off` | **`checking`** |

`INTERPRETER_CONTRACTS` は tree-walker (`evaluation/mod.rs`) しか読まない。
既定エンジンの IR VM は契約を lowering に埋め込んで実行するので、
**契約を切っても述語は走り、副作用は残る**。
`off` が効いているように見えるのは、違反して `Diverged` になったときに
tree-walker が再実行するからであって、述語の実行を止めているからではない
(この再実行の構造は [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) 実測 2 と同じもの)。

**DbC 側から見た本機能の動機はここが一番強い。** 契約は
「検査を切っても意味が変わらない」ものであるべきで、そのためには
述語が純粋である必要がある。今はそれを誰も要求していない。

### 実測 7: 静的解析の骨格は既にある

`frontend/src/type_checker/alloc_check.rs` (454 行) が `never_allocates` の
検査をしている: 呼び出しグラフの到達可能性、経路つきの診断、
追えない呼び出し (closure / `dyn` / `extern`) の拒否。

**`const fn` の適格性検査は、このパスの sink 集合を差し替えたもの**である。
新しい解析を書く話ではない。

### 参考: 既にある足場と、そこから決まる制約

- **`compiler_ir::Const` は scalar のみ** (`I64`/`U64`/narrow/`F64`/`Bool`)。
  → **MVP で `const fn` が返せる型の上限がここで決まる**。str / struct / enum は返せない
- **step budget** (`InterpreterError::StepBudgetExceeded`、`--check` 用) —
  停止性の逃げ道の前例がそのまま使える
- **RUNTIME-TRAP の guard は lowering にある** → IR を評価する限り
  0 除算・underflow・添字外の意味論はタダで一致する
- **`compiler` は既に `interpreter::check_typing_with_core_modules` を呼んでいる**
  (`compiler/src/jit.rs:184`)。frontend パイプラインが interpreter 経由なので、
  **driver 層に置いたパスは AOT からも interpreter からも通る**
- **クレート依存**: `frontend ← interpreter ← compiler`、`frontend ← compiler_lower ← interpreter`。
  **frontend からは評価器に手が届かない**。これが Phase 分割を決めている

---

## 論点と決定

### 論点 1: 何を保証する注釈にするか

| 言語 | 綴り | 意味 |
|---|---|---|
| C++ | `constexpr` / `consteval` | 「評価**しうる**」/「評価**しなければならない**」 |
| Rust | `const fn` | const 文脈で呼べる。純粋性は型システムが別途担保 |
| D | `pure` + `enum` 定数 | 純粋性の注釈と、強制的なコンパイル時評価が別の綴り |
| Zig | `comptime` | 引数・式・型に付く。言語の中心的な機構 |

**決定: `const fn` を 1 語だけ入れる。意味は C++ の `constexpr` 寄り
「コンパイル時に評価しうる」。**

- 綴りは**既存キーワード `const` の再利用**で、新しい予約語が要らない。
  `const fn f()` と `const NAME: T = ...` は次のトークンで曖昧さなく分かれる
- 位置は `never_allocates` と同じ前置修飾子: `pub const fn f(...) -> u64`
- **`pure` を別に立てない。** 純粋性は独立の軸だが、この言語では
  `const fn` の適格性検査 (論点 2 の sink 集合) が純粋性検査を内包するので、
  2 語に分けても保証が増えない。増やすのは覚えることだけ
- **`consteval` 相当 (必ずコンパイル時) は関数側では表現しない。**
  「必ず」は使う側の文脈 (`const NAME = f(1u64)`、配列長) が決めるべきもの

### 論点 2: どの評価器で走らせるか ← 最重要

| 案 | 内容 | 評価 |
|---|---|---|
| (a) | frontend に小さな AST 評価器を新設 | **却下**。5 つ目の意味論になる。`consts.rs` が既にその小型版で、実測 1 の食い違いを生んだ |
| (b) | **tree-walker を driver 層から呼ぶ** | **MVP に採用** |
| (c) | **IR VM をコンパイル時に走らせる** | **最終形**。ただしクレート抽出が要る |
| (d) | バックエンドごとに畳む | 却下。4 通りの答えを作る |

**決定: (b) で意味論を固定し、(c) を C6 の独立した refactor に切る。**

- (b) の根拠: `CLAUDE.md` が「オラクルが要る場面では
  `execute_program_tree_walking` を使う」と定めており、**CTFE はまさに
  オラクルが要る場面**。compiler は既に interpreter に依存しているので配線も済んでいる
- (c) が最終形である根拠: 同じ lowering・同じ trap guard・同じ IR を通るので、
  「コンパイル時と実行時で答えが違う」が**構造的に起こりえなくなる**。
  RUNTIME-TRAP の guard を二重に実装しなくてよいのも同じ理由
- (c) の費用: `interpreter/src/ir_vm/` は 3,428 行で、`crate::object::Object` /
  `crate::runtime_state::RuntimeState` / `crate::heap` に依存している。
  クレート抽出は本機能とは独立した作業なので、**本機能の前提にはしない**

### 論点 3: 評価が失敗したらどうなるか

| 事象 | 扱い |
|---|---|
| **trap** (0 除算 / underflow / 添字外) | **コンパイルエラー**。実行すれば必ず落ちるものを通す理由が無い。位置を出すには DEBUG-OBS D3 の `SiteId` が要る |
| **契約違反** | 同上。コンパイルエラー。P6-2 の値キャプチャがそのまま診断になる |
| **非停止** | **step budget** (既存の `StepBudgetExceeded` を再利用) → コンパイルエラー。「予算を使い切った」と言う、黙って諦めない |
| **確保** | **MVP は禁止**。結果が `Const` に載らない。検査は `never_allocates` と同じ sink |
| **extern / IO / `random` / `now`** | 禁止。適格性検査の sink |
| **f64 の `sin`/`cos`/`log`/`exp`** | **MVP は除外**。ホストの libm で評価することになる |

f64 transcendental の除外について。この言語は今 **host-only コンパイル**
(`cranelift_native::builder()`) なので、ホストとターゲットの libm の差は
今日は観測できない。しかし CTFE は**その依存を作る**側であり、
一度定数を焼き込むと後から cross-compilation を足すときに
「昔ビルドした値」と「ターゲットの値」が食い違う。**作らない方を選ぶ。**

### 論点 4: どの層で置換するか

**決定: 型検査の後・lowering の前に、driver 層で AST を書き換える。**

- 配置は `interpreter::module_integration` と同じ流儀 (interpreter クレートに置き、
  compiler からも呼ぶ)。既にその配線がある
- 利点: **バックエンドは砂糖を見ない**。`STRUCT-UPDATE` / `NEWTYPE` と同じ流儀で、
  4 実行系への配線がゼロ
- 欠点: 型検査より**後**なので、型の中の値 (配列長 `[i64; N]`) には届かない。
  そこは C5 で「frontend が評価器を trait で受け取る」形にして解く
  (frontend が `trait ConstEvaluator` を宣言し、driver が実装を注入する。
  依存の向きを変えずに評価器を貸せる唯一の形)

### 論点 5: DbC との接続

1. **契約述語は `const fn` しか呼べない、を段階的に強制する** (warn → error)。
   実測 6 の「契約を切っても述語の副作用が残る」が消え、
   `--release` / `INTERPRETER_CONTRACTS` が**本当に意味を変えない切り替え**になる。
   既存プログラムを壊すので 1 リリース分は警告に留める
2. **呼び出し引数がすべて定数なら `requires` をコンパイル時に検査する。**
   `half(3u64)` に `requires is_even(n)` があれば、実行時 panic ではなく
   型検査エラー。CONTRACT-ELISION が既に契約を静的に読んでいるので、
   読む場所が増えるわけではない
3. **`ensures` の CTFE はしない。** `result` が要る = 関数本体の CTFE と同じことになり、
   2 が通れば得られる情報は増えない
4. `--api` の出力に `const fn` を出す (`never_allocates` と同じ扱い)

### 論点 6: 畳み込みは意味論を変えてはならない

C2 の定数畳み込みは CTFE と独立に効くので先に入れられるが、規則を先に決める。

- **定数同士の trap はコンパイルエラー**にする (`1u64 / 0u64`)。
  実行時に必ず落ちるものを黙って畳んで別の値にするのは論外で、
  畳まずに残すのは診断の機会を捨てている
- ただし**畳み込みは分岐に踏み込まない**。`if false { 1u64 / 0u64 }` を
  コンパイルエラーにしてはいけない (到達しないので実行時には落ちない)。
  → **畳み込みは basic block ローカル、CTFE は呼び出し単位**という線を引く
- `+` / `*` の overflow は wrap する (RUNTIME-TRAP の規定)。
  **畳み込みも wrap する**。ホストの Rust で `wrapping_*` を使うこと

---

## Phase 分割

### C0 — 用語と意味論の固定 ✅

- `docs/language.md` の「`const fn` — evaluation while compiling」節が正本。
- `compiler/tests/consistency/const_eval.rs` が **「コンパイル時に畳んだ結果 ==
  実行時に計算した結果」**を検査する。1 つの body から 2 プログラムを組み
  (`const fn` + `const` 初期化子で畳ませた版 / 注釈なしで実行時に計算させた版)、
  各版を 4 レーンで一致させたうえで**両版の答えを突き合わせる**。
  ケースは手書き評価器が最もずれやすいところ — `+` / `*` の wrap、
  符号付き除算・剰余の truncation、narrow 幅の wrap、f64。
- **このレーンは空振りできない**: `compiler_lower::consts` は今も呼び出しを
  評価できないので、fold が起きていなければ「畳ませた版」が JIT / AOT で
  コンパイルできず、`assert_consistent` が落ちる。

### C1 — `const fn` の宣言と適格性検査 ✅

1. parser: 前置修飾子。**新しい予約語は増えていない** — `const` の次の
   トークンが `fn` なら修飾子、名前なら宣言。`never_allocates` とは
   どちらの順でも書ける。`Function::const_fn` (schema 18 → 19)。
2. `alloc_check.rs` を **`reachability.rs` に一般化**した。roots / sinks /
   `extern` の扱い / `str` レシーバ免除を `Policy` が持ち、
   `alloc_check.rs` と `const_fn_check.rs` はその Policy と診断だけ。
3. `const fn` の sink: heap + raw pointer builtin / allocator context /
   アロケーションカウンタ / `print` / `println`。IO・`random`・`now` は
   `extern fn` なので Opaque として自動的に落ちる。
   **`extern` に逃げ道は無い** — `never_allocates` と違い、純粋だと
   申告されてもコンパイル時に**呼ぶ手段が無い**。
   `panic` / `assert` は**許す** (強制 fold で踏んだらコンパイルエラーになる、
   それが望ましい)。
4. 診断は `never_allocates` と同じく経路を出す (`E0017`、`--explain` あり)。
5. **自由関数のみ**。

### C2 — IR の定数畳み込み ✅

`compiler_lower/src/fold.rs`。`emit()` が命令を積む直前に、両オペランドが
**同じ block で定数と分かっている**なら畳んで `Const` に差し替える。
`switch_to` で表を空にするので block ローカル。

1. **wrap するところは wrap する** (`wrapping_*`)。**trap するところは畳まない** —
   `u64` underflow / 0 除算 / 符号付き `MIN / -1` / 幅を超えるシフトは
   `None` を返し、RUNTIME-TRAP の guard がそのまま実行時に落とす。
   ここが論点 6 の修正点で、**コンパイルエラーにはしない**
   (この pass は到達可能性を知らないので `if false { 1u64 / 0u64 }` を
   誤って落としてしまう)。
2. libm 経由 (`pow` / `sqrt`) は畳まない (論点 3)。
3. **`trap_unless` の条件が `true` に畳まれたら guard ごと消える** —
   block が割れないので後続の畳み込みも続く。`false` のときは残す。
4. `drop_dead_consts` が使われない `Const` 命令を消す。liveness は
   `InstKind::for_each_operand` (compiler_ir、**catch-all 無しの網羅 match**なので
   オペランドを持つ variant を足したらコンパイルエラーになる)。
5. `consts.rs` の手書き評価器は `fold.rs` に委譲。演算子表も
   `fold::binop_for` の 1 つに統合した (expr_ops.rs の重複を削除)。
6. 受け入れ基準: 実測 3 の IR が `%v4 = const 7u64 / ret %v4` になる ✅。

**副産物: IR VM の narrow int バグを 1 つ見つけて直した** (C0 のレーンが
検出)。`eval_binop` が常に 64bit で計算して結果をそのまま置いていたので
`200u8 * 3u8` がスロットに 600 のまま残り、print も cast も masking する
ので見えず、**比較して初めて食い違った** (`200u8 * 3u8 == 88u8` が IR VM
だけ false)。narrow 型の結果を毎回正規化するようにした。

### C3 — driver 層の CTFE ✅

`interpreter/src/const_eval.rs`。型検査の直後 (エラーが 0 のときだけ) に
走り、`program.expression` を in-place で書き換える。

1. **強制位置 (forced)**: `const NAME: T = <呼び出しを含む式>`。
   値が無いと先に進めないので、失敗はすべて `E0017` のコンパイルエラー
   — trap / `panic` / `requires` 違反 / step budget / **`const fn` でない
   callee**。最後のものは実測 1 の逆側で、「interpreter だけ通る」を
   「どこでも通らない」に揃えた (エラー文言が `const fn` を指す)。
2. **任意位置 (opportunistic)**: 引数が**すべてリテラル**の `const fn` 呼び出し。
   失敗したら**畳まないだけ**で、実行時の挙動は変わらない。
   → `if false { boom(1u64) }` は合法のまま (論点 6 の要求)。
   引数を「リテラルのみ」に絞ったのは、body 中の名前が同名の const を
   隠したローカルでありうるため (誤コンパイルになる)。const 初期化子には
   ローカルが無いのでそちらは式全体を評価する。
3. 戻せるのは **scalar のみ** (`compiler_ir::Const` の形)。str / struct を
   返す `const fn` は型検査を通り実行もできるが、畳まれない。
4. step budget は 100 万ループ back-edge (`--check` の 10 万とは別。
   fold は 1 回しか走らないので緩くてよい)。
5. CTFE 中の確保は `snapshot_profile` / `restore_profile` で
   `--profile=mem` から除外 (MEMORY_PROFILING M0)。
6. 受け入れ基準: 実測 1 のプログラムが 4 実行系すべてで 42 ✅。

### C4 — DbC 接続 ✅ (item 4 は次リリース)

**この言語には warning という出力が無かった**ので、まずそれを作った。
`check_typing_diagnostics` の `Ok` が `Vec<Diagnostic>` (= warnings) を
運ぶようになり、2 つの CLI driver が表示する
(`Severity::Warning` は今回初めて実際に使われた)。

1. **述語の純粋性** (`frontend/src/type_checker/contract_purity.rs`、`E0018`、warn)。
   **設計から変えた点**: 「`const fn` しか呼べない」ではなく
   **到達可能性で純粋性そのものを検査**する。前者は stdlib に注釈を
   付けて回る作業を生むが、それは `never_allocates` / `const fn` が
   両方とも意図的に避けた道で、しかも保証は増えない。
   sink は heap alloc/free/realloc・ptr write・mem copy/move/set・
   `print` / `println` と、追えない呼び出し (`extern` / closure / `dyn`)。
   **カウンタ読みは許す** (ALLOC-CONTRACT がまさにそれ)。
   実測: `core/std/*.t` に契約は 1 つも無く、`interpreter/example/*.t`
   の契約付き 6 本すべてで警告 0 — 移行コストはこのリポジトリでは 0。
2. **定数引数の `requires` 静的検査** (`E0018`、warn)。fold が
   `ContractViolation { kind: "requires" }` を観測したら報告する。
   値キャプチャ (`with n = 3`) はそのまま流用。
   **設計から変えた点**: 型検査**エラー**ではなく警告。到達可能性を
   知らない以上 `if false { half(3u64) }` を落とせないのと、
   `INTERPRETER_CONTRACTS=off` では実際に成功するため。
   **強制位置 (`const` 初期化子) では従来どおりエラー (`E0017`)** なので、
   受け入れ基準の「型検査で落ちる」はそちらで満たしている。
3. `--api` に `const fn` / `never_allocates` を出す (どちらも出ていなかった)。
   `--explain E0018` を追加。
4. warn → error は次リリース (未実施)。

### C5 — 型の中の値

1. 配列長に `const` 識別子を許す (実測 5)。次に `const fn` 呼び出しを許す。
2. frontend に `trait ConstEvaluator` を置き、driver が実装を注入する。
   注入が無い構成 (frontend 単体テスト) では**リテラル長のみ**、
   という縮退が明示的に起きるようにする (黙って別の答えを出さない)。
3. 受け入れ基準: `val a: [i64; N]` が通り、`[i64; double(2u64)]` も通る。

### C6 — 評価器の一本化

1. `interpreter/src/ir_vm/` を `Object` / `RuntimeState` / `heap` から切り離し、
   `compiler_ir` の上のクレートとして抽出する。
2. CTFE を tree-walker から IR VM に載せ替える。
   **同じ lowering・同じ trap guard を通る**ので、C0 のレーンは構造的に通る。
3. 受け入れ基準: CTFE 経路とランタイム経路が同じコードを共有し、
   `consts.rs` の手書き評価器が消える (評価器が 3 つ → 1 つ)。

---

## 非目標

- **Zig 風の `comptime`** — 型を返す関数、型に対する計算、コンパイル時の型生成。
  この言語の generics は宣言的で、そこに手を入れる話は別の設計になる
- **コンパイル時のアロケーションと可変ヒープ** — `const fn` が `Vec` / `String` を
  返せるようにするには `Const` の表現から作り直す必要がある (論点 3)
- **str / struct / enum を返す `const fn`** — 同上。MVP は scalar のみ
- **契約の完全な静的証明** (SPARK / Dafny 的な検証) — CTFE が静的に言えるのは
  「引数が定数の呼び出しについてだけ」。それ以上は別の機構
- **cross-compilation 対応** — 今は host-only。CTFE はこの難度を上げる方向なので、
  論点 3 のとおり libm 依存を作らない選択をしておく

---

## 関連

- [`NEVER_ALLOCATES.md`](NEVER_ALLOCATES.md) — 静的検査の前例。C1 は同じパスの sink 差し替え
- [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) — CTFE の失敗を位置つきで報告するには D3 の `SiteId` が要る
- [`docs/design_by_contract.md`](../docs/design_by_contract.md) — 契約の書き方。C4 が制約を足す
- [`MEMORY_PROFILING.md`](MEMORY_PROFILING.md) — CTFE 中の確保を数えない根拠 (M0 の定義)
- [`BACKEND.md`](BACKEND.md) — 実行系の分担。CTFE を 5 つ目にしないための地図
