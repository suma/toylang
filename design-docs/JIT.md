# Cranelift JIT

> Implementation-side companion to the language reference. For language
> syntax and semantics see [`docs/language.md`](../docs/language.md); for
> the binary's CLI / env vars see
> [`interpreter/README.md`](../interpreter/README.md).

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
| `i8` / `i16` / `i32` (NUM-W) | sign-extended at ABI | I8 / I16 / I32 |
| `u8` / `u16` / `u32` (NUM-W) | zero-extended at ABI | I8 / I16 / I32 |
| `f64` | f64 (IEEE 754) | F64 |
| `bool` | u8 (0 or 1) | I8 |
| `ptr` | u64 (heap address) | I64 |
| `Allocator` | u64 (registry handle) | I64 |
| `str` | i64 pointer to `[bytes][NUL][u64 len LE]` heap blob (string-interpolation only — see *Strings* below) | I64 |
| `Unit` | — | none |
| `Never` | bottom — branch unification only (`panic`) | none |

Narrow integer ABI: signed variants are sign-extended and unsigned
variants are zero-extended at function-call boundaries via
`make_signature` so calls match the platform's calling convention.

`Array`, `Dict`, `Range`, and unbounded generic type parameters are
**not** supported. `Struct`, flat scalar `Tuple`, `Enum`, and `str`
*are* supported with caveats — see the dedicated subsections.

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
| `panic("literal")` | argument must be a parse-time `Expr::String(sym)`; codegen passes the symbol id as a u64 to `jit_panic`, which resolves it through a thread-local pointer to the program's `StringInterner` and `process::exit(1)`s. After the call, codegen emits `trap UserCode(1)` purely as a CFG terminator (always dead at runtime). The eligibility return type is `ScalarTy::Never`, which `unify_branch` treats as a wildcard, so both statement-position panics (`if cond { panic("…") }`) **and** expression-position panics (`if cond { panic("…") } else { value }`) compile — the panicking branch's `trap` keeps it from reaching the cont block, leaving the value-producing branch as the sole predecessor of the merged result. |
| `assert(cond, "literal")` | message must be a string literal (same constraint as `panic`); the condition is any bool expression. Lowered to `brif cond, cont_blk, fail_blk; fail_blk: call jit_panic(msg_sym); trap UserCode(1); cont_blk: …` so the success path costs one branch and the failure path reuses the panic helper unchanged. |
| `String` literal + interpolation | `"hello {x}"` lowers through `jit_string_literal` (interner-symbol fast path) + per-piece `jit_to_string_<ty>` + `jit_str_concat`. See *Strings* below. |
| `__builtin_to_string(x)` | scalar primitive `x` only; routes to the `jit_to_string_<ty>` matching the static value type. |
| `s.concat(t)` | inherent `String` / `str` method; lowers to `jit_str_concat`. |
| `match` expression | scalar / enum scrutinees with literal patterns, enum variant patterns, payload binding, wildcards. Arm bodies must yield the same `ScalarTy`. Match guards (`if cond` arms) are still rejected. |
| Enum constructor (`Option::Some(x)`, user enums) | tuple + unit variants; payloads must be JIT scalars. Lowers via the JE-2/3/4/5/6 path. See *Enums* below. |

### Statements

