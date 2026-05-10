# Builtin 関数システム アーキテクチャ

> **位置づけ**: 実装側ドキュメント。ユーザ向け builtin 一覧
> (`print` / `println` / `panic` / `assert` / `__builtin_*` 系) は
> [`docs/language.md` → Built-in functions and methods](../docs/language.md)
> を、allocator 関連は [`ALLOCATOR_PLAN.md`](ALLOCATOR_PLAN.md) を、
> JIT 側のサポート範囲は [`JIT.md`](JIT.md) を参照してください。

toylang の builtin 関数システムは **共通 frontend AST + per-backend
dispatch** で構成されている。3 backend (tree-walking interpreter /
cranelift JIT / cranelift AOT compiler) は frontend で型付けされた
`BuiltinFunction` / `BuiltinMethod` enum を共有しつつ、それぞれ
独立した実行・lowering 経路を持つ。

旧版 (2025-08-17) で提案されていた `ExecutionBackend` trait による
backend 間共通インターフェースは採用しなかった — 各 backend の
value type と runtime model が大きく異なる (interpreter は
`Rc<RefCell<Object>>`、cranelift は `ir::Value`、AOT は IR
`InstKind`) ため、共通化コストの割にメリットが小さい。代わりに
**AST レベル (BuiltinFunction enum + type checker signature) で
一元化**し、それ以下は backend ごとに最適化する方針。

## 全体図

```
            ┌────────────────────────────────────────┐
            │              frontend (Layer 1)         │
            │   BuiltinFunction / BuiltinMethod enum  │
            │     + visit_builtin_call (type check)   │
            └────────────────────────────────────────┘
                             │ shared AST
            ┌────────────────┼─────────────────────┐
            ▼                ▼                     ▼
   ┌─────────────────┐  ┌──────────────────┐  ┌────────────────────┐
   │ Interpreter     │  │ Cranelift JIT    │  │  AOT compiler      │
   │ (Layer 2a)      │  │ (Layer 2b)       │  │  (Layer 2c)        │
   │                 │  │                  │  │                    │
   │ evaluation/     │  │ jit/eligibility/ │  │ compiler/lower/    │
   │   builtin.rs    │  │ jit/codegen/     │  │ compiler/codegen   │
   │ (全 variant 直  │  │ + runtime helper │  │ + runtime helper   │
   │  実装、         │  │   (Rust mirror)  │  │   (toylang_rt.c)   │
   │  fallback 不要) │  │ 非対応 → silent  │  │ + JIT mirror       │
   │                 │  │   fallback to    │  │   (compiler/jit.rs)│
   │                 │  │   interpreter    │  │                    │
   └─────────────────┘  └──────────────────┘  └────────────────────┘
                             │
                             ▼
            ┌────────────────────────────────────────┐
            │      stdlib `core/std/` (Layer 3)        │
            │  pub fn / pub trait / extension impl    │
            │  (auto-load — 起動時に program に統合)  │
            └────────────────────────────────────────┘
```

## Layer 1: frontend AST

### `BuiltinFunction` enum

`frontend/src/ast/expr.rs::BuiltinFunction` に全 variant が並ぶ
(現状 24 個)。グループごとに分類:

| グループ | Variant | 概要 |
|---|---|---|
| メモリ管理 | `HeapAlloc` / `HeapFree` / `HeapRealloc` | active allocator 経由 |
| ポインタ操作 | `PtrRead` / `PtrWrite` / `PtrIsNull` | typed-slot ベース |
| 文字列 | `StrToPtr` / `StrLen` | str primitive 用 |
| メモリ操作 | `MemCopy` / `MemMove` / `MemSet` | アドレスベース |
| Allocator | `CurrentAllocator` / `DefaultAllocator` / `ArenaAllocator` / `FixedBufferAllocator` / `ArenaDrop` / `FixedBufferDrop` | active stack の操作 |
| I/O | `Print` / `Println` | user-facing (prefix なし) |
| 終了制御 | `Panic` / `Assert` | user-facing |
| 型検査 | `SizeOf` / `ToString` | generic stride 計算、interpolation 用 |
| 数値 | `Abs` / `Min` / `Max` | 多相 (i64 / u64 / narrow ints) |

