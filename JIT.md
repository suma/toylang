# Cranelift JIT

> Implementation-side companion to the language reference. For language
> syntax and semantics see [`docs/language.md`](docs/language.md); for
> the binary's CLI / env vars see
> [`interpreter/README.md`](interpreter/README.md).

`interpreter` ships an optional cranelift-based JIT for numeric / boolean
code. It runs alongside the tree-walking interpreter: when enabled, the
JIT examines `main` (and every function it transitively calls). If every
reachable function fits the supported subset, the JIT compiles them into
native code and runs that. Anything outside the subset causes a silent
fallback to the interpreter — no behavior change, no error.

## Activating the JIT

| | |
|---|---|
| Build-time gate | cargo feature `jit` (on by default). `--no-default-features` disables it entirely; the `cranelift*` crates aren't even linked in that case. |
| Run-time gate | environment variable `INTERPRETER_JIT=1`. Without it, the JIT path is never entered, even when the feature is built in. |
| Verbose log | pass `-v` to the `interpreter` binary to see `JIT compiled: …` (success) or `JIT: skipped (…)` (fallback) on stderr. |

```sh
# Default tree-walk
cargo run example/fib.t

# JIT
INTERPRETER_JIT=1 cargo run example/fib.t

# JIT with diagnostics
INTERPRETER_JIT=1 cargo run example/fib.t -v
```

The numeric `main` result lands in the process exit code (Object::Int64 /
Object::UInt64 → `process::exit`), so:

```sh
$ INTERPRETER_JIT=1 cargo run example/fib.t; echo $?
8
```

## Supported subset

A function is JIT-eligible when its signature *and* its body stay inside
the supported scalar set and use only the supported syntax. The first
function `main` calls into something unsupported makes the whole
reachable set ineligible.

### Scalar types

| Toy type | JIT representation | Cranelift IR |
|---|---|---|
| `i64` | i64 | I64 |
| `u64` | u64 | I64 |
| `f64` | f64 (IEEE 754) | F64 |
| `bool` | u8 (0 or 1) | I8 |
| `ptr` | u64 (heap address) | I64 |
| `Allocator` | u64 (registry handle) | I64 |
| `Unit` | — | none |

`String`, `Array`, `Struct`, `Enum` (variants), `Dict`, `Range`, and
unbounded generic type parameters are **not** supported. (`Struct` and
flat scalar `Tuple` *are* supported — see the dedicated subsections.)

### Expressions

