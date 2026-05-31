# Backend アーキテクチャ技術詳細

本ドキュメントは toylang の **実行バックエンド** の技術詳細をまとめる。
言語仕様そのものは [`docs/language.md`](../docs/language.md)、運用ガイダンスは
[`CLAUDE.md`](../CLAUDE.md)、タスク進捗は [`design-docs/todo.md`](todo.md) を参照。

## 全体像

toylang は 1 つのフロントエンド（lexer / parser / type checker）の出力を、
**4 つのバックエンド**が解釈する構成になっている。

| バックエンド | 実体 | 方式 | カバレッジ |
|---|---|---|---|
| **Tree-walker** | `interpreter/src/evaluation/` | AST を再帰 walk、`Rc<RefCell<Object>>` 値 | 言語全機能（リファレンス実装） |
| **AOT compiler** | `compiler/src/codegen/` | IR → cranelift → object file → リンク | compiler MVP の範囲 |
| **Cranelift JIT** | `interpreter/src/jit/` | AST → cranelift を直結（IR を経由しない） | scalar/数値中心、未対応は silent fallback |
| **IR VM** | `interpreter/src/ir_vm/` | 共有 IR を直接 interpret（flat slot） | lower 可能なものほぼ全部（opt-in） |

- **Tree-walker** が意味論の正本（reference oracle）。新機能はまずここで動く。
- **AOT / IR VM** は共有 IR (`compiler_ir`) を入力にするため、`compiler_lower` の
  lowering 1 箇所を直せば両方に反映される。
- **JIT** だけは歴史的経緯で AST → cranelift 直結の独立経路。
- 4 バックエンドの一致は `compiler/tests/consistency.rs` の `assert_consistent`
  で常時検証している（後述）。

> **背景**: もともと tree-walker / AOT / JIT の 3 経路が各々独立に言語仕様を
> 解釈しており、新機能ごとに 3 箇所更新する整合性コストが恒常的に発生していた。
> `~/.claude/plans/interpreter-byte-code-ir-ffi-sparkling-fog.md` の計画に基づき、
> interpreter を「既存 AOT IR をそのまま実行する VM」へ作り替える作業を進行中
> （Phase 0〜4）。最終的に tree-walker を撤去し、IR を真実の単一ソースにするのが
> ゴール。

## crate 構成と依存グラフ

```
frontend            … lexer (rflex) + AST pool + type checker。依存なし
  ↑
compiler_core       … CompilerSession（parse + typecheck の薄いラッパ）。→ frontend
compiler_ir         … 共有 IR 型 (Module/Function/InstKind/Type/...) + layout。依存なし
compiler_lower      … AST → IR lowering。→ frontend, compiler_ir
  ↑                              ↑
interpreter ────────────────────┘   … tree-walker + JIT + IR VM + 共有 runtime
  → frontend, compiler_core, compiler_ir, compiler_lower
  ↑
compiler            … AOT codegen + driver + CLI。
  → frontend, compiler_core, compiler_ir, compiler_lower, interpreter
```

依存の要点：

- **`compiler` → `interpreter`**（`check_typing_with_core_modules` を借りる）の一方向
  依存があるため、`interpreter` は `compiler` に依存できない。
- そこで lowering を独立 crate **`compiler_lower`** に切り出した（Phase 4-step1）。
  これにより `interpreter` が `compiler_lower::lower_program` を呼んで IR を生成
  → IR VM で実行できるようになった（循環依存の解消）。
- `compiler` 側は `pub use compiler_lower as lower;` で後方互換を維持。

| crate | 行数(概算) | 役割 |
|---|---|---|
| `compiler_ir` | ~1.7k | IR 型定義 + `layout.rs`（leaf-flatten） |
| `compiler_lower` | ~16k | AST → IR lowering（24 ファイル） |
| `compiler/src/codegen` | ~3.3k | IR → cranelift object |
| `interpreter/src/evaluation` | ~6k | tree-walker |
| `interpreter/src/jit` | ~9.3k | cranelift JIT |
| `interpreter/src/ir_vm` | ~3.1k | IR VM |

