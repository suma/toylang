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

stdlib (`core/std/allocator.t`) に user-facing trait `Alloc` を提供:

```toylang
pub trait Alloc {
    fn alloc(&self, size: u64) -> ptr
    fn free(&self, p: ptr)
    fn realloc(&self, p: ptr, new_size: u64) -> ptr
}
```

使用形態：

1. **trait method 経由** — `arena.alloc(8u64)` (struct.alloc 直接呼び)
2. **ambient（暗黙）** — `with allocator = ... { __builtin_heap_alloc(size) }` で active stack 経由 dispatch

## IR レベルでの表現

alloc site ごとに `AllocatorBinding` を持たせる：

- `AllocatorBinding::Static(allocator_id)` — コンパイル時定数
- `AllocatorBinding::Generic(type_param)` — 型パラメータ
- `AllocatorBinding::Ambient` — 実行時スタック
- `AllocatorBinding::Local(var_id)` — ローカル変数

バックエンド（interpreter / compiler）はこの情報を見て静的／動的ディスパッチを決める。

## 現在の実装状況

### Phase 1a（完了: 2026-04-19）

**実装済み:**

- `TypeDecl::Allocator` — 不透明な allocator ハンドル型（frontend）
- `Object::Allocator(u64)` — Phase 1a は ID ベース。Phase 1b で trait オブジェクト化
- `with` キーワードと構文 `with allocator = expr { body }`
  - lexer / token / AST（`Expr::With`）/ parser / visitor / pool 全対応
- 意味解析レベルで RHS を `Allocator` 型に制約（`visit_with` が型エラーを発出）
- `__builtin_current_allocator() -> Allocator` — スタック top を返す
- `__builtin_default_allocator() -> Allocator` — グローバルハンドル（ID = 0）
- `EvaluationContext.allocator_stack: Vec<RcObject>` — push/pop セマンティクス
- `Allocator` 値の同値性比較（`==` / `!=`、順序比較は不可）
- 8 件の統合テスト（パース、スコープ、ネスト、型エラー）

**Phase 1a の制約:**

- `Allocator` 値は runtime では単なる `u64` ID。実 allocator には未接続
- `heap_alloc` / `heap_free` は依然として直接 `HeapManager` を使用
- 関数の allocator 引数機構は採用せず — caller 側 `with` で済ませる方針

### Phase 1b（完了: 2026-04-19）

**実装済み:**

- `Allocator` trait を `interpreter/src/heap.rs` に定義（`alloc` / `free` / `realloc`、`fmt::Debug` 境界）
- `GlobalAllocator` 実装（`Rc<RefCell<HeapManager>>` をラップ、`&self` メソッドで interior mutability）
- `Object::Allocator(Rc<dyn Allocator>)` に置換
  - `PartialEq` / `Hash` / `Ord::cmp` は `Rc` のポインタ identity を使用
- `__builtin_default_allocator()` が `EvaluationContext.global_allocator` の `Rc::clone` を返す
- `EvaluationContext`:
  - `heap_manager: Rc<RefCell<HeapManager>>`（共有）
  - `global_allocator: Rc<dyn Allocator>` を保持
  - `allocator_stack: Vec<Rc<dyn Allocator>>` の bottom に global allocator を常に配置
- `heap_alloc` / `heap_free` / `heap_realloc` が `allocator_stack.last()` 経由で動作
- `ptr_read` / `ptr_write` / `mem_copy` / `mem_move` / `mem_set` は `heap_manager.borrow*()` で直接アクセス（allocator 非依存のアドレスベース API のため）
- `Expr::With` は評価結果から `Rc<dyn Allocator>` を抽出して push、終了時に pop
- `Allocator` 値の等価比較は `Rc::ptr_eq` ベース（同一インスタンスなら true）
- 新規テスト: global allocator がスタック底部に常駐することを検証

**Phase 1b の未実装（Phase 1c に先送り）:**

- `ArenaAllocator`（領域単位で解放）
- `FixedBufferAllocator`（固定バッファ）
- これらを言語側から生成するビルトイン（例: `arena_allocator()`）

Phase 1b 時点では全ての allocator が `GlobalAllocator` に帰着するため、`with` の効果は「スコープ追跡のみ」観測可能。実際に別々のヒープに振り分けるのは Phase 1c 以降。

### Phase 1c: カスタム allocator 実装（完了: 2026-04-19）

**実装済み:**

- `ArenaAllocator` 実装（`heap.rs`）
  - `Rc<RefCell<HeapManager>>` を共有（別アドレス空間ではなく、同じ物理メモリを使う）
  - tracked addresses を `RefCell<Vec<usize>>` で保持
  - `free(&self, _)` は no-op（アリーナは個別解放しない）
  - `reset()` で tracked を一括解放、アリーナは再利用可能
  - `Drop` で最後の `Rc` が落ちたら tracked を一括解放
- `FixedBufferAllocator` 実装（`heap.rs`）
  - `Rc<RefCell<HeapManager>>` を共有、バイト数 quota を `capacity` で課す
  - `alloc`: `used + size > capacity` なら `0`（null）を返して失敗
  - `free`: tracked から該当エントリを削除し quota を返却
  - `realloc`: 新サイズが quota 内に収まるか事前チェック、超過なら 0
  - `Drop` で tracked を一括解放
- `__builtin_arena_allocator() -> Allocator` ビルトイン
- `__builtin_fixed_buffer_allocator(capacity: u64) -> Allocator` ビルトイン
- 新規テスト計 13 件:
  - unit (heap.rs): `GlobalAllocator` 委譲、arena free no-op + reset、arena/fixed_buffer の Drop 解放、fixed_buffer の容量内成功 / 超過失敗 / free による quota 回復
  - integration (memory_tests.rs): arena と default の非同値、`with arena` 中の `current_allocator` 一致、arena 経由の alloc→write→read、fixed_buffer の容量内成功 / 超過 null / free 後の再 alloc 成功