| Supported | Notes |
|---|---|
| `Int64`, `UInt64`, `Float64`, `True`, `False` | scalar literals; floats use `f64const` |
| `Identifier` | parameters and locals declared via `val`/`var` |
| `Binary` on integers: `+ - * / %`, `== != < <= > >=`, `&& \|\|`, `& \| ^`, `<< >>` | arithmetic and comparisons honor signed/unsigned distinction; `&&`/`\|\|` short-circuit |
| `Binary` on `f64`: `+ - * /`, `== != < <= > >=` | lowered to `fadd`/`fsub`/`fmul`/`fdiv` and ordered `fcmp` (NaN compares false against everything, matching Rust's `PartialOrd`). `%` on `f64` is **not** JIT-supported (cranelift has no native `frem`) |
| `Unary`: `-`, `!`, `~` | `-` accepts `i64` (`ineg`) and `f64` (`fneg`). `~` on integers, `!` on bool. |
| `Block { stmts }` | last expression is the block value |
| `if/elif/else` | all branches must agree on type |
| `Assign(Identifier, expr)` | only to a previously declared local |
| `Call(name, args)` | callee must itself be JIT-eligible |
| `Cast(expr, T)` | `i64` ↔ `u64` (no-op at the IR level); `i64`/`u64` → `f64` via `fcvt_from_sint`/`fcvt_from_uint`; `f64` → `i64`/`u64` via `fcvt_to_sint_sat`/`fcvt_to_uint_sat` (saturating, NaN → 0; matches Rust `as`) |
| `__builtin_sizeof(probe)` | scalar probe; result is a compile-time iconst |
| `__builtin_heap_alloc / heap_free / heap_realloc` | route through `HeapManager` |
| `__builtin_ptr_is_null` | inline `icmp_imm(Equal, p, 0)` |
| `__builtin_mem_copy / mem_move / mem_set` | route through `HeapManager` |
| `__builtin_ptr_write(p, off, value)` | helper picked from the value's static type |
| `__builtin_ptr_read(p, off)` | only as the *direct* RHS of `val NAME: T = …`, `var NAME: T = …`, or `name = …` (the JIT needs a static expected type) |
| `print(x) / println(x)` | scalar arg only; calls Rust `extern "C"` helpers (`jit_print_i64/u64/bool/f64` and `*_println_*`) |
| `panic("literal")` | argument must be a parse-time `Expr::String(sym)`; codegen passes the symbol id as a u64 to `jit_panic`, which resolves it through a thread-local pointer to the program's `StringInterner` and `process::exit(1)`s. After the call, codegen emits `trap UserCode(1)` purely as a CFG terminator (always dead at runtime). Statement-position panics (`if cond { panic("…") }`) compile cleanly; expression-position panics (`if cond { panic("…") } else { value }`) silent-fall back today because `Panic` returns `ScalarTy::Unit` and the if-branches' types must match in eligibility. |

### Statements

`Stmt::Expression`, `Stmt::Val`, `Stmt::Var`, `Stmt::Return`, `Stmt::For`
(both `to` and `..` ranges), `Stmt::While`, `Stmt::Break`,
`Stmt::Continue`. `StructDecl` / `ImplBlock` / `EnumDecl` cause the
enclosing function to be rejected.

### Structs

A struct whose fields are all JIT scalars can flow through function
parameters and return values, in addition to local mutation:

```rust
struct Point { x: i64, y: i64 }

fn make_point(x: i64, y: i64) -> Point {
    Point { x: x, y: y }
}

fn sum_xy(p: Point) -> i64 {
    p.x + p.y
}

fn main() -> u64 {
    val a = make_point(3i64, 4i64)
    val b = make_point(5i64, 6i64)
    val total: i64 = sum_xy(a) + sum_xy(b)
    total as u64
}
```

Each scalar field is decomposed into its own SSA `Variable`, so reads
and writes never touch memory. Struct parameters expand into one
cranelift parameter per field; struct returns expand into a
multi-return whose results the caller reassembles into a fresh
struct local. Arguments at call sites must be `Identifier`s
referring to a known struct local; the body of a struct-returning
function must end in either an `Identifier` or a `StructLiteral`.

Methods declared in `impl` blocks dispatch through the same path
as free functions: `p.dist_squared()` becomes a normal cranelift
call where the receiver expands into per-field arguments and any
extra arguments follow. `Self` references in the method's signature
resolve to the impl block's target struct.

Out of scope for this iteration:

* Copying a struct between locals (`var q = p`).
* Nested struct fields.
* Generic structs (`struct Box<T> { … }`) and generic methods.
* `main` returning a struct.

### Tuples

A tuple of scalar elements decomposes the same way a scalar-only struct
does: each element becomes its own SSA `Variable`, parameters expand
into one cranelift parameter per element, and tuple returns expand into
a multi-return whose values populate a fresh tuple local in the caller.

```rust
fn swap(p: (u64, u64)) -> (u64, u64) {
    (p.1, p.0)
}

fn main() -> u64 {
    val src = (10u64, 20u64)
    val (a, b) = swap(src)
    a + b
}
```

`val (a, b) = expr` destructuring works because the parser desugars
into `val tmp = expr; val a = tmp.0; val b = tmp.1`, and the JIT
matches both the `TupleLiteral` rhs (or tuple-returning call) and the
subsequent `TupleAccess` reads. Tuple arguments at a call site must be
`Identifier`s of a known tuple local — inline tuple literals as
arguments are still rejected.

Out of scope for this iteration:

* Nested tuples (`((a, b), c)`-shaped types).
* Inline `TupleLiteral` as a function argument.
* Tuples whose elements are non-scalar (struct, string, etc.).
* `main` returning a tuple.

### Generic functions

A generic function `fn id<T>(x: T) -> T { x }` is monomorphized per call
site: each unique combination of substituted scalar types becomes its
own cranelift function (e.g. `id__I64` and `id__U64`). Generic bounds
(`<A: Allocator>`) are still rejected, and a generic function body
cannot use `__builtin_ptr_read` because the per-call expected type
cannot be expressed in the shared hint table.

### Allocators

`with allocator = expr { … }` blocks compile when `expr` is one of
`__builtin_default_allocator()`, `__builtin_arena_allocator()`, or
`__builtin_current_allocator()`. The JIT runtime maintains a registry
of allocator instances plus an active stack; the `with` block lowers
to a `push` of the chosen allocator before the body and a `pop`
after, with `heap_alloc` / `heap_free` / `heap_realloc` dispatching
through the active allocator. Bodies must be linear — `return`,
`break`, and `continue` inside a `with` are rejected so the matching
pop is guaranteed to run.

```rust
val arena = __builtin_arena_allocator()
val total: u64 = with allocator = arena {
    val p: ptr = __builtin_heap_alloc(8u64)
    __builtin_ptr_write(p, 0u64, 12345u64)
    val x: u64 = __builtin_ptr_read(p, 0u64)
    x
}
```

### Panic

`panic("literal")` lowers to a Rust host helper (`jit_panic`) plus a
cranelift `trap UserCode(1)` terminator:

```text
panic("division by zero")
↓ codegen
  iconst.i64    <sym_id>          ; DefaultSymbol::to_usize() as u64
  call          jit_panic(sym_id)
  trap          UserCode(1)       ; CFG terminator; dead at runtime
```

The helper resolves the symbol id back to the original message through
a thread-local pointer to the program's `StringInterner` (parked by
`execute_cached` and cleared on the same `HeapGuard::drop` that tears
down the heap state). It then prints

