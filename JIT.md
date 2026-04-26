# Cranelift JIT

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
| `bool` | u8 (0 or 1) | I8 |
| `ptr` | u64 (heap address) | I64 |
| `Unit` | — | none |

`String`, `Array`, `Struct`, `Enum`, `Tuple`, `Dict`, `Range`, `Allocator`
and generic type parameters are **not** supported.

### Expressions

| Supported | Notes |
|---|---|
| `Int64`, `UInt64`, `True`, `False` | scalar literals |
| `Identifier` | parameters and locals declared via `val`/`var` |
| `Binary`: `+ - * /`, `== != < <= > >=`, `&& \|\|`, `& \| ^`, `<< >>` | arithmetic and comparisons honor signed/unsigned distinction; `&&`/`\|\|` short-circuit |
| `Unary`: `-`, `!`, `~` | `-` only on `i64` |
| `Block { stmts }` | last expression is the block value |
| `if/elif/else` | all branches must agree on type |
| `Assign(Identifier, expr)` | only to a previously declared local |
| `Call(name, args)` | callee must itself be JIT-eligible |
| `Cast(expr, T)` | only `i64` ↔ `u64` (and identity) |
| `__builtin_sizeof(probe)` | scalar probe; result is a compile-time iconst |
| `__builtin_heap_alloc / heap_free / heap_realloc` | route through `HeapManager` |
| `__builtin_ptr_is_null` | inline `icmp_imm(Equal, p, 0)` |
| `__builtin_mem_copy / mem_move / mem_set` | route through `HeapManager` |
| `__builtin_ptr_write(p, off, value)` | helper picked from the value's static type |
| `__builtin_ptr_read(p, off)` | only as the *direct* RHS of `val NAME: T = …`, `var NAME: T = …`, or `name = …` (the JIT needs a static expected type) |
| `print(x) / println(x)` | scalar arg only; calls Rust `extern "C"` helpers |

### Statements

`Stmt::Expression`, `Stmt::Val`, `Stmt::Var`, `Stmt::Return`, `Stmt::For`
(both `to` and `..` ranges), `Stmt::While`, `Stmt::Break`,
`Stmt::Continue`. `StructDecl` / `ImplBlock` / `EnumDecl` cause the
enclosing function to be rejected.

### Structs

A struct whose fields are all JIT scalars can be created and mutated
locally:

```rust
struct Point { x: i64, y: i64 }

fn main() -> u64 {
    var p = Point { x: 1i64, y: 2i64 }
    p.x = p.x + 9i64
    val total: i64 = p.x + p.y
    total as u64
}
```

Each scalar field is decomposed into its own SSA `Variable`, so reads
and writes never touch memory. Out of scope for this iteration:

* Passing structs as function parameters or returning them.
* Calling methods on struct values.
* Copying a struct between locals (`var q = p`).
* Nested struct fields.
* Generic structs (`struct Box<T> { … }`).

### Generic functions

A generic function `fn id<T>(x: T) -> T { x }` is monomorphized per call
site: each unique combination of substituted scalar types becomes its
own cranelift function (e.g. `id__I64` and `id__U64`). Generic bounds
(`<A: Allocator>`) are still rejected, and a generic function body
cannot use `__builtin_ptr_read` because the per-call expected type
cannot be expressed in the shared hint table.

### Not supported (silent fallback)

* Generic bounds (`<A: Allocator>`).
* String, Array, Struct, Enum, Tuple, Dict, Range values.
* Method calls, associated functions, field access.
* `with allocator = …` blocks and the allocator stack.
* Allocator handle builtins (`__builtin_current_allocator`,
  `__builtin_default_allocator`, `__builtin_arena_allocator`,
  `__builtin_fixed_buffer_allocator`).
* `match` expressions.

## Architecture

```
interpreter/src/jit/
  mod.rs         re-exports try_execute_main
  eligibility.rs walks the AST starting from main; produces EligibleSet
                 and ptr_read_hints, or a String reject reason.
  codegen.rs     translates each eligible function into cranelift IR.
  runtime.rs     creates the JITModule, registers extern "C" host
                 callbacks (print/println/heap/ptr_read/ptr_write),
                 compiles every eligible function, transmutes the
                 finalized main pointer, calls it, and wraps the
                 scalar result back into an Object.
```

Host callbacks reach a `HeapManager` installed in a `thread_local`
slot for the duration of `try_execute_main`. The JIT and the
tree-walking interpreter currently use *separate* heaps — pointers
returned from JIT main aren't valid in the interpreter and vice versa.

## Diagnostics

Run with `-v` to see one-line outcome:

```
JIT compiled: main, fib                                      # success
JIT: skipped (function `main`: uses unsupported expression array literal)
JIT: skipped (function `main`: uses unsupported builtin ArenaAllocator)
JIT: skipped (function `f` is generic)
JIT: skipped (function `g`: ptr_read used outside a typed val/var/assign — JIT needs the result type to be statically known)
```

The first reject reason wins. Subsequent rejections deeper in the
recursion are ignored to keep the message close to the surface.

## Performance (Apple Silicon, release)

`cargo bench --bench jit_bench --warm-up-time 1 --measurement-time 3`

| Workload | Tree-walk | JIT (incl. compile) | Speedup |
|---|---|---|---|
| `fib(20)` recursive | 13.65 ms | 107 µs | ~127× |
| `sum_to(100k)` while-loop | 51.6 ms | 134 µs | ~383× |
| `fib_iter(50k)` | 39.2 ms | 106 µs | ~371× |

The JIT timings include cranelift compilation per `execute_program`
call (no caching across runs), so the marginal compile overhead is
roughly ~100 µs for a couple of small functions. Tight numeric
loops benefit the most.

## Examples

`interpreter/example/jit_*.t` are runnable smoke tests:

* `jit_cast.t` — `i64` ↔ `u64` cast → exit 7
* `jit_print.t` — `print`/`println` + cross-function call → exit 6
* `jit_heap.t` — alloc / realloc / free / `ptr_is_null` / `mem_set` → exit 42
* `jit_ptr.t` — `ptr_read` / `ptr_write` round-trip across all four types → exit 103
* `jit_sizeof.t` — `__builtin_sizeof` of all scalar types → exit 25
* `jit_generic.t` — `id<T>` and `add<T>` monomorphized for `i64` and `u64` → exit 206
* `jit_struct.t` — `Point { x, y }` field reads / writes → exit 20

`interpreter/tests/jit_integration.rs` runs each of these (plus
`example/fib.t`) under both modes and asserts exit code + stdout
match. The `unsupported_program_falls_back_silently` test verifies
that `bool_array_complex_test.t` produces a meaningful skip log and
the same end-to-end output as the interpreter.

## Future work

Tracked under todo.md item #153 ("JIT Phase 2 拡張"):

* Struct values as function parameters / return values, and method
  dispatch.
* `with allocator = …` and the allocator stack.
* Caching compiled functions across `execute_program` calls (would
  amortize the ~100 µs cranelift overhead in repeated bench runs and
  in long-running sessions).