## パイプライン

```
source (.t)
  │  frontend: lexer (rflex, lexer.l) → tokens
  │            parser → AST（StmtPool / ExprPool / LocationPool のメモリプール）
  │            type checker（context-based 推論 + 自動変換）
  ▼
type-checked File（+ core modules を auto-load して統合）
  ├──────────────► Tree-walker: AST を直接評価
  │
  └─ compiler_lower::lower_program(File, interner, contract_msgs, release)
        ▼
     compiler_ir::Module
        ├──────────► AOT: codegen → cranelift IR → object → cc/ld でリンク → 実行ファイル
        └──────────► IR VM: ir_vm::run_module → flat slot 実行

  （JIT は別経路: AST → jit/codegen → cranelift → メモリ上の関数ポインタを直接 call）
```

## 共有 IR（`compiler_ir`）

AOT と IR VM が共有する中間表現。SSA ではなく **typed local + 明示的 load/store**
のシンプルな形（SSA 構築は cranelift の `FunctionBuilder` 側、IR VM は不要）。

### 主要型

- **`Module`**: `functions: Vec<Function>` + 型テーブル（`struct_defs` / `tuple_defs` /
  `enum_defs`）+ dyn trait 情報（`trait_method_order` / `vtables`）。
- **`Function`**: `params: Vec<Type>` / `locals: Vec<Type>`（index 0..params が引数）/
  `blocks: Vec<Block>` / `entry: BlockId` / `linkage` / `return_type` /
  `address_taken_locals` / `dyn_coerce_slots` / `param_dyn_trait` /
  `self_writeback_types` / `array_slots`。
- **`Block`**: `instructions: Vec<Instruction>` + `terminator: Option<Terminator>`。
- **`Instruction`**: `result: Option<(ValueId, Type)>` + `kind: InstKind`。
- **`Linkage`**: `Export`（main のみ） / `Local`（`toy_` prefix） / `Import`（extern、本体なし）。

### `Type`（15 variant）

`I64 U64 I8 U8 I16 U16 I32 U32 F64 Bool Unit Struct(StructId) Tuple(TupleId) Enum(EnumId) Str`

- ポインタは `U64`（専用 Ptr 型はない）。
- **compound（Struct/Tuple/Enum）は leaf-flatten**：`compiler_ir::layout::flatten_compound_leaf_types`
  が compound 型を scalar leaf 列に展開し、各 leaf を別々の `LocalId` / cranelift slot に
  割り当てる。AOT と IR VM が同じ resolver を共有することで構造的に整合する。
  - struct: フィールド宣言順に leaf を展開。
  - enum: `[tag: u8][variant payload leaves...]`、サイズは `1 + max(variant payload)`。
  - byte size は `compute_byte_size`（natural-sum レイアウト、`__builtin_ptr_read/write` 互換）。

### `InstKind`（46 variant）

カテゴリ別の代表：

- **scalar**: `Const`, `BinOp`, `UnaryOp`, `Cast`, `LoadLocal`, `StoreLocal`
- **call**: `Call`（scalar 戻り）, `CallStruct` / `CallTuple` / `CallEnum`（compound 戻りを
  `dests: Vec<LocalId>` に leaf 分配）
- **heap / pointer**: `HeapAlloc`, `HeapRealloc`, `HeapFree`, `PtrRead`, `PtrWrite`,
  `PtrIsNull`, `PtrEq`, `MemCopy`
- **array**: `ArrayLoad`, `ArrayStore`, `ArrayElemAddr`（固定長 array slot）
- **allocator**: `AllocPush`, `AllocPop`, `AllocCurrent`（`with allocator` の active stack）
- **string**: `ConstStr`（interner 経由）, `ConstStrBytes`（raw bytes）, `StrLen`,
  `StrConcat`, `ToString`