### `BuiltinMethod` enum

`frontend/src/ast/expr.rs::BuiltinMethod` (現状 9 個)。レシーバーが
**primitive (主に `str`)** の method 呼び出しを扱う:

`IsNull` / `StrLen` / `StrConcat` / `StrSubstring` / `StrContains` /
`StrSplit` / `StrTrim` / `StrToUpper` / `StrToLower`

`String` (heap-managed nominal struct) や `Vec<T>` の method は
`BuiltinMethod` ではなく **stdlib の trait impl** 経由で dispatch される
(Layer 3 参照)。`i64.abs()` / `f64.sqrt()` も同様に extension trait に
移行済みで、ここには残っていない。

### 名前解決と型付け

- **シンボル → variant** マッピング:
  `BuiltinFunctionSymbols::symbol_to_builtin()` で `__builtin_<name>` の
  形を `BuiltinFunction` に解決。lexer は何もせず、parser/type-checker
  レベルで shape を判定する。
- **型シグネチャ**:
  `frontend/src/type_checker/visitor_impl.rs::visit_builtin_call()` が
  variant ごとに引数数・型を検査し、戻り値型を返す。`Abs` / `Min` /
  `Max` は引数型から多相的に決定 (`i64 → i64`、`u64 → u64`)、`PtrRead`
  は **return type を context から推論** (`val v: T = __builtin_ptr_read(...)`
  の `T` に従う)。

### user-facing vs 低レベル名前

- `__builtin_` prefix — 低レベル primitive。allocator / pointer 系や
  `__builtin_to_string` (string interpolation の desugar 結果) など。
- prefix なし — 日常的に使う user-facing 関数。`print` / `println` /
  `panic` / `assert` / `sizeof` / `ambient` (allocator 用糖衣)。

両者とも同じ `BuiltinFunction` enum を共有し、parser がどちらの形式
でも受理する。

### Parser-level macros (no enum variant)

一部の名前は `BuiltinFunction` enum に**載らず**、parser が見たそばから
ordinary AST に書き換える。enum を経由しないので type checker / 3
backend のどれもこれらの名前を直接知らない — 出口は普通の literal /
block / call である。

| Macro | Desugar shape | 担当ヘルパ (`frontend/src/parser/expr.rs`) |
|---|---|---|
| `__builtin_source_file()` | `Expr::String(<path>)` | `try_intercept_parser_macro` |
| `__builtin_source_line()` | `Expr::Number(<line as u64>)` | 同上 |
| `__builtin_source_column()` | `Expr::Number(<col as u64>)` | 同上 |
| `__builtin_dbg(EXPR)` | `{ val __dbg_n = EXPR; println("[file:line] text = ".concat(to_string(__dbg_n))); __dbg_n }` | `parse_dbg_macro` |
| `assert_eq(a, b)` | `{ val l=a; val r=b; assert(l==r, "<msg>".concat(...)) }` | `parse_assert_cmp_macro(equal=true)` |
| `assert_ne(a, b)` | 同上、比較を `!=` に反転 | `parse_assert_cmp_macro(equal=false)` |

source location 系は parser コンストラクタから渡された `source_file:
Option<String>` (default `"<source>"`) を `string_interner` に焼き、
`__builtin_dbg` は `Parser::source_substring(byte_range)` で原文を
そのまま切り出す (AST 再レンダリングではなく入力バッファ参照)。
`assert_eq` / `assert_ne` の panic message も parse 時にヘッダを焼き、
`a` / `b` 部分のみ runtime concat に残す。

