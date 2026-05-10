# Allocator システム 実装計画

> 実装側の設計・進捗ドキュメント。**ユーザ向けの構文・セマンティクス**は
> [`docs/language.md`](docs/language.md) の *Allocators* 章を参照してください。
> JIT 側の allocator 対応範囲は [`JIT.md`](JIT.md) の *Allocators* 節にあります。

本ドキュメントは toylang における allocator システムの設計方針と段階的な実装計画をまとめたものです。

関連する TODO 項目: `todo.md` の 121 番。

## 設計の動機

現在の `__builtin_heap_alloc` / `__builtin_heap_free` は C と同等の最下層 API で、allocator 抽象を持たない。今後：

- より高レベルなコレクション型（List、Dict 等）を実装する
- 将来的にネイティブコード生成（静的バイナリ）を行う
- 領域ごとに allocator 戦略を切り替えたい（arena、pool、tracking 等）

これらを支えるため、allocator を言語・ランタイム・コード生成の各層で一貫して扱える仕組みを導入する。

## 設計方針

**ambient + lexical scope** 方式: Odin/Jai の暗黙コンテキストに近い。

- **デフォルト**: ambient（暗黙）なグローバル allocator
- **スコープ上書き**: `with allocator = expr { ... }` による lexical scope
- 関数引数として allocator を取る形 (`fn f(a: Allocator)` や generic bound `<A: Allocator>`) は採用しない — caller が `with allocator = ...` で囲むことを期待

### 既存言語との比較

| 言語 | 方式 | 本プロジェクト |
|---|---|---|
| **Zig** | 明示的 allocator を全関数に渡す | 採用しない (active stack で代替) |
| **Odin / Jai** | `context.allocator` の暗黙スタック | 同じ lexical スタック方式 |
| **C++ std::pmr** | vtable ベースの実行時 allocator | デフォルトのランタイム形態として使用 |

## 言語表層

```rust
# ambient allocator（デフォルト）
val x = some_alloc_function()

# スコープ内で allocator を差し替え
with allocator = arena {
    val y = some_alloc_function()  # arena から確保
}

# stdlib wrapper: trait Alloc + Global / Arena / FixedBuffer
# パターン 1 — temporary form（推奨、auto-cleanup）
with allocator = Arena::new() {
    val p = __builtin_heap_alloc(64u64)
    # block exit 時に runtime が arena handle を自動 release
}

# パターン 2 — named binding form（with をまたぐ allocator）
val arena = Arena::new()       # named binding は user 管理
with allocator = arena {
    val p = __builtin_heap_alloc(64u64)
}
with allocator = arena {       # 同じ arena を別の with で再利用
    val q = __builtin_heap_alloc(32u64)
}
arena.drop()                   # 明示 drop が必要
```

詳細とサンプルコードは下の **Allocator 寿命管理ポリシー** を参照。

### `with` のセマンティクス

`with allocator = expr { body }` は **lexically scoped な push/pop**。

- `expr` が compile-time 定数 → コンパイラは body 内の ambient 参照を `expr` で定数伝搬
- それ以外 → 動的スタックに push、ブロック終端で pop（例外・return 等も含め必ず pop）
- 型検査器は `expr` の「静的決定性」を属性として IR に残す

### Allocator 寿命管理ポリシー（採用: Design A — scope-bound）

複数の設計案 (Drop trait / `defer` / reset / closure / linear / 階層 arena など)
を検討した上で、**`with` の lexical scope = allocator の lifetime** とする
**Design A (scope-bound)** を採用する。`with allocator = ... { body }` の
`...` 部分が **temporary expression**（名前 binding 無し）の場合のみ
runtime / IR 層で自動 cleanup を発火させる。

#### パターン 1 — temporary form（推奨、auto-cleanup）

block 内に閉じる短命な arena は temporary として書く。`with` の exit 時
（return / break / panic / 通常 exit のいずれでも）に runtime が
`__builtin_arena_drop` を自動呼び出しする。user は drop を意識しない。

```toylang
fn process_chunk(input: u64) -> u64 {
    var sum: u64 = 0u64
    with allocator = Arena::new() {
        # この block 内の heap_alloc / heap_realloc は新 arena から
        val buf: ptr = __builtin_heap_alloc(input * 8u64)
        # ...
        sum = compute(buf, input)
    }
    # block を抜けた瞬間に arena slot は release される。
    # 関数を抜ける時にリークなし、明示 drop コール無し。
    sum
}
```

- 同じ pattern は `FixedBuffer::new(capacity)` にも適用される（`Arena` と
  対称、scope 終了時に handle を release）。
- `Global::new()` は default allocator の wrapper で実体は process 全体
  共有なので auto-cleanup の対象外（drop は no-op）。