- **print**: `Print`（scalar）, `PrintStr`（literal symbol）, `PrintRaw`（固定文字列）
- **reference**: `AddressOf`（address-taken local の参照）, `LoadRef`, `StoreRef`
- **`&mut self` writeback**: `CallWithSelfWriteback`, `CallWithSelfWritebackCompound`
  （callee が `[ret_leaves..., self_writeback_leaves...]` を multi-value で返す）
- **closure**: `FuncAddr`（関数ポインタ）, `MakeClosure`（env を heap 構築）,
  `CallIndirect`（env-based indirect call）
- **dyn trait**: `VtableAddr`, `DynCoerceSlotAddr`, `CallIndirectFn` /
  `CallIndirectFnStruct` / `CallIndirectFnTuple` / `CallIndirectFnEnum`

`Terminator`（5 variant）: `Return(Vec<ValueId>)`（multi-value 可） / `Jump(BlockId)` /
`Branch { cond, then_blk, else_blk }` / `Panic { message: Symbol }` / `Unreachable`。

> **新 `InstKind` 追加時のチェックリスト**：VM dispatch (`ir_vm/dispatch.rs`) /
> AOT codegen (`compiler/src/codegen/lower_inst.rs`) / IR Display / IR VM eligibility
> の 4 箇所を更新する。

### `str` のランタイムレイアウト

AOT と IR VM で **byte-uniform**（`__builtin_str_to_ptr` 互換）：

```
[bytes...][NUL][u64 len LE]
 ^               ^
 byte_start      str 値はここ（len フィールド）を指す
```

`str` 値 = len フィールドのアドレス。`byte_start = str_ptr - len - 1`。
`StrConcat` / `ToString` / `ConstStr` も同レイアウトの heap str を返す。
AOT は `compiler/runtime/toylang_rt.c` の `toy_str_alloc` / `toy_str_concat` /
`toy_to_string_<ty>`、IR VM は `ir_vm/heap.rs` の `alloc_str_bytes` / `concat_strings` /
`read_str` で対称実装。

## Lowering（`compiler_lower`）

`lower_program(&File, &interner, &ContractMessages, release: bool) -> Module`。
type-checked AST を walk して `Module` を生成する（24 ファイル、`FunctionLower` に
per-feature の `impl` を分割）。

- **storage model**: `val` / `var` / 引数は typed local slot に住み、`LoadLocal` /
  `StoreLocal` で読み書き。SSA 構築はバックエンド側。
- **`release`**: `false` で contract（`requires` / `ensures`）を `Branch + Panic` として
  IR に埋め込む。`true` で skip（D の `-release` 相当）。
- compound 値は leaf-flatten して複数 local に展開（`bindings.rs`）。
- **`ContractMessages`**: contract 違反の panic message symbol。`compiler_lower` に定義し、
  `compiler` は再エクスポート。

contract は専用命令ではなく `Branch { cond, pass, fail } + fail: Panic { message }` として
lowering 時に埋め込まれるため、バックエンドは Branch / Panic を扱えれば自動的に対応する。

## 各バックエンド詳細

### Tree-walker（`interpreter/src/evaluation/`）

- 値は `Rc<RefCell<Object>>`（`RcObject`）。`Object` enum が全ランタイム値
  （`Int64` / `Float64` / `Struct` / `EnumVariant` / `Array` / `Dict` / `String` /
  `ConstString` / `Closure` / `Allocator` / ...）。
- `EvaluationContext` が環境 / メソッドレジストリ / enum・struct レジストリを保持。
- 言語全機能をサポートするリファレンス実装（dict、generics、エラー、negative index、
  value-based `__builtin_sizeof` 等、compiler MVP を超える機能も含む）。
- **意味論の正本**。`assert_consistent` の比較基準。