```
Runtime error occurred:
panic: <message>
```

to stderr and `process::exit(1)`s. The cranelift `trap` after the call
exists only so the basic block has a recognised terminator — by the
time the trap would execute, the process is already gone. This avoids
the DWARF-unwind / signal-handler infrastructure WebAssembly engines
typically need for traps.

The argument must be a parse-time `Expr::String(sym)`; `panic(SOME_CONST)`
or `panic(some_str_var)` falls back. The JIT also rejects panic in
expression position (`if cond { panic("…") } else { 5i64 }`) because
the if-elif-else eligibility requires sibling branches to agree on a
`ScalarTy` and panic returns `Unit` while the other branch is `i64`. A
future `ScalarTy::Never` variant would lift this. The
*statement-position* form

```rust
fn divide(a: i64, b: i64) -> i64 {
    if b == 0i64 { panic("division by zero") }
    a / b
}
```

JITs cleanly because both branches of the inner `if` are `Unit`.

### Not supported (silent fallback)

* Generic bounds (`<A: Allocator>`).
* String, Array, Enum, Dict, Range values.
* Nested tuples / non-scalar tuple elements (flat scalar tuples are
  supported; see *Tuples* above).
* `f64` modulo (`%`) — cranelift has no native `frem`. Integer
  modulo lowers to `srem`/`urem` and is fine.
* `__builtin_fixed_buffer_allocator` (the quota-tracking allocator
  variant — only `default` and `arena` are wired up so far).
* `match` expressions.
* `panic(expr)` where `expr` is anything other than a string literal —
  e.g. `panic(SOME_CONST)` or `panic(some_str_var)`. The JIT's helper
  receives a `DefaultSymbol`'s u32 representation as a u64 immediate,
  which is only known at codegen time for inline literals.
* `panic(...)` in expression position — `if cond { panic("…") } else
  { 5i64 }` is rejected because eligibility requires both branches to
  agree on a `ScalarTy`. The statement-position form (`if cond
  { panic("…") }` followed by other statements) JITs cleanly.
* Functions that reference a top-level `const`. The constant is bound
  in the interpreter's environment at startup; the JIT eligibility
  walker has no view of it and rejects the unresolved identifier,
  triggering silent fallback.
* Functions and methods carrying any `requires` / `ensures` clause.
  Contract evaluation lives in the tree-walking interpreter so that the
  `INTERPRETER_CONTRACTS=all|pre|post|off` env-var gate and the
  `ContractViolation` diagnostic stay in one place. Callers see
  `JIT: skipped (function 'foo' has DbC contracts (not supported in JIT))`
  in `-v` mode.

## Architecture

```
interpreter/src/jit/
  mod.rs         re-exports try_execute_main
  eligibility.rs walks the AST starting from main; produces EligibleSet
                 and ptr_read_hints, or a String reject reason.
  codegen.rs     translates each eligible function into cranelift IR.
  runtime.rs     creates the JITModule, registers extern "C" host
                 callbacks (print/println/heap/ptr_read/ptr_write/
                 panic), compiles every eligible function, transmutes
                 the finalized main pointer, calls it, and wraps the
                 scalar result back into an Object.
```

Host callbacks reach two pieces of state through thread-local slots
that `execute_cached` parks for the duration of the call:

- a `HeapManager` (used by `heap_alloc` / `heap_free` / `heap_realloc`
  / the `ptr_*` and `mem_*` helpers)