- 副次修正: interpreter の `evaluate_comparison_op` に `Bool == Bool` / `Bool != Bool` を追加（型チェッカーは許可していたが実行時に落ちていた）

**Phase 1c の設計メモ:**

- arena / fixed_buffer はいずれも「物理的に別領域に振り分ける」方式ではなく、同じ `HeapManager` を共有する
- これは `ptr_read` / `ptr_write` 等のアドレスベース builtin を一貫して動かすため
- arena の意義は「ライフタイムの束ね」と「個別 free の無視」
- fixed_buffer の意義は「失敗しうる allocator のセマンティクス（溢れると null）」とそれを使う側のエラーハンドリング

### Phase 2a: 関数の Allocator bound（完了: 2026-04-19）

**実装済み:**

- `Function.generic_bounds: HashMap<DefaultSymbol, TypeDecl>` を AST に追加（`generic_params` と並走）
- パーサの `parse_generic_params` を `(Vec<DefaultSymbol>, HashMap<DefaultSymbol, TypeDecl>)` に拡張
  - `<T>` / `<T, U>` はそのまま
  - `<A: Type>` で bound を受理（bound はネストジェネリックもパース可）
- `parse_type_declaration` で識別子 `Allocator` を `TypeDecl::Allocator` に contextual に解決
- struct / impl の bounds は Phase 2a では無視（2b 以降で対応）
- 型チェッカー:
  - `TypeCheckContext.current_fn_generic_bounds` を追加
  - `type_check(func)` 開始時に `func.generic_bounds` を push、終了（正常・エラー問わず）で前の状態に復元
  - `visit_with` が `TypeDecl::Generic(A)` を受理する条件を `current_fn_generic_bounds[A] == Allocator` に拡張
- 新規テスト 2 件:
  - `fn use_alloc<A: Allocator>(a: A)` の `with allocator = a` が動作
  - `fn use_alloc<A>(a: A)`（bound なし）の `with allocator = a` は型エラー

**Phase 2a のスコープ外（Phase 2b 以降）:**

- 関数呼び出し側での bound 検証（`f(non_allocator)` を拒否）— 現状 `is_equivalent` が Generic をワイルドカード扱い
- struct / impl ブロックでの bound 対応
- 複数 bound（`<A: Allocator + Clone>`）や trait 定義
- `Box<T, A>` / `List<T, A>` の導入

### Phase 2b: 呼び出し・struct・impl での bound 検証（完了: 2026-04-19）

**実装済み:**

- **関数呼び出し**: `visit_generic_call` で制約解法後に `fun.generic_bounds` を検査
  - 推論結果の型が bound と一致しない場合は "bound violation" として型エラー
  - 呼び出し側が自身の `<B: Allocator>` パラメータを渡すケースも許容（bound 同一なら連鎖 OK）
- **struct**: `Stmt::StructDecl.generic_bounds` を AST に追加し、型チェッカーが `struct_generic_bounds` を保存
  - struct literal の制約解法後に同様の bound 検査
- **impl**: `impl<A: Allocator> Container<A>` の bound を各メソッドの `MethodFunction.generic_bounds` に継承
  - `visit_impl_block_impl` でメソッド本体型チェック時に `current_fn_generic_bounds` にインストール
  - 終了時（正常・エラー問わず）に元の bounds を復元
- `AstVisitor::visit_struct_decl` のシグネチャに `generic_bounds` を追加
- テスト計 5 件追加:
  - `use_alloc(42u64)` が `<A: Allocator>` に対して型エラー
  - `fn outer<B: Allocator>(b: B) { inner(b) }` が成功（bound 連鎖）
  - `struct Holder<A: Allocator>` に Allocator 値を入れると成功
  - 同 struct に u64 を入れると bound violation
  - `impl<A: Allocator> Holder<A> { fn run(self) { with allocator = self.alloc {...} } }` がメソッド本体で bound を見えるケース

**Phase 2b の残タスク（より先の Phase に移動）:**

- 複数 bound の構文（`<A: Allocator + Clone>` 等）と trait 定義の導入 — 独立した機能で Phase 2c 以降

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

### Phase 5: 残タスク

- [ ] **scope-bound auto-cleanup (Design A)** — `with allocator = Arena::new() { ... }`
      / `with allocator = FixedBuffer::new(cap) { ... }` の temporary form を
      検出して block exit 時に自動 drop を発火させる。実装方針:
      - lower 段階で `Expr::With` の allocator_expr を peek
      - `Expr::AssociatedFunctionCall("Arena", "new", ...)` /
        `("FixedBuffer", "new", ...)` の形なら scope cleanup chain に
        `<temporary>.drop()` の synthesized 呼び出しを追加
      - `Identifier(name)` などの bind 済み形は対象外（user 管理）
      - 3 backend (interpreter / JIT silent fallback / AOT) で同形の
        cleanup wiring + 3-way `assert_consistent` test
      - Drop trait / 汎用 RAII は別 phase（必要になったら）
- [ ] AllocatorBinding::Static / Local の lower 配線 — 現状は Ambient 等価 (toy_alloc_current 経由)。compile-time 定数 allocator では libc 直接呼び出しに最適化できる余地。注: scope-bound auto-cleanup と独立、優先度低（const-prop / inlining が入るまで観察可能な perf 改善はゼロ）

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

Phase 1a では allocator 型は完全に不透明。`Allocator` 同士の `==` / `!=` のみ許可、順序比較や算術は型エラー。

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