### AOT compiler（`compiler/src/codegen/`）

- `Module` を入力に cranelift IR を生成 → object file → `cc` / `ld` でリンク → 実行ファイル。
- `build_object_module` は 2 phase（関数ごとの lower を rayon で並列 → `define_function_bytes`
  を逐次）。
- ランタイムは `compiler/runtime/toylang_rt.c`（str helper、panic、print、to_string 等）を
  リンク。`Linkage::Import` の extern は `-l<name>` でリンカに委譲。
- **dyn trait**: fat pointer `(data_ptr, vtable_ptr)`、`toy_vtable_*` global data に
  関数アドレス reloc、per-method thunk（`(data_ptr, ...args)` 統一 ABI）。
  macOS では `__bss` の関数アドレス reloc が ld bug を踏むため `define(zeros)` で
  `__DATA,__data` 配置 + lazy vtable emit。
- **closure**: env-based ABI（`MakeClosure` で `[fn_ptr, cap0, ...]` を malloc、`CallIndirect`
  で env+0 から fn_ptr load + env を第 1 引数に）。

### Cranelift JIT（`interpreter/src/jit/`）

- AST → cranelift を直結（IR を経由しない独立経路）。`INTERPRETER_JIT=1` で有効。
- `jit/eligibility/` が対応可否を静的解析し、未対応構文は **silent fallback**（tree-walker）。
- ランタイムヘルパは `jit/runtime.rs`（~63 個の `jit_*` 関数：heap_alloc / str_concat /
  to_string / print / panic 等）を cranelift から直接 call。
- str は同形 layout だが **function 境界（param/return）は禁止**（Object lifecycle 整合性）。
- `compile_to_jit_main` がキャッシュ（program ポインタ identity がキー）。

### IR VM（`interpreter/src/ir_vm/`）

`Module` を flat slot で直接 interpret する VM。**opt-in**（`TOY_IR_VM=1`）。

モジュール構成：

| ファイル | 役割 |
|---|---|
| `mod.rs` | `Vm` / `run_loop` / `run_module*` / call frame 管理 |
| `slot.rs` | `RawSlot`（8-byte union: i64/u64/f64/bool/ptr） |
| `frame.rs` | `CallFrame`（locals / values / value_types / array_bases / addr_cells / dyn_coerce_addrs） |
| `dispatch.rs` | `InstKind` ごとの実行 + `eval_binop` / `eval_unaryop` / `eval_cast` |
| `heap.rs` | heap 操作（共有 `HeapManager` 経由）+ str helper |
| `eligibility.rs` | reachability ベースの実行可否判定 |
| `lift.rs` | `compiler_lower` を呼んで lowering + 実行（`try_execute_main`） |

設計の要点：

- **値表現**: `RawSlot`（固定 8-byte union）。型は静的（`Function::locals[i].ty` /
  `inst.result.1`）に既知なので動的タグ不要。compound は leaf-flatten で複数 `LocalId` に展開、
  大きな値（Array / Dict / String）は heap に置き slot からは `ptr` で間接参照。
  - **型ディスパッチ**: `BinOp` / `UnaryOp` は `CallFrame::value_types`（per-value 型追跡）で
    operand 型を引き、f64 / signed / unsigned を区別。`RawSlot::from_bool` は 8-byte zero-extend
    （上位バイトに garbage を残さない）。
- **call frame**: `frames: Vec<CallFrame>`。`Return` で multi-value を caller の `return_dest`
  （scalar）/ `return_dests`（compound leaf）に分配。
- **reference**: address-taken local は heap cell で backing し、`read_local` / `write_local` を
  cell 経由にルーティング。`AddressOf` が cell アドレスを返し、`&mut T` の cross-call
  mutation propagation が成立。
- **dyn trait**: `VtableAddr` で vtable を heap に lazy materialize（FuncId 列）、`CallIndirectFn`
  で FuncId へ dispatch。`&mut dyn` writeback は `CallWithSelfWriteback` の multi-value 返却を
  combined dest list で受ける。