#### パターン 2 — named binding form（with をまたぐ allocator）

arena を 1 つの `with` block より長く生かしたいケース、例えば

- 複数の `with allocator = a { ... }` ブロックで同じ arena から確保
  したい（共通 arena を使いまわす）
- 複数の return path / 別関数まで持ち回したい
- arena の drop タイミングを user が決めたい（中盤で reset したい等）

このときは **named binding** で `val a = Arena::new()` し、user 自身が
`a.drop()` を呼ぶ責任を持つ（auto-cleanup は発火しない）。

```toylang
fn build_two_views(n: u64) -> u64 {
    # 同じ arena を 2 つの with ブロックで共有したい
    val a: Arena = Arena::new()

    var first: u64 = 0u64
    with allocator = a {
        val buf1: ptr = __builtin_heap_alloc(n * 8u64)
        first = consume(buf1, n)
    }

    var second: u64 = 0u64
    with allocator = a {
        val buf2: ptr = __builtin_heap_alloc(n * 8u64)
        # buf1 と buf2 は同じ arena slot を共有 — 個別 free は no-op、
        # まとめて a.drop() で解放される
        second = consume(buf2, n)
    }

    val result: u64 = first + second
    a.drop()   # named binding は user 管理 — 忘れると process exit まで生きる
    result
}
```

- named binding を `with` に渡しても auto-cleanup は走らない。lexical
  scope が allocator の lifetime ではなくなるので、user が責任を取る。
- `Arena::drop` は idempotent — 二度呼んでも second call は no-op
  （registry slot は handle index ごとに 1 回だけ実 free を行う）。
  忘却 footgun は process 全体で見れば「arena slot 1 個分の常住」だけ。

#### パターン 3 — 関数引数として allocator を持ち回す（推奨しない）

`fn f(a: Allocator)` のように allocator を引数で渡す形は **避ける**。
代わりに caller 側で `with allocator = ... { f() }` で囲み、callee は
ambient 経由で受け取る。

```toylang
# 非推奨
fn fill_old(a: Allocator, n: u64) -> ptr {
    with allocator = a {
        __builtin_heap_alloc(n * 8u64)
    }
}

# 推奨
fn fill(n: u64) -> ptr {
    __builtin_heap_alloc(n * 8u64)   # ambient allocator が active
}
fn caller(n: u64) -> ptr {
    with allocator = Arena::new() {
        fill(n)
    }
}
```

理由: ambient + `with` で渡す方が caller 側で allocator を一箇所に集約
でき、callee の signature を汚さずに済む（Odin / Jai の context system と
同じ思想）。

#### auto-cleanup の判定条件

runtime / IR 層で auto-cleanup を発火させる判定:

1. `with allocator = <expr> { ... }` の `<expr>` が **構文的に**
   stdlib wrapper struct のコンストラクタ呼び出し（現在は
   `Arena::new()` / `FixedBuffer::new(...)`）であること。
2. その struct が `drop(&mut self)` メソッドを持つこと。
3. block exit 時（通常 / `return` / `break` / `continue` / panic
   いずれも）に synthesized で `<temporary>.drop()` を呼ぶ。

`val a = Arena::new()` のように **bind した値**を `with` に渡した場合
（`<expr>` が `Identifier(a)`）、auto-cleanup は **発火しない**。lexical
sniff だけで判定するので RAII / Drop trait / lifetime inference は不要。

汎用 RAII（任意の struct で `Drop` trait を impl して自動呼び出し）は
別 phase（必要になったら）。この sniff だけで allocator の典型ユース
ケースの 9 割をカバーできる。

## Allocator trait

stdlib (`core/std/allocator.t`) に user-facing trait `Alloc` と
3 つの wrapper struct を提供する。`Allocator` (primitive runtime
ハンドル、`__builtin_*_allocator()` の戻り型) と `Alloc` (この
trait) は名前空間が分かれているので衝突しない。

### `trait Alloc`

```toylang
pub trait Alloc {
    fn alloc(&self, size: u64) -> ptr
    fn free(&self, p: ptr)
    fn realloc(&self, p: ptr, new_size: u64) -> ptr
}
```

各メソッドは `&self` 受信。実装は内部の `Allocator` ハンドルを
`with allocator = self.h { __builtin_heap_* }` で active stack に
push して dispatch する — これにより struct.method 経由の呼び出しと
ambient 経由の呼び出しが完全に同じ runtime path を通る。

### Wrapper structs