`Stmt::Expression`, `Stmt::Val`, `Stmt::Var`, `Stmt::Return`, `Stmt::For`
(both `to` and `..` ranges; `for x in EXPR` iterator-protocol form is
parser-desugared to `while + match Option::Some(x)/None` so it inherits
JIT support automatically when the iterator's `next()` is JIT-eligible),
`Stmt::While`, `Stmt::Break`, `Stmt::Continue`. `StructDecl` /
`ImplBlock` / `EnumDecl` cause the enclosing function to be rejected.

**Labelled loops** (`@label: while ... { break @label; continue @label }`)
are supported: `LoopFrame.label` is tracked per loop and `break` /
`continue` resolve via reverse search up the loop stack to the matching
label, falling back to the innermost frame when the operand is omitted.

**`if val PAT = EXPR { ... }` and `while val PAT = EXPR { ... }`** are
parser-desugared to `match` (and `while + match`) so they inherit JIT
support automatically when the scrutinee and patterns are JIT-eligible.

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

### Strings (interpolation only)

String literals (`"hello"`) and interpolated strings (`"sum={a + b}"`)
JIT, but `str` values cannot cross function boundaries — `check_signature`
rejects any function with a `str` parameter or return type, so strings
must originate **and** be consumed inside a single JIT-compiled function.
The typical shape is "build an interpolation chain, hand it to
`println(s)`":

```rust
fn report(n: i64, total: u64) -> u64 {
    println("processed {n} items, total = {total}")
    0u64
}
```

Lowering routes through Rust `extern "C"` runtime helpers:

| Helper | Role |
|---|---|
| `jit_string_literal(sym_id)` | resolve interner symbol → heap-allocated str blob |
| `jit_str_concat(a, b)` | concatenate two `str` values into a fresh blob |
| `jit_to_string_<ty>(x)` | scalar → str (one helper per `i64 / u64 / f64 / bool / str / i8 / u8 / i16 / u16 / i32 / u32`) |
| `jit_print_str(s)` / `jit_println_str(s)` | stdout, no-newline / with-newline |

Heap layout is **pointer-uniform** with the AOT and compiler-side JIT
str layouts: `[bytes][NUL][u64 len LE]`, where the `str` value points at
the `u64 len` field. Allocations are made via libc malloc and leaked at
process exit (interpolation strings are typically short-lived; freeing
them across compiled / interpreted code paths would require a tagged
discipline the JIT does not have today).

`s.concat(t)` (inherent `String` method) and `__builtin_to_string(x)` are
lowered through the same helpers, so `s.concat(t)` chains compose with
interpolation seamlessly. `s.len()` / `s.substring(...)` / other str
builtins still cause a fallback (the JIT does not yet model the byte
buffer at the value level).

### Enums

Tuple-payload and unit variants of both `enum Option<T>` / `enum Result<T, E>`
(stdlib) and user-defined enums JIT through the JE-2 → JE-6 family:

* **JE-2** — non-generic tuple variants (e.g. `enum Shape { Circle(i64), Rect(i64, i64) }`).
* **JE-3 / JE-4** — generic enums with one or more type parameters
  (`Option<T>`, `Result<T, E>`, user-defined `Box<T>`).
* **JE-5** — generic enums at function boundaries: an `Option<i64>` /
  `Result<u64, i64>` can be a parameter or a return type, expanding into
  a `(tag, payload_slot...)` multi-value at the cranelift signature.
* **JE-6** — receiver-method dispatch on enums: `opt.unwrap_or(0i64)`
  reaches into the impl block and calls the method like any free
  function with the enum receiver expanded.

Match arms (literal patterns / enum variants / payload binding /
wildcards) lower to a tag-dispatch `br_table` (or compare-and-branch
chain) plus per-arm payload reads. All arms must agree on `ScalarTy`.

Out of scope (silent fallback):

* Enums whose payloads are non-scalar (`enum E { Pair((i64, i64)) }` or
  enum payloads carrying a struct).
* Nested generic enums (`Option<Option<T>>`) — payload reads cannot yet
  recurse through the layout table.
* Match arm guards (`if cond`).

### Match

`match` over scalar (i64 / u64 / bool / str-equality) and enum
scrutinees compiles directly. The codegen pipeline is shared between
plain `match` expressions, the `if val PAT = EXPR` parser-desugar, and
the `while val PAT = EXPR` parser-desugar — there is no separate JIT
path for `if val` / `while val`.

### Generic functions

A generic function `fn id<T>(x: T) -> T { x }` is monomorphized per call
site: each unique combination of substituted scalar types becomes its
own cranelift function (e.g. `id__I64` and `id__U64`). Generic bounds
(`<A: Allocator>`) are still rejected, and a generic function body
cannot use `__builtin_ptr_read` because the per-call expected type
cannot be expressed in the shared hint table.

### Allocators

`with allocator = expr { … }` blocks compile when `expr` is any
expression of type `Allocator`. The JIT runtime maintains a registry
of allocator instances plus an active stack; the `with` block lowers
to a `push` of the chosen allocator before the body and a `pop`
after, with `heap_alloc` / `heap_free` / `heap_realloc` dispatching
through the active allocator. `return` / `break` / `continue` inside
a `with` body are supported: the codegen tracks the active push depth
and emits the matching pops before each early-exit terminator (every
`with` for `return`; the `with`s opened inside the loop for `break` /
`continue`).

The supported allocator-producing expressions are:

* `__builtin_default_allocator()` — process-wide global allocator.
* `__builtin_current_allocator()` — top of the active stack.
* The stdlib wrapper constructors `Arena::new()` /
  `FixedBuffer::new(capacity)` / `Global::new()`. These flow through
  `core/std/allocator.t` as nominal `struct` values; the compiler /
  JIT auto-extracts the inner `Allocator` field at the `with` boundary
  so user code does not have to write `with allocator = arena.h { … }`.

(The earlier `__builtin_arena_allocator()` / `__builtin_fixed_buffer_allocator(cap)`
runtime builtins were retired when the runtime arena / fixed-buffer
infrastructure moved into the stdlib — see commit `dc720a3`. New code
should use the stdlib `Arena::new()` / `FixedBuffer::new(cap)` form.)

```rust
val arena = Arena::new()
val total: u64 = with allocator = arena {
    val p: ptr = __builtin_heap_alloc(8u64)
    __builtin_ptr_write(p, 0u64, 12345u64)
    val x: u64 = __builtin_ptr_read(p, 0u64)
    x
}
```

### Panic / assert

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
or `panic(some_str_var)` falls back. Expression-position panic also
JITs:

```rust
fn divide(a: i64, b: i64) -> i64 {
    val q: i64 = if b == 0i64 { panic("division by zero") } else { a / b }
    q
}
```

The panicking branch types as `ScalarTy::Never` — the bottom type that
unifies with any sibling type via `ScalarTy::unify_branch` — so the
if-expression takes the value branch's type (`i64` here). Codegen
marks the panic block as `terminated`, so it never jumps to the
merged cont block; only the value-producing branch supplies the
block param, and the cranelift verifier sees a single predecessor.

`assert(cond, "literal")` reuses the same `jit_panic` helper through
a conditional branch:

```text
assert(b != 0i64, "divisor must be non-zero")
↓ codegen
  cond_v = ...                ; bool: b != 0
  brif cond_v, cont, fail
fail:
  call jit_panic(<msg_sym>)
  trap UserCode(1)
cont:
  ; control resumes here for the cond=true case
```

The condition is any bool expression. Only the message has the
literal-only constraint; everything dynamic about the assertion goes
through the cond. The success path adds exactly one branch
instruction over straight-line code.

### Not supported (silent fallback)

* Generic bounds (`<A: Allocator>`) and generic structs / generic methods.
  Per-callsite monomorphisation needs `struct_layouts` to be type-args
  aware, which is the residual blocker tracked under
  `design-docs/todo.md` #159.
* `Array`, `Dict`, `Range` values.
* Closures / lambda values — the JIT does not model `Object::Closure`.
  Skip wording: *"JIT does not yet support closure / lambda values
  (interpreter handles them; AOT support is a later phase)"*.
* Nested tuples / non-scalar tuple elements (flat scalar tuples are
  supported; see *Tuples* above).
* `str` values at function boundaries (params / returns) — strings
  must originate and be consumed inside the same JIT-compiled
  function. `s.len()` / `s.substring(...)` / other str builtins are
  also still rejected at the value level (`__builtin_str_to_ptr` /
  `__builtin_str_len` produce *"JIT does not yet model str scalar
  values"*).
* `f64` modulo (`%`) — cranelift has no native `frem`. Integer
  modulo lowers to `srem`/`urem` and is fine.
* Match arm guards (`match expr { Foo(x) if x > 0 => ... }`). Plain
  `match` (and the `if val` / `while val` parser-desugars on top of it)
  *is* supported; the guard predicate alone is the blocker.
* Enums with non-scalar payloads (struct / tuple / nested generic
  payload). Tuple + unit variants with scalar payloads JIT through
  JE-2 → JE-6.
* `panic(expr)` where `expr` is anything other than a string literal —
  e.g. `panic(SOME_CONST)` or `panic(some_str_var)`. The JIT's helper
  receives a `DefaultSymbol`'s u32 representation as a u64 immediate,
  which is only known at codegen time for inline literals.
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
  mod.rs            re-exports try_execute_main
  runtime.rs        creates the JITModule, registers extern "C" host
                    callbacks (print/println/heap/ptr_read/ptr_write/
                    panic + jit_string_literal / jit_str_concat /
                    jit_to_string_<ty> / jit_print_str /
                    jit_println_str), compiles every eligible function,
                    transmutes the finalized main pointer, calls it,
                    and wraps the scalar result back into an Object.
  eligibility/
    mod.rs          re-exports EligibilityResult and the entry point
    analyze.rs      walks the AST starting from main; produces
                    EligibleSet, ptr_read_hints, and a reject reason
    checker.rs      per-expression / per-statement eligibility checks
    collection.rs   per-program traversal helpers
    extern_dispatch.rs  per-extern signature classification
    layout.rs       struct / tuple / enum layout descriptors
    resolver.rs     identifier resolution (locals, params, free fns)
    scalar.rs       ScalarTy enum + branch unification
    signature.rs    cranelift Signature builder (ABI extension etc.)
  codegen/
    mod.rs          translates each eligible function into cranelift IR
    signature.rs    function-signature lowering shared with eligibility
    ty.rs           ScalarTy → cranelift IR Type mapping helpers
```

Host callbacks reach two pieces of state through thread-local slots
that `execute_cached` parks for the duration of the call:

- a `HeapManager` (used by `heap_alloc` / `heap_free` / `heap_realloc`
  / the `ptr_*` and `mem_*` helpers)
- a `*const DefaultStringInterner` pointing at the program's interner,
  which `jit_panic` and `jit_string_literal` dereference to resolve a
  `DefaultSymbol` (passed as a u64 immediate from JIT code) back into
  the original message / literal text

Both slots are cleared on the same drop guard so a borrow can never
outlive `try_execute_main`. The JIT and the tree-walking interpreter
currently use *separate* heaps — pointers returned from JIT main aren't
valid in the interpreter and vice versa.

## Diagnostics

Run with `-v` to see one-line outcome:

```
JIT compiled: main, fib                                       # success
JIT: skipped (function `main`: uses unsupported expression array literal)
JIT: skipped (function `f` is generic)
JIT: skipped (function `g`: ptr_read used outside a typed val/var/assign — JIT needs the result type to be statically known)
JIT: skipped (function `bar`: panic argument must be a string literal in JIT)
JIT: skipped (function `qux` has DbC contracts (not supported in JIT))
JIT: skipped (function `h`: JIT does not yet support closure / lambda values
                            (interpreter handles them; AOT support is a later phase))
JIT: skipped (function `mk`: struct literal references a generic struct
                             (JIT does not yet model generic struct values; see #159))
```

The first reject reason wins. Subsequent rejections deeper in the
recursion are ignored to keep the message close to the surface. The
generic-struct skip wording is pinned by the
`jit_skip_reason_for_generic_struct` test so the diagnostic stays
linkable to the todo entry (#159).

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

`interpreter/example/jit_*.t` are runnable smoke tests. The current set
spans every JIT-eligible feature:

| File | Covers |
|---|---|
| `jit_cast.t` | `i64` ↔ `u64` cast |
| `jit_print.t` | `print` / `println` + cross-function call |
| `jit_heap.t` | alloc / realloc / free / `ptr_is_null` / `mem_set` |
| `jit_ptr.t` | `ptr_read` / `ptr_write` round-trip across primitive types |
| `jit_sizeof.t` | `__builtin_sizeof` of every scalar |
| `jit_generic.t` | `id<T>` / `add<T>` monomorphised for multiple instantiations |
| `jit_struct.t` / `jit_struct_param.t` / `jit_struct_return.t` | scalar-field structs across param / return |
| `jit_method.t` | `impl` block method dispatch |
| `jit_allocator.t` | `with allocator = arena { … }` round-trip |
| `jit_fixed_buffer_allocator.t` | `with allocator = FixedBuffer::new(…)` round-trip |
| `jit_with_early_exit.t` | `return` / `break` / `continue` inside a `with` block |
| `jit_tuple.t` | flat scalar tuple param / return / destructure |
| `jit_tuple_inline_arg.t` / `jit_nested_tuple_fallback.t` | pin the tuple fallback shape |
| `jit_float64.t` | `f64` arithmetic, comparisons, casts, `println(f64)` |
| `jit_panic.t` / `jit_panic_expr.t` / `jit_panic_expr_fail.t` | statement / expression-position panic |
| `jit_assert.t` | `assert(cond, "literal")` with passing / failing conditions |
| `jit_tuple_enum_je2.t` / `jit_enum_boundary_je2d.t` | non-generic tuple-payload enums (JE-2) |
| `jit_generic_enum_je3.t` / `jit_multi_generic_enum_je4.t` | single- and multi-generic enums (JE-3 / JE-4) |
| `jit_generic_enum_boundary_je5.t` | generic enum at function boundary (JE-5) |
| `jit_enum_receiver_method_je6.t` | enum receiver method dispatch (JE-6) |
| `jit_unit_enum_pending.t` | unit-only enum fallback shape |
| `jit_generic_struct_fallback.t` | pins the `JIT does not yet model generic struct values` skip wording |
| `string_interpolation_jit.t` | JIT string-interpolation chain end-to-end |
| `extern_math_jit.t` | extern math intrinsics through the JIT |

`interpreter/tests/jit_integration.rs` runs each of these (plus
`example/fib.t`) under both modes and asserts exit code + stdout
match. The `unsupported_program_falls_back_silently` test verifies
that `bool_array_complex_test.t` produces a meaningful skip log and
the same end-to-end output as the interpreter.

## Future work

The bulk of the original "Phase 2" roadmap has landed (allocator
stack, fixed-buffer allocator, early-exit cleanup inside `with`
blocks, NUM-W narrow ints, string interpolation, JE-2 → JE-6 enum
support, labelled loops, `if val` / `while val`, the iterator
protocol). The remaining items tracked under
`design-docs/todo.md` #159 and `JIT-enum-1 (residual)`:

* Generic structs and generic methods (`struct_layouts` needs a
  type-args-aware refactor — diagnostic skip text already pins the
  todo entry via the `jit_skip_reason_for_generic_struct` test).
* Closures — `Object::Closure` would need a captured-environment
  representation + indirect-call dispatch.
* Nested generic enum payloads (`Option<Option<T>>`) and enums whose
  payloads are struct / tuple values.
* Match arm guards.
* Strings at function boundaries + the remaining `__builtin_str_*`
  family at the value level (`s.len()`, `s.substring(...)`, ...).
* `f64` modulo via a runtime callback into `f64::rem`.
* Lowering simple `requires` / `ensures` predicates to cranelift IR
  so contract-bearing numeric kernels can JIT (currently any contract
  forces fallback).