- **eligibility（reachability ベース）**: `main` から static call graph（Call 系 + `FuncAddr` /
  `MakeClosure` + `VtableAddr`→vtable thunk）を BFS し、到達集合のみ検査する。到達する body-less
  関数（実際に呼ばれる `Import` extern）があれば up-front reject（mid-run の副作用＝print 二重
  出力を回避）、indirect edge 経由の漏れは `run_loop` の runtime guard（空 blocks で clean
  diverge）が捕捉。これにより auto-load される prelude の **未使用 extern** で全 module が
  不可判定される問題を回避（カバレッジ ~15% → ~70%）。
- **fallback**: 非対応 / 非 scalar・非 str 戻り / lower 失敗 / diverge 時は `None` を返し、
  `execute_program` が tree-walker に透過 fallback。
- **負数 array index**: `compiler_lower` 側の `resolve_const_index` で定数 index を負数調整
  （`a[-1]` = 末尾）。AOT / IR VM 共通。

## 実行ディスパッチ（`interpreter::execute_program`）

```
execute_program(File, interner, ...) -> RcObject
  1. #[cfg(jit)]  jit::try_execute_main      （INTERPRETER_JIT=1 のとき。JIT が勝つ）
  2.              ir_vm::lift::run_main_via_ir_vm（Phase 4 デフォルト。compound 戻りも対応）
  3.              tree-walker（eval.evaluate_function）   ← fb_lower_err / ineligible の fallback
```

- JIT を先に試すことで「JIT 有効時は JIT が勝つ（JIT 固有テストを shadow しない）、
  JIT 無効時は IR VM がデフォルト」という優先順位（Phase 4-step10）。
- IR VM は env 非依存 (`run_main_via_ir_vm`) で常に最初に試行。`Some` を返せば早期 return、
  `None` なら tree-walker へ fall through（compiler MVP gap / ineligible プログラムの救済）。
- **tree-walker は fallback のみ**。新規テストは IR VM lane で pin する。

## 共有ランタイム（`interpreter/src/runtime_state.rs`）

- thread-local `RT: RefCell<Option<RuntimeState>>` を tree-walker / JIT / IR VM が共有。
- `RuntimeState { heap: Rc<RefCell<HeapManager>>, registry: Vec<Rc<dyn Allocator>>, active: Vec<usize> }`。
- **`HeapManager`**（`interpreter/src/heap.rs`）: 1-based 連続 byte buffer + typed-slot map
  （`(addr, offset) → RcObject`）。`alloc` / `realloc` / `free` / `read_u64` / `write_u64` /
  `copy_memory` / `read_bytes_raw` / `write_bytes_raw`（interior アドレス対応、str layout 用）。
- **allocator stack**: `with allocator = ...` の lexical scope を `active` で push/pop。
  heap builtin は常に現在の allocator を経由。stdlib `core/std/allocator.t` に
  `Global` / `Arena` / `FixedBuffer`。

## 整合性検証

### 4-way consistency（`compiler/tests/consistency.rs`）

`assert_consistent(source, stem)` が **interpreter（tree-walker） / AOT / JIT / IR VM** の
exit code を `& 0xff` で比較。IR VM lane は `ir_vm_supported` が eligible のときのみ参加
（unsupported は silent skip）。新機能は基本ここに 1 ケース足して 4-way で pin する。

### IR VM ⇄ tree-walker parity（`interpreter/tests/ir_vm_engine_parity.rs`）

stdlib に依存しない corpus を tree-walker と IR VM の **両方**で実行し、`Object` 完全一致を
assert。`assert_consistent` の `& 0xff` マスクで隠れる差（bool union garbage、f64 演算、
str 内容比較等）を full-value 比較で検出する目的。

### カバレッジ計測