- a `*const DefaultStringInterner` pointing at the program's interner,
  which `jit_panic` dereferences to resolve a `DefaultSymbol` (passed
  as a u64 immediate from JIT code) back into the original message text

Both slots are cleared on the same drop guard so a borrow can never
outlive `try_execute_main`. The JIT and the tree-walking interpreter
currently use *separate* heaps — pointers returned from JIT main aren't
valid in the interpreter and vice versa.

## Diagnostics

Run with `-v` to see one-line outcome:

```
JIT compiled: main, fib                                      # success
JIT: skipped (function `main`: uses unsupported expression array literal)
JIT: skipped (function `main`: uses unsupported builtin ArenaAllocator)
JIT: skipped (function `f` is generic)
JIT: skipped (function `g`: ptr_read used outside a typed val/var/assign — JIT needs the result type to be statically known)
JIT: skipped (function `bar`: panic argument must be a string literal in JIT)
JIT: skipped (function `qux` has DbC contracts (not supported in JIT))
```

The first reject reason wins. Subsequent rejections deeper in the
recursion are ignored to keep the message close to the surface.

## Performance (Apple Silicon, release)

`cargo bench --bench jit_bench --warm-up-time 1 --measurement-time 3`

| Workload | Tree-walk | JIT (cached) | Speedup |
|---|---|---|---|
| `fib(20)` recursive | 13.9 ms | 30.8 µs | ~451× |
| `sum_to(100k)` while-loop | 53.8 ms | 30.9 µs | ~1741× |
| `fib_iter(50k)` | 40.8 ms | 31.0 µs | ~1316× |

A thread-local cache keyed by `&Program` pointer-identity stores the
finalized `JITModule` after the first compile, so repeated calls to
`execute_program` (such as criterion's iter loop) skip eligibility,
codegen and finalization entirely. The remaining ~31 µs reflect the
heap install / cached-pointer dispatch / `Object` wrapping path —
the native code itself is faster.

## Examples

`interpreter/example/jit_*.t` are runnable smoke tests:

* `jit_cast.t` — `i64` ↔ `u64` cast → exit 7
* `jit_print.t` — `print`/`println` + cross-function call → exit 6
* `jit_heap.t` — alloc / realloc / free / `ptr_is_null` / `mem_set` → exit 42
* `jit_ptr.t` — `ptr_read` / `ptr_write` round-trip across all four types → exit 103
* `jit_sizeof.t` — `__builtin_sizeof` of all scalar types → exit 25
* `jit_generic.t` — `id<T>` and `add<T>` monomorphized for `i64` and `u64` → exit 206
* `jit_struct.t` — `Point { x, y }` field reads / writes → exit 20
* `jit_struct_param.t` — struct passed across a `sum_xy(Point) -> i64` call → exit 24
* `jit_struct_return.t` — `make_point(...) -> Point` factory used twice → exit 18
* `jit_method.t` — `impl Point { fn dist_squared(self: Self) -> i64 }` dispatched twice → exit 194
* `jit_allocator.t` — `with allocator = arena { … }` round-trip → exit 57 (12345 % 256)
* `jit_tuple.t` — flat tuple param / return, destructure, and `TupleAccess` → exit 33
* `jit_float64.t` — `f64` arithmetic, comparisons, casts, `println(f64)` → exit 7
* `jit_panic.t` — `panic("literal")` lowered to `jit_panic` helper + `trap` terminator → exit 1 with `panic: division by zero` on stderr

`interpreter/tests/jit_integration.rs` runs each of these (plus
`example/fib.t`) under both modes and asserts exit code + stdout
match. The `unsupported_program_falls_back_silently` test verifies
that `bool_array_complex_test.t` produces a meaningful skip log and
the same end-to-end output as the interpreter.

## Future work

Tracked under todo.md item #159 ("JIT Phase 2 拡張"):

* `__builtin_fixed_buffer_allocator` (quota-tracking allocator variant).
* `with` bodies that contain `return` / `break` / `continue` (need
  cleanup-style pop emission before the early exit).
* Generic methods and generic structs.
* `f64` modulo via a runtime callback into `f64::rem`.
* Lowering simple `requires` / `ensures` predicates to cranelift IR
  so contract-bearing numeric kernels can JIT (currently any contract
  forces fallback).
* Expression-position panic (`if cond { panic("…") } else { value }`)
  via a `ScalarTy::Never` variant that unifies with any sibling type
  in if-elif-else eligibility.