backend 一貫性: 出力 AST が普通の expression / block / `assert` /
`println` / `__builtin_to_string` だけなので、interpreter / AOT は
実装変更不要で動く。JIT は `assert(cond, "literal")` で literal-message
を要求するため、`assert_eq` の dynamic-message を含む関数は eligibility
で fallback する (interpreter にミラー実行させる)。`__builtin_dbg` は
`println` + concat + `__builtin_to_string` の組み合わせなので、内部値が
JIT scalar の範囲内なら JIT-eligible のまま。

## Layer 2: per-backend dispatch

### 2a. Tree-walking interpreter

- **エントリポイント**: `interpreter/src/evaluation/builtin.rs::evaluate_builtin_call()`
  の巨大 match (約 900 行)。全 `BuiltinFunction` variant を網羅実装、
  fallback は不要。
- runtime: `EvaluationContext` の `heap_manager` (`Rc<RefCell<HeapManager>>`)、
  `string_interner`、`allocator_stack` を直接操作。
- メソッド側 (`BuiltinMethod`) は `evaluation/method.rs` 周辺で同様に
  match 実装。

### 2b. Cranelift JIT (interpreter 内)

- **位置**: `interpreter/src/jit/` (`eligibility/` + `codegen/` +
  `runtime.rs`)。`INTERPRETER_JIT=1` で opt-in。
- **eligibility check** (`jit/eligibility/`): 関数本体を walk し、
  対応していない builtin / 構文 / 型 (closure / enum / generic struct
  等) を含む関数を **silent fallback** 対象としてマーク → interpreter
  経路に流す。
- **codegen** (`jit/codegen/`): eligible な builtin を直接 cranelift
  instruction に lower するか、`jit/runtime.rs` の Rust helper を
  call。`jit_str_concat` / `jit_to_string_<ty>` / `jit_print_str` /
  `jit_panic` などを Rust で実装し JIT module に symbol 登録。
- 補完範囲: 数値 / bool / 一部の str。詳細は [`JIT.md`](JIT.md)。

### 2c. AOT compiler (cranelift)

- **エントリポイント**: `compiler/src/lower/expr.rs::lower_builtin_call()` —
  AST の `Expr::BuiltinCall` を IR (`compiler/src/ir.rs::InstKind`) に
  lower。
- **IR variant** (代表例): `HeapAlloc { binding }` / `HeapRealloc` /
  `HeapFree` / `PtrRead` / `PtrWrite` / `Print` / `PrintStr` /
  `StrLen` / `StrConcat` / `AllocArena` / `AllocFixedBuffer { capacity }` /
  `AllocPush` / `AllocPop` / `AllocArenaDrop` / `AllocFixedBufferDrop` /
  `Panic` / `Assert` 等。
- **codegen** (`compiler/src/codegen.rs`): IR を cranelift IR に lower。
  `RuntimeRefs` で runtime helper の `FuncRef` を保持。
- **runtime helper** (`compiler/runtime/toylang_rt.c`): C で実装された
  `toy_*` 関数群 — `toy_print_<ty>` (i64/u64/f64/bool/str + narrow ints)、
  `toy_alloc_current` / `toy_alloc_push` / `toy_alloc_pop` /
  `toy_dispatched_alloc` / `toy_arena_drop` / `toy_fixed_buffer_drop` /
  `toy_str_concat` / `toy_to_string_<ty>` 等。executable には static
  link される。
- **compiler-side JIT mirror** (`compiler/src/jit.rs`): AOT と同じ
  helper を Rust で実装して JIT module に symbol 登録。これにより
  AOT executable と compiler-internal JIT (3-way `assert_consistent`
  テスト用) で同一 semantics を保証。

> 注: interpreter JIT (2b) の `jit/runtime.rs` と compiler-side JIT
> mirror (2c の `compiler/src/jit.rs`) は **別物**。前者は interpreter
> から呼ばれる per-function JIT、後者は AOT パイプラインの中で AOT
> codegen と同じ helper を Rust で再実装したもの。symbol 名は揃えて
> あるので library 共有は可能だが、実装コードベースは独立している。