`TOY_IR_VM_TRACE=1` で `run_main_via_ir_vm` が結果カテゴリを stderr に出力：
`IRVM_TRACE <ran | fb_ineligible | fb_lower_err | fb_nonscalar_return | fb_diverge>`。
全 interpreter テストを `TOY_IR_VM=1 TOY_IR_VM_TRACE=1` で流して集計すると、実行プログラムの
IR VM カバレッジを定量化できる（現状 ~70%）。

### テスト実行

```bash
# 全 workspace（nextest 推奨、PROPTEST_CASES=32 必須）
PROPTEST_CASES=32 cargo nextest run
# IR VM lane を強制（validation）
TOY_IR_VM=1 PROPTEST_CASES=16 cargo nextest run -p interpreter
```

## 現状と既知の差分（2026-05 時点）

IR VM は Phase 3 で全 `InstKind` を実装済み。Phase 4 で crate 再編 + fallback 配線 +
reachability eligibility + 各種ギャップ解消を実施し、`TOY_IR_VM=1` での tree-walker との
divergence は **2 件**（`__builtin_sizeof`）まで縮小：

- **`__builtin_sizeof`（enum / generic）×2**: tree-walker は runtime 値ベース（variant 固有、
  `None=1` / `Some=9`）、compiler/IR VM は型ベース（`max variant=9`）。**型ベースが canonical**
  と決定済み（C/Rust `size_of` と一致、`List<T>` の要素サイズ算出に必須）。frontend 定数畳み込み
  pass か tree-walker 削除時に自然解消。generic 経由の `sizeof(bool)=8` は別途 compiler
  monomorphization のバグ。

解消済みの主な意味差（参考）：

- **`val` 再代入**: frontend type checker で compile-time error 化（spec 準拠）。
- **負数 array index**（`a[-1]`）: `compiler_lower` の `resolve_const_index` で定数 index を
  負数調整。AOT / IR VM 共通。
- **str repr**（`ConstString` / `String`）: str-returning main で interner 既存判定により
  literal は `ConstString`、computed は `String` を返す近似。
- **struct field mutation の method 越し可視性**: spec（docs/language.md）は `self: Self` を
  by-value（mutation は local）、`&mut self` のみ writeback で伝播と規定。tree-walker は
  `Rc<RefCell>` 共有で `self: Self` も伝播させていた（spec 違反）。AOT / IR VM は spec 準拠
  （by-value）。該当テストを spec 準拠の `&mut self` に修正し 4-way 一致（tree-walker の
  `self: Self` 過剰共有は削除時に解消）。

`fb_nonscalar_return`（struct/tuple/enum/array を返す main）は **2026-06-01 に解消**。
`lift.rs` の `reconstruct_object` で `compiler_ir::Module` の `struct_defs` / `enum_defs` /
`tuple_defs` を参照し、flat leaf slots から `Object::Struct` / `Tuple` / `EnumVariant` /
`Array` を再構築。`execute_program` は IR VM をデフォルトに変更し、tree-walker は
`fb_lower_err` / `fb_ineligible` 等の最後の fallback に。

`fb_lower_err` は compiler MVP の真のカバレッジギャップ（dict 等、および closure print /
closure binding copy 等の lower 制限）。tree-walker 撤去（Phase 4 完了）にはこれらの
解消が前提。

## 関連ドキュメント

- 言語仕様: [`docs/language.md`](../docs/language.md)
- JIT 詳細・ロードマップ: [`design-docs/JIT.md`](JIT.md)
- ビルトイン関数: [`design-docs/BUILTIN_ARCHITECTURE.md`](BUILTIN_ARCHITECTURE.md)
- allocator: [`design-docs/ALLOCATOR_PLAN.md`](ALLOCATOR_PLAN.md)
- テスト戦略: [`design-docs/TEST_PLAN.md`](TEST_PLAN.md)
- 進捗・タスク: [`design-docs/todo.md`](todo.md)