| Struct | Field | Constructor | Drop |
|---|---|---|---|
| `Global` | `h: Allocator` | `Global::new()` → `__builtin_default_allocator()` | (no-op、process 全体共有) |
| `Arena` | `h: Allocator` | `Arena::new()` → `__builtin_arena_allocator()` | `impl Drop` で `__builtin_arena_drop(self.h)` |
| `FixedBuffer` | `h: Allocator`, `cap: u64` | `FixedBuffer::new(capacity)` → `__builtin_fixed_buffer_allocator(capacity)` | `impl Drop` で `__builtin_fixed_buffer_drop(self.h)` |

`FixedBuffer` だけ `capacity(&self) -> u64` の inherent method
(quota の問い合わせ用) を追加で持つ。

`Global::new()` は default allocator の wrapper で、実体は process
全体共有なので `Drop` は提供されない (`impl Drop` 自体が定義され
ていない)。`Arena` / `FixedBuffer` は `with allocator = X::new() { ... }`
の temporary form で auto-cleanup の対象になる (Phase 5 — 下記参照)。

```toylang
pub struct Arena { h: Allocator }

impl Arena {
    fn new() -> Self { Arena { h: __builtin_arena_allocator() } }
}

impl Drop for Arena {
    fn drop(&mut self) { __builtin_arena_drop(self.h) }
}

impl Alloc for Arena {
    fn alloc(&self, size: u64) -> ptr {
        with allocator = self.h { __builtin_heap_alloc(size) }
    }
    fn free(&self, p: ptr) {
        with allocator = self.h { __builtin_heap_free(p) }
    }
    fn realloc(&self, p: ptr, new_size: u64) -> ptr {
        with allocator = self.h { __builtin_heap_realloc(p, new_size) }
    }
}
```

### 使用形態

1. **trait method 経由** — `arena.alloc(8u64)` (struct.alloc 直接呼び)。
   内部で `with allocator = self.h { ... }` を踏むので active stack
   経由と同じ dispatch path に乗る。
2. **ambient（暗黙）** — `with allocator = arena { __builtin_heap_alloc(size) }`
   で active stack 経由 dispatch。`with` に渡せるのは
   `Allocator` 値、または `Alloc` impl を持つ struct (単一の
   `Allocator`-typed field を auto-extract)。

## IR レベルでの表現

alloc site ごとに `AllocatorBinding` を持たせる：

- `AllocatorBinding::Static(allocator_id)` — コンパイル時定数
- `AllocatorBinding::Generic(type_param)` — 型パラメータ
- `AllocatorBinding::Ambient` — 実行時スタック
- `AllocatorBinding::Local(var_id)` — ローカル変数

バックエンド（interpreter / compiler）はこの情報を見て静的／動的ディスパッチを決める。

## 現在の実装状況