## Layer 3: stdlib (`core/std/`)

`core/std/` 配下の `.t` ファイルが auto-load 経由で全プログラムに
integrate される (詳細は [`docs/language.md` → Modules → Core modules
(auto-load)](../docs/language.md))。

| File | 提供内容 |
|---|---|
| `allocator.t` | `pub trait Alloc` + wrapper struct (`Global` / `Arena` / `FixedBuffer`)、各 `impl Drop` |
| `drop.t` | `pub trait Drop { fn drop(&mut self) }` |
| `math.t` | `math::abs` / `sqrt` / `min_*` / `max_*` / `pow` / `sin` / `cos` / `tan` / `log` / `log2` / `exp` / `floor` / `ceil` (libm 経由は `extern fn __extern_*_f64`) |
| `i64.t` / `f64.t` | extension trait (`Abs` / `Sqrt` 等) と `impl Abs for i64` のような primitive impl |
| `char.t` | `type char = u32` alias |
| `string.t` | `pub struct String` + inherent methods (`new` / `from_str` / `push` / `pop` / `len` / `as_ptr` / `eq` / `to_string` 等) |
| `str_ops.t` | extension trait `Substring` / `Trim` / `CaseConvert` / `Concat<T>` / `Contains<T>` / `Split<T, U>` |
| `option.t` / `result.t` | generic enum + method (`is_some` / `unwrap_or` / `expect` 等) |
| `collections/vec.t` | generic `Vec<T>` |
| `dict.t` / `hash.t` | dict 型 + Hash trait (extension trait over primitives) |

### dispatch パターン

- **`str` primitive method** (`s.len()` / `s.concat(...)` 等) →
  `BuiltinMethod` で直接 dispatch (Layer 1)。stdlib trait は定義
  されていない (str は primitive)。
- **`String` method** (`s.len()` / `s.push_char(...)` 等) → stdlib の
  `impl String` (inherent) または `impl <Trait> for String`
  (extension)。同じ call shape で str / String 両方が動くよう、trait
  名は `Concat<T>` / `Contains<T>` のように parameterised してある。
- **primitive 数値 method** (`n.abs()` / `r.sqrt()` 等) →
  `impl Abs for i64` / `impl Sqrt for f64` のような extension trait
  impl。3 backend の method dispatch 経路がそのまま動く。
- **`math::sin(x)` 等の transcendental** → `extern fn __extern_sin_f64(x: f64)` を
  stdlib の `pub fn sin` が wrap。各 backend は `extern fn` を:
  - **interpreter**: `evaluation::extern_math::build_default_registry`
    の `HashMap<&str, fn(&[Value]) -> Value>` で dispatch
  - **JIT**: `jit::eligibility::JIT_EXTERN_DISPATCH` で
    runtime helper またはネイティブ cranelift instruction に mapping
  - **AOT**: `Linkage::Import` で libm の `sin` 等にリンク
    (`compiler/src/lower/program.rs::libm_import_name_for`)

## 設計判断 — 採用しなかったもの

旧設計提案 (2025-08-17 版) からの変更点:

- **`ExecutionBackend` trait による共通抽象** — 採用せず。各 backend
  の value type / runtime model が大きく異なるため、AST レベルの
  共通化 (`BuiltinFunction` enum + `visit_builtin_call`) のみとした。
- **LLVM バックエンド** — cranelift を採用 (interpreter JIT / AOT
  compiler の両方)。LLVM 依存は無し。
- **Lua bytecode バックエンド** — 計画なし。
- **`compiler_core` クレート / `native/` クレート分離** — 実体は
  `frontend` / `interpreter` / `compiler` の 3 クレート構成。
- **`builtin/` ディレクトリ** — `core/std/` 配下に統合。
- **package 名 `builtin.string` 等** — `core/std/string.t` (auto-load
  時に alias `string` で参照可能)。

## 新しい builtin を追加する手順

1. **frontend AST**: `frontend/src/ast/expr.rs::BuiltinFunction` に
   variant を追加し、`BuiltinFunctionSymbols::symbol_to_builtin()` に
   `__builtin_<name>` のマッピングを追加。
2. **type checker**: `frontend/src/type_checker/visitor_impl.rs::visit_builtin_call()`
   に signature を追加 (引数数・型の検査、戻り値型の決定)。
3. **interpreter**: `interpreter/src/evaluation/builtin.rs::evaluate_builtin_call()`
   に実装を追加。3 backend の中で**唯一 fallback がない**ので、ここでは
   全ケースを実装する必要がある。
4. **JIT (任意)**: 対応するなら `interpreter/src/jit/eligibility/` で
   eligible 判定を追加 + `jit/codegen/` で cranelift lower + 必要なら
   `jit/runtime.rs` に Rust helper を追加。対応しないなら eligibility
   で reject すれば silent fallback で interpreter が肩代わりする。
5. **AOT (任意)**: 対応するなら `compiler/src/lower/expr.rs::lower_builtin_call()`
   に IR lower を追加 + 必要なら `compiler/src/ir.rs::InstKind` に
   variant を追加 + `compiler/src/codegen.rs` に cranelift lower を
   追加 + `compiler/runtime/toylang_rt.c` に C 実装の runtime helper を
   追加 + `compiler/src/jit.rs` に compiler-side JIT mirror を登録。
6. **stdlib wrapper (任意)**: user-facing にしたい場合は
   `core/std/*.t` に `pub fn` / `pub trait` / `impl` を追加。auto-load
   経由で全プログラムに自動 integrate される。
7. **テスト**: 3 backend が同じ結果を返すか `compiler/tests/consistency.rs`
   の `assert_consistent` で pin。stdlib wrapper を追加した場合は
   interpreter のみのテストを `interpreter/tests/` に追加。

## ファイル構成

```
frontend/src/
  ast/expr.rs                            BuiltinFunction / BuiltinMethod enum
  type_checker/visitor_impl.rs           visit_builtin_call (signature 検査)

interpreter/src/
  evaluation/builtin.rs                  全 variant の interpreter 実装
  evaluation/extern_math.rs              extern fn dispatch registry
  jit/eligibility/                       JIT 対応判定
  jit/codegen/                           JIT cranelift lower
  jit/runtime.rs                         JIT runtime helper (Rust)

compiler/src/
  lower/expr.rs                          AST::Expr::BuiltinCall → IR
  lower/program.rs                       libm import 名前解決
  ir.rs                                  InstKind variant
  codegen.rs                             IR → cranelift IR
  jit.rs                                 compiler-side JIT (runtime helper mirror)
runtime/toylang_rt.c                     AOT runtime helper (C)

core/std/                                stdlib (auto-load)
  allocator.t  drop.t  math.t  string.t  str_ops.t  option.t  result.t
  i64.t  f64.t  char.t  hash.t  dict.t  collections/vec.t
```

## 設計原則

1. **共通 frontend、独立 backend** — AST と型シグネチャは frontend で
   一元化、それ以下の dispatch は backend ごとに最適化。
2. **interpreter は fallback 不要** — 全 builtin を実装する責務を持つ。
   JIT は eligibility で silent fallback、AOT は実装しないと link エラー。
3. **AOT/JIT は runtime helper を共有** — `toy_*` (C) と JIT mirror
   (Rust) で symbol 名を揃え、3-way consistency テストで semantics
   一致を pin。
4. **`__builtin_*` は低レベル / prefix なしは user-facing** — 命名
   規約で意図を伝える。
5. **stdlib 経由で user-facing API を整備** — primitive impl + extension
   trait を組み合わせ、call shape の一貫性 (str / String / Vec が同じ
   method 名で呼べる) を確保。