> **進捗サマリ**:
> Phase 1〜5 の主要マイルストーンは全て landing 済み。残タスクは
> Phase 3 の AST→IR lowering / `Static` 化パスと、Phase 5 の
> `AllocatorBinding` refinement (Static/Local 化) + devirt pass。
> 履歴詳細は [変更履歴](#変更履歴) 表を参照。

### Phase 1: Allocator handle + with 構文 + custom allocator (完了: 2026-04-19)

**1a — handle 型と `with` 構文 (lexical scope のみ):**

- `TypeDecl::Allocator` (frontend) と `Object::Allocator` (runtime) の追加
- `with allocator = expr { body }` 構文 (lexer / token / AST `Expr::With` /
  parser / visitor / pool の全層対応)、RHS は `Allocator` 型に制約
- `__builtin_current_allocator()` / `__builtin_default_allocator()` builtin
- `EvaluationContext.allocator_stack` (push/pop)、global allocator が常に
  bottom に常駐
- `Allocator` 同士の `==` / `!=` のみ許可 (順序比較は型エラー)

**1b — `Allocator` trait と Rc 化:**

- `Allocator` trait を `interpreter/src/heap.rs` に定義
  (`alloc` / `free` / `realloc`、`&self` で interior mutability)
- `GlobalAllocator` 実装、`Object::Allocator(Rc<dyn Allocator>)` に変更
- `heap_alloc` / `heap_free` / `heap_realloc` が `allocator_stack.last()`
  経由で dispatch、`ptr_read/write` / `mem_copy/move/set` は
  `heap_manager` 直接 (allocator 非依存のアドレス API)

**1c — custom allocator 実装:**

- `ArenaAllocator` — `HeapManager` 共有 + tracked addrs (`RefCell<Vec<usize>>`)、
  `free` は no-op、`reset()` / `Drop` で一括解放
- `FixedBufferAllocator` — quota `capacity` 課金、超過で `alloc` が `0`
  (null) を返す、`free` で quota 復帰、`Drop` で一括解放
- `__builtin_arena_allocator()` / `__builtin_fixed_buffer_allocator(cap)` builtin

**設計メモ**: arena / fixed_buffer はいずれも物理的に別領域ではなく、同じ
`HeapManager` を共有する (アドレスベース builtin を一貫して動かすため)。
arena の意義は「ライフタイムの束ね + 個別 free の無視」、fixed_buffer の
意義は「失敗しうる allocator のセマンティクス + caller 側エラーハンドリング」。

### Phase 2: 関数・struct・impl での Allocator bound (完了: 2026-04-19)

**2a — 関数の bound 構文:**

- `Function.generic_bounds: HashMap<DefaultSymbol, TypeDecl>` を AST に追加
- パーサが `<A: Type>` を受理 (bound はネストジェネリック可)
- `Allocator` 識別子を `TypeDecl::Allocator` に contextual 解決
- 型チェッカー `TypeCheckContext.current_fn_generic_bounds` を関数 entry 時 push、
  exit 時に復元。`visit_with` が `Generic(A)` を bound 経由で受理

**2b — 呼び出し・struct・impl への拡張:**

- **関数呼び出し**: `visit_generic_call` が制約解法後に bound 検査、不一致は
  "bound violation"。caller 自身の `<B: Allocator>` を渡す bound 連鎖は許容
- **struct**: `StructDecl.generic_bounds` を AST + context に追加、struct literal
  でも bound 検査
- **impl**: `impl<A: Allocator> Container<A>` の bound を `MethodFunction.generic_bounds`
  に継承、メソッド本体型チェック時に `current_fn_generic_bounds` にインストール

**残タスク** (より先の Phase に移動): 複数 bound (`<A: Allocator + Clone>`)、
独立 trait 定義機構 — Phase 2c 以降扱い。

### Phase 3: コレクション型 + IR 整備

- [x] **ユーザ空間の List<u64> が書ける** — 組み込み List ではなく、struct+impl+heap builtin で `with allocator` を透過的に使えるところまで（2026-04-19）
  - ptr/Allocator を struct フィールドに使えるよう struct 型検査を拡張
  - 非ジェネリック struct の Self/Identifier return type を Struct に正規化（`make_list() -> List` と `.push() -> Self` の連鎖が通る）
  - 関数 return type の比較を非ジェネリック struct の `Identifier == Struct(name, [])` に限定して緩める（ジェネリック struct の型引数省略エラーは維持）
- [x] ジェネリック `Vec<T>` (= 旧 List<T> の後継) — `core/std/collections/vec.t` に landing
- [x] `ambient` キーワード（式）で `__builtin_current_allocator()` への糖衣を提供（2026-04-19）
- [x] 下位 IR の設計：`compiler/src/ir.rs` に `AllocatorBinding` enum を導入（2026-05-02）
  - `AllocatorBinding::Static(u32)` — コンパイル時定数（allocator id）
  - `AllocatorBinding::Generic(DefaultSymbol)` — 型パラメータ（パラメータ名で識別）
  - `AllocatorBinding::Ambient` — 実行時スタック
  - `AllocatorBinding::Local(u32)` — ローカル変数（local id）
  - `Display` 実装と 5 件の unit test (`compiler::ir::allocator_binding_tests`)
  - 現状は型定義のみ。compiler が `__builtin_heap_alloc` 系をまだ lowering しないため、実際の Instruction には付与されていない。次フェーズ（Phase 4 native codegen）で alloc site の lowering と同時に classify ロジックを wire する
- [ ] 型チェック後に AST → IR への lowering パスを追加
- [ ] `with` ブロックの allocator 式が compile-time 定数かを判定し、内部の `Ambient` を `Static` に置換するパス

### Phase 4: Native codegen (完了: 2026-05-04)

- [x] Cranelift 採用 (compiler crate)
- [x] 呼び出し規約: 関数引数に allocator を載せず、active stack 経由で dispatch
  - `__builtin_heap_alloc / _realloc / _free` は `toy_alloc_current()` を読んで `toy_dispatched_*` に委譲
  - `with allocator = expr { body }` は `AllocPush` / `AllocPop` で stack を操作
- [x] 静的バイナリ生成 (`cargo run -p compiler -- --emit=executable`)
- [x] arena / fixed_buffer の native runtime (toylang_rt.c)

### Phase 5: Allocator lifecycle 管理 (auto-cleanup + Drop trait + 汎用 RAII) (完了: 2026-05-04)

Allocator の寿命管理ポリシー (Design A — scope-bound) を 3 backend で
wire up し、stdlib `Drop` trait と汎用 RAII を整備。詳細な実装手順や
ファイル単位の変更は git log / 変更履歴表を参照、ここでは設計 outcome のみ。

**5.1 — Arena / FixedBuffer auto-cleanup (temporary form):**

`with allocator = Arena::new() { ... }` および
`with allocator = FixedBuffer::new(cap) { ... }` の temporary form を、
3 backend (interpreter / cranelift JIT / AOT) すべてで scope exit 時に
auto-cleanup する。

- 判定方式は **syntactic sniff** — allocator_expr が
  `Expr::AssociatedFunctionCall("Arena"|"FixedBuffer", "new", ...)` の形か
  どうかをパーサ出力レベルで peek する。`val a = Arena::new()` 経由の
  named binding は `<expr>` が `Identifier(a)` になるので auto-cleanup は
  発火せず、user が `a.drop()` を明示的に呼ぶ。
- **AOT**: `FunctionLower.with_scope_arena_drops: Vec<WithScopeCleanup>`
  (3 variant `None / ArenaDrop(h) / FixedBufferDrop(h)`) で per-scope
  cleanup record を保持。linear exit / 早期 exit (`return` / `break` /
  `continue`) のいずれでも `AllocPop` の直後に `AllocArenaDrop` または
  `AllocFixedBufferDrop` を emit。runtime helper `toy_arena_drop` /
  `toy_fixed_buffer_drop` (`compiler/runtime/toylang_rt.c`) は handle 種別を
  defensive にチェックし、想定外の slot kind では no-op。
- **interpreter**: `Allocator` trait の `reset()` メソッドを
  `ArenaAllocator` / `FixedBufferAllocator` で override (tracked addrs を
  全 free)。`Expr::With` の lexical sniff で temporary 判定後、scope exit
  時 (panic / 早期 return / 通常 exit の全 path) に `allocator_rc.reset()`
  を呼ぶ。`GlobalAllocator` の default 実装は no-op。
- **JIT**: silent fallback で interpreter 経路に流れて自動対応。

**5.2 — stdlib `Drop` trait scaffolding:**

`core/std/drop.t` に `pub trait Drop { fn drop(&mut self) }` を新設し、
`core/std/allocator.t::impl Drop for Arena` / `impl Drop for FixedBuffer`
を実装。`__builtin_arena_drop(handle)` / `__builtin_fixed_buffer_drop(handle)`
builtin を frontend / interpreter / JIT eligibility / AOT lower の 4 layer に
登録。`arena.drop()` / `fb.drop()` の named-binding 呼び出しは trait 経由で
dispatch されるが builtin 直呼び出しと semantics 同一。

**5.3 — 汎用 RAII (interpreter + AOT):**

任意 user struct で `impl Drop for X { fn drop(&mut self) }` を書けば、
binding が scope exit する時に LIFO 順で auto-call される。

- **interpreter**: `EvaluationContext.drop_trait_structs: HashSet<DefaultSymbol>`
  (Drop impl のある struct 集合) と `drop_scopes: Vec<Vec<DropEntry>>`
  (per-scope の record list) を追加。`evaluate_block` で push/pop、val/var
  宣言時に `register_drop_if_needed(name, value)`、scope exit (linear /
  Return / Break / Continue) で逆順に invoke。`collect_drop_trait_structs`
  は program AST から `ImplBlock { trait_name: Some("Drop"), .. }` を walk
  して集合を作る。
- **panic 安全性**: `panic("...")` は `Err(_)` 経路なので drop は skip
  (Rust の unwind=drop なしモード相当 — process exit 中の double-fault
  リスクを避ける)。
- **AOT**: `Module.drop_trait_structs` + `FunctionLower.drop_scopes` +
  `register_drop_for_struct_binding` + `emit_drop_scopes_to_depth` を追加。
  `Expr::Block` / `terminate_return` / `Stmt::Break` / `Stmt::Continue` で
  wire し、`emit_drop_call` は `CallWithSelfWriteback` 経由で `Drop::drop`
  メソッドを invoke。stdlib の `Arena` / `FixedBuffer` は
  `drop_trait_structs` から除外 (5.1 syntactic-sniff path との二重 drop
  回避)。
- **JIT**: user-defined `impl Drop for X` を含むプログラムは silent
  fallback で interpreter 経路へ。

**5.4 — `AllocatorBinding` wiring (informational tag):**

`InstKind::HeapAlloc` / `HeapRealloc` / `HeapFree` に
`binding: AllocatorBinding` フィールドを追加。
`FunctionLower::classify_active_allocator_binding()` が lower 時に現在の
`with` scope 構成から binding を返す (現状は常に `Ambient` を返す保守的
実装)。codegen は `binding: _` で無視し、active-stack dispatch
(`toy_alloc_current` + `toy_dispatched_*`) を継続 — tag は IR dump
(`; alloc=ambient` postfix) で確認できる informational マーカー。
Static/Local 化や devirt pass の hook ポイント。

### Phase 5 残タスク

- [ ] `AllocatorBinding::Static(id)` / `Local(local)` / `Generic(sym)` への
      refinement — `with` scope の expr が `__builtin_default_allocator()`
      や named local binding の場合に classifier がそれを認識する。
- [ ] devirt pass — refined binding を使って alloc site を libc malloc
      直接呼び出しに fold。const-prop / inlining が入る前は perf 改善は
      限定的。

## 設計上の注意点

### alloc / free の allocator 不一致

alloc 時と free 時で異なる allocator が使われるとメモリ破壊を招く。対策：

1. ポインタヘッダに allocator ID を埋め込み、free 時に検証
2. または arena 系のみサポートして個別 `free` を型エラーにする
3. コンパイラ側では逃げ出し解析で検出

### クロージャのキャプチャ

クロージャ生成時点の ambient か、呼び出し時の ambient か。

**採用：呼び出し時の ambient**（Odin / Jai と同じ）。キャプチャ時固定が必要なら `with` で明示する。

### interpreter / compiler の挙動差

**観測可能な動作は同一**とするのが契約。allocator の副作用（alloc 回数、順序等）が見える場合も両者で同じ順序で呼ぶ。

### 関数境界での ambient 漏れ

`with` のスコープは lexical のみ。呼ばれた先に自動伝搬し、戻る時点で元に戻る（call stack unwind と同じ）。

### 型システムの制約

`Allocator` 型は完全に不透明。`Allocator` 同士の `==` / `!=` のみ許可、
順序比較や算術は型エラー。同一インスタンスなら `==` が true (異なるハンドル
は別物として扱われる)。

## バックエンド別の実装戦略

### Interpreter

```
EvaluationContext {
    ...
    allocator_stack: Vec<Rc<dyn Allocator>>,  // Phase 1b
}
```

- `with` → push、ブロック終端 → pop
- `heap_alloc(size)` → `allocator_stack.last().alloc(size, align)`
- ジェネリック関数 `fn f<A>(...)` は型引数を runtime `Rc<dyn Allocator>` として受け渡し（特殊化しない）

実装コスト: 小（Phase 1b で完結）。

### Compiler（将来）

**推奨戦略（案A + 案C のハイブリッド）:**

- **案A（隠し引数）**: デフォルトは全関数に `&dyn Allocator` を暗黙追加。`with` は呼び出し時に引数を差し替える。定数伝搬で vtable が消えればインライン化される
- **案C（型パラメータ単相化）**: `#[specialize_allocator]` 属性または compile-time 定数 allocator が使われている関数は allocator 型ごとに複製

「通常は動的ディスパッチ（コードサイズ優先）、hot path は単相化（速度優先）」が両立する。

## 参考

- **Zig**: [Allocators Guide](https://zig.guide/standard-library/allocators/) — 明示的 allocator、comptime で単相化可能
- **Odin**: [Implicit context system](https://odin-lang.org/docs/overview/#implicit-context-system) — `context.allocator` による ambient
- **Jai**: `push_context` / `context.allocator`
- **Rust**: `Box<T, A>` (Nightly `Allocator` API) による型パラメータ単相化 (本プロジェクトは未採用)
- **C++**: `std::pmr` は vtable ベースの実行時 allocator

## 変更履歴

| 日付 | Phase | 内容 |
|---|---|---|
| 2026-05-10 | runtime arena/fixed_buffer 撤去 | toylang stdlib `Arena` / `FixedBuffer` が tracking + bulk-free + quota check をすべて実装するようになったので、runtime 側の専用 infrastructure を撤去。削除: `__builtin_arena_allocator()` / `__builtin_fixed_buffer_allocator(cap)` / `__builtin_arena_drop` / `__builtin_fixed_buffer_drop` builtins、interpreter `heap.rs::ArenaAllocator` / `FixedBufferAllocator`、JIT registry/helpers、AOT IR variants `AllocArena` / `AllocFixedBuffer` / `AllocArenaDrop` / `AllocFixedBufferDrop`、AOT `WithScopeCleanup::ArenaDrop` / `FixedBufferDrop`、`compiler/runtime/toylang_rt.c::toy_arena_*` / `toy_fixed_buffer_*` / `toy_alloc_registry` / `toy_alloc_slot_*`、`compiler/src/jit.rs::JitAllocSlot` / `JitAllocKind` / `JIT_ALLOC_REGISTRY`。stdlib `_h: Allocator` を `__builtin_default_allocator()` に切替、`reset()` は toylang 側追跡表を walk して each `__builtin_heap_free`。inline-temporary `with allocator = Arena::new() { ... }` は AOT 側で synthetic struct construction + drop_scopes 経由の user-Drop dispatch、interpreter 側で Expr::With 後に struct.drop() invoke。Arena/FixedBuffer は `drop_trait_structs` から exclude しないよう変更 (toylang `drop()` が idempotent)。`toy_dispatched_alloc/free/realloc` は handle 引数を ignore して libc passthrough に簡略化。`raw_builtin_arena_auto_cleanup_round_trip` / `aot_arena_drop_releases_and_reuses` / `aot_arena_and_fixed_buffer_allocators_round_trip` 等 raw builtin を直接使う test を撤去 (新 stdlib API tests `aot_arena_bytes_used_and_reset` / `aot_fixed_buffer_introspection` でカバー) |
| 2026-05-10 | stdlib introspection 拡充 (Odin/Zig 風) | `core/std/allocator.t` の `Arena` / `FixedBuffer` を toylang で再実装。`(addr, size)` の parallel array (`addrs` / `sizes` + `count` / `cap_slots`) を struct に持たせて `arena.bytes_used()` / `fb.used()` / `fb.remaining()` / `fb.is_empty()` / `arena.reset()` / `fb.reset()` を露出。runtime arena/fixed_buffer は引き続き `_h` field 経由で backing として利用 (`with allocator = arena_struct { __builtin_heap_alloc(...) }` の既存 user code が無修正で動作するため)。null pointer の local 変数 bind を避けるため alloc/realloc は size==0 / quota 超過を early-return で扱う。`trait Alloc` を `&self` から `&mut self` に変更 (AOT は `&self` 経由の field write-back を持たないため)。新 builtin: `__builtin_ptr_eq(a: ptr, b: ptr) -> bool` (parallel array の `_find` 用) と `__builtin_null_ptr() -> ptr` (libc 非依存の null pointer; `__builtin_heap_alloc(0u64)` は AOT で libc malloc(0) に委譲するため非 null を返しうる)。新例 `interpreter/example/allocator_reuse.t`、新 consistency test `aot_arena_bytes_used_and_reset` / `aot_fixed_buffer_introspection` |
| 2026-05-04 | 汎用 RAII の AOT 補完 | `Module.drop_trait_structs` + `FunctionLower.drop_scopes` + `register_drop_for_struct_binding` + `emit_drop_scopes_to_depth` を追加、`Expr::Block` / `terminate_return` / `Stmt::Break` / `Stmt::Continue` で wire、`emit_drop_call` は CallWithSelfWriteback 経由で `Drop::drop` を invoke。stdlib Arena/FixedBuffer は drop_trait_structs から除外 (syntactic-sniff path との二重 drop 回避)。JIT は user-defined Drop 検出時 silent fallback。3-way assert_consistent で全 backend pass。1171 → 1173 tests pass |
| 2026-05-04 | 汎用 RAII (interpreter) | 任意 user struct の `impl Drop for X` が scope exit 時に LIFO 順で auto-call。`EvaluationContext.drop_trait_structs` + `drop_scopes`、`evaluate_block` で push/pop、val/var 宣言で `register_drop_if_needed`。panic / Err は drop skip。AOT は別 phase。1169 → 1171 tests pass |
| 2026-05-04 | Phase 5 完了 (Drop trait + AllocatorBinding wiring) | 残 2 項目を最小 viable で landing。stdlib `core/std/drop.t::Drop` trait 新設、Arena/FixedBuffer を `impl Drop` に移行、`__builtin_fixed_buffer_drop` を 4 layer に登録。`InstKind::Heap*` に `binding: AllocatorBinding` field 追加 + lower 時 classify (現状 Ambient 一択)、codegen は active-stack dispatch 継続 (tag は informational、devirt pass の hook)。汎用 RAII / Static/Local binding refinement は別 phase。1168 → 1169 tests pass |
| 2026-05-04 | Phase 5 部分完了 (FixedBuffer auto-cleanup) | `with allocator = FixedBuffer::new(cap) { body }` の temporary form を 3 backend 全部で auto-cleanup 化。Arena と完全対称な構造 — 新 runtime helper `toy_fixed_buffer_drop`、新 IR `AllocFixedBufferDrop`、`with_scope_arena_drops` を `WithScopeCleanup` enum に refactor (None / ArenaDrop / FixedBufferDrop)、interpreter `FixedBufferAllocator::reset` impl。`InlineAlloc` enum で AOT lower の判定統一。1166 → 1168 tests pass |
| 2026-05-04 | Phase 5 部分完了 (Arena auto-cleanup) | `with allocator = Arena::new() { body }` の temporary form を 3 backend 全部で auto-cleanup 化。lexical sniff で `Expr::AssociatedFunctionCall("Arena", "new", [])` を判定、AOT は新フィールド `with_scope_arena_drops` で各 with scope の handle を保持し linear exit と早期 exit (`return` / `break` / `continue`) で `AllocPop` + `AllocArenaDrop` を emit。interpreter は body 後に `allocator_rc.reset()`。`val a = Arena::new()` の named binding は引き続き user 管理 (`a.drop()` 明示)。FixedBuffer は drop builtin 未実装のため後続 phase。1164 → 1166 tests pass |
| 2026-05-04 | Phase 5 設計確定 | Allocator 寿命管理ポリシーを **Design A (scope-bound)** に確定。`with allocator = Arena::new() { ... }` / `FixedBuffer::new(cap) { ... }` の temporary form は block exit 時 auto-cleanup、`val a = Arena::new()` 経由の named binding は user 管理 (明示 `a.drop()` が必要)。with をまたぐ allocator 利用パターンとサンプルコードを「Allocator 寿命管理ポリシー」節に追加。Drop trait / `defer` / closure / linear / 階層 arena など他案の比較検討あり、scope-bound + lexical sniff が最小コスト。実装は Phase 5 残タスクとして wire up 待ち |
| 2026-04-22 | Phase 3 部分（sizeof の型拡張） | `__builtin_sizeof` が struct（フィールド合計）/ enum（1-byte タグ + payload 合計、variant 依存）/ tuple / array にも対応。`List<Option<i64>>` のような合成型で stride 計算が可能に |
| 2026-04-22 | Phase 3 部分（任意型 T 対応 ptr I/O） | HeapManager に typed-slot map を追加、`__builtin_ptr_write(p, off, value)` は任意型を受理、`__builtin_ptr_read(p, off)` は型ヒントに合わせた値を返す。`List<i64>` / `List<bool>` / `List<T>` がそのまま動作 |
| 2026-04-22 | Phase 3 部分（allocator 型パラメータ） | `struct List<T, A: Allocator>` 形式をサポート。フィールドに現れない型パラメータを val 注釈 / メソッド return type からヒント推論、struct-level bound を impl 内部へマージ、block の numeric hint が外側 hint を上書きしないよう修正 |
| 2026-04-22 | Phase 3 前提（sizeof builtin） | `__builtin_sizeof(value)` を追加、value の型（primitive のみ）を u64 のバイトサイズに評価。generic `T` の実体サイズ取得が可能になり、将来のジェネリック List<T> 実装の土台が整った |
| 2026-04-19 | Phase 3 部分（ユーザ List<u64>） | struct フィールドに `ptr`/`Allocator` を許可、非ジェネリック struct の Self/Identifier 正規化、struct+impl で書いた List が `with allocator = arena` 内で動作 |
| 2026-04-19 | Phase 3 部分（自動 ambient 挿入） | `visit_call` で末尾 Allocator 引数省略時に合成 `BuiltinCall(CurrentAllocator)` を AST に挿入。Allocator vs `Generic(A: Allocator)` の比較も許可 |
| 2026-04-19 | Phase 3 部分（ambient sugar） | `ambient` キーワード式（`__builtin_current_allocator()` の糖衣）。lexer/parser で対応、テスト 3 件 |
| 2026-04-19 | Phase 2b 完了 | impl ブロックの bound をメソッドに継承、`MethodFunction.generic_bounds`、`visit_impl_block_impl` で body 型チェック中に bounds をインストール |
| 2026-04-19 | Phase 2b 部分完了 | struct bound 対応（`Stmt::StructDecl.generic_bounds`、`struct_generic_bounds` context、struct literal での bound 検査） |
| 2026-04-19 | Phase 2b 部分完了 | `visit_generic_call` で bound 違反を検出、bound 連鎖のテスト |
| 2026-04-19 | Phase 2a 完了 | `fn f<A: Allocator>` bound 構文のパース、`Function.generic_bounds`、`TypeCheckContext.current_fn_generic_bounds`、`visit_with` で bound 付き generic を受理、`Allocator` を contextual type として解決 |
| 2026-04-19 | Phase 1c 完了 | `FixedBufferAllocator`、`__builtin_fixed_buffer_allocator(capacity)`、Bool 同値比較の実行時サポート、quota 越えで null を返す動作のテスト |
| 2026-04-19 | Phase 1c 部分完了 | `ArenaAllocator`、`__builtin_arena_allocator()`、arena 統合テスト・ユニットテスト |
| 2026-04-19 | Phase 1b 完了 | `Allocator` trait、`GlobalAllocator`、`Object::Allocator(Rc<dyn Allocator>)`、`heap_alloc` 等のスタック経由ルーティング |
| 2026-04-19 | Phase 1a 完了 | `with` 構文、`TypeDecl::Allocator`、`Object::Allocator`、`current_allocator` / `default_allocator` ビルトイン |
| 2026-04-19 | 計画策定 | ハイブリッド設計の採用、Phase 1〜5 ロードマップ確定 |
