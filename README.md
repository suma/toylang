# Programming Language Implementation in Rust

A complete programming language implementation featuring a frontend library and tree-walking interpreter, built as a learning project to explore language design and implementation techniques.

> **For language syntax and semantics, see [`docs/language.md`](docs/language.md)** —
> the consolidated reference. This README covers the project as a whole
> (overview, build, test, examples). Per-component details live in
> [`interpreter/README.md`](interpreter/README.md), [`design-docs/JIT.md`](design-docs/JIT.md),
> [`design-docs/ALLOCATOR_PLAN.md`](design-docs/ALLOCATOR_PLAN.md), and
> [`design-docs/BUILTIN_ARCHITECTURE.md`](design-docs/BUILTIN_ARCHITECTURE.md).

## Overview

This project implements a statically-typed programming language with
comprehensive type checking, automatic type inference, and modern language
features. **The same program has to mean the same thing on four engines**,
which is the constraint most of the design answers to:

| Crate | What it is |
|---|---|
| `frontend` | Parser, AST pools, type checker, and the whole-program checks (ownership, regions, effects, contracts) |
| `interpreter` | The tree-walking engine — the **reference oracle** — plus an AST-level Cranelift JIT |
| `compiler_ir` / `compiler_lower` / `compiler_vm` | The shared IR, the AST → IR lowering, and an IR interpreter (the engine `interpreter` actually uses by default) |
| `compiler` | AOT: IR → Cranelift → object file → executable, and a Cranelift JIT over the same IR |
| `compiler/runtime/toylang_rt` | The runtime both compiled lanes link: output, allocator, profiler, backtraces |
| `toy` | The build tool — `toy build` / `run` / `test` / `check` for a program with its own modules |

A larger program written **in** the language lives in
[`poc/logsearch`](poc/logsearch) — a log archiver and search server, which
is where most of the gaps in the language were found.

## Language Features

### Core Language Constructs
- **Functions** with explicit return types: `fn fibonacci(n: u64) -> u64`
- **Variables**: Immutable (`val`) and mutable (`var`) declarations
- **Top-level constants**: `const PI: f64 = 3.14159f64` evaluated once at startup
- **Control Flow**: `if/else/elif`, `for` loops with `break/continue`, `while` loops
- **Types**: `u64` / `i64` / `f64` / `f32` / `bool` / `str` / `ptr` / `usize`,
  narrow integers (`u8`–`u32`, `i8`–`i32`), `char`, 128-bit SIMD vectors
  (`f64x2` / `f32x4` / `i32x4` / `i64x2` / `u8x16`), tuples, fixed arrays,
  `dict`

### Advanced Features
- **Fixed Arrays**: `val arr: [i64; 5] = [1, 2, 3, 4, 5]` with type inference
- **Tuples**: `val (a, b) = (1u64, 2u64)` with destructuring (including nested patterns)
- **Dictionary Type**: `dict{key1: value1, key2: value2}` with Object-keyable types
- **Structures**: `struct Point { x: i64, y: i64 }` with method implementations
- **Enums and Pattern Matching**: `enum Shape { Circle(i64), Rect(i64, i64), Point }` with tuple-variant binding, literal patterns, nested patterns, and per-arm `if` guards
- **Generics with bounds**: `fn id<T>(x: T) -> T` and `fn run<A: Allocator>(a: A)`
- **Design by Contract**: `requires` (preconditions) and `ensures` (postconditions) on functions and methods, with `result` for the return value. Runtime gating via `INTERPRETER_CONTRACTS=all|pre|post|off`
- **Termination primitives**: `panic("msg")` and `assert(cond, "msg")` for explicit failure
- **Allocator system**: `with allocator = arena { … }` lexically scoped allocator binding, `<A: Allocator>` bound, arena / fixed-buffer / global allocator builtins
- **Built-in Methods**: String operations like `"hello".len()` returning `u64`
- **Unary Operators**: `-x` (signed int / `f64`), `!` (logical not), `~` (bitwise not)
- **Ownership**: a type with an `impl Drop` has one owner. Handing it to
  something that outlives the scope **transfers** it (`[E0014]`), drop glue
  frees containers recursively, and a container **lends** an element with
  `borrow` rather than handing out a second owner (`[E0028]`)
- **Effects**: what a declaration can do besides compute — `never_allocates`
  and `const fn` are checked against the same reachability walk
- **Data parallelism**: `parallel for i in 0u64..n { .. }` — the iterations
  may run in any order (every engine runs them in order today; the answer is
  fixed before the threads arrive)
- **Comments**: `# line` and `/* block */`
- **No Semicolons**: Statements are separated by newlines, not semicolons
- **Module System**: Go-style modules with `package`/`import` declarations,
  resolved by matching the **end** of a path (`math::f` finds `std.math.f`)
- **Qualified Identifiers**: Rust-style `module::function` syntax
- **A standard library written in the language itself** (`core/std/`):
  `String` / `Vec<T>` / `Box<T>` / `Dict<K, V>` / `Option` / `Result` /
  `Span<T>` / `Ptr<T>`, files, sockets, a poller, JSON, hashes, time

### Type System
- **Context-based Type Inference**: Automatic type resolution based on usage context
- **Generic Type Inference**: Constraint-based unification algorithm for automatic type parameter resolution
- **Advanced Type Checking**: Comprehensive validation with detailed error reporting
- **Memory Pool Architecture**: Efficient AST storage with `StmtPool` and `ExprPool`
- **Strict Type Safety**: No implicit type conversions; all type changes must be explicit

## Architecture

### Frontend Library (`frontend/`)
- **Lexer Generation**: Uses `rflex` crate to generate lexer from flex-style `.l` files
- **AST Design**: Memory-efficient representation with reference-based pools
- **Type Checker**: Sophisticated inference engine with caching and context propagation
- **Module Resolution**: Go-style package/import system with AST integration
- **Error System**: Structured error reporting with consistent formatting

### Interpreter (`interpreter/`)
- **Tree-walking Execution**: Direct AST traversal with `Rc<RefCell<Object>>` runtime values
- **Environment Management**: Proper variable scoping and lifetime management
- **Module Integration**: Seamless AST integration with qualified identifier support
- **Built-in Operations**: Comprehensive arithmetic, logical, and comparison operators
- **Method Registry**: Support for both struct methods and built-in type methods
- **Resource Management**: Automatic object destruction with custom destructor support

## Getting Started

### Prerequisites
- Rust 1.70+ 
- Cargo package manager

### Building

Everything runs from the repository root with `-p`; `cd <crate> && cargo …`
buys nothing and costs a directory change.

```bash
# Type errors only, as fast as they come
cargo check --workspace --message-format=short

# One crate
cargo build -p frontend
cargo build -p interpreter
cargo build -p compiler

# Release
cargo build --release -p interpreter
```

### Running Programs

```bash
# Run a program (the process exit code is the program's result)
cargo run -q -p interpreter -- interpreter/example/fib.t

# A few illustrative examples out of the ~190 in interpreter/example/
cargo run -q -p interpreter -- interpreter/example/contracts.t      # requires / ensures / result
cargo run -q -p interpreter -- interpreter/example/box_linked_list.t # Box, ownership, drop glue
cargo run -q -p interpreter -- interpreter/example/net_echo_server.t # sockets + poller
cargo run -q -p interpreter -- interpreter/example/simd.t            # 128-bit vectors
cargo run -q -p interpreter -- interpreter/example/allocator_basic.t # `with allocator = arena`

# Compile ahead of time, then run the binary
cargo run -q -p compiler -- interpreter/example/fib.t -o /tmp/fib && /tmp/fib

# Run the same program on every engine and report only disagreements
cargo run -q -p compiler -- interpreter/example/fib.t --all-backends

# Ask instead of running: error prose, a module's signatures, what a
# declaration can do besides compute
cargo run -q -p interpreter -- --explain E0028
cargo run -q -p interpreter -- --api core/std/string.t
cargo run -q -p interpreter -- --effects interpreter/example/fib.t

# `test "..." { }` blocks, and `requires` / `ensures` as a property test
cargo run -q -p interpreter -- --test interpreter/example/memory_contract.t
cargo run -q -p interpreter -- --check interpreter/example/memory_contract.t

# Print allocation totals after the run (JSON with --profile-format=json)
cargo run -q -p interpreter -- --profile=mem interpreter/example/allocator_list.t
```

### A program with its own modules (`toy`)

```bash
cargo run -q -p toy -- build poc/logsearch [--release]
cargo run -q -p toy -- run   poc/logsearch -- serve /var/log/archive 8080
cargo run -q -p toy -- test  poc/logsearch          # AOT by default, in parallel
cargo run -q -p toy -- check poc/logsearch --diagnostics=json
```

Most subcommands take `--format=json` for the result and
`--diagnostics=json` for the errors; see
[`design-docs/BUILD_TOOL.md`](design-docs/BUILD_TOOL.md).

For the full CLI / env-var reference see [`interpreter/README.md`](interpreter/README.md).

### Testing

`cargo nextest` is what the suite is tuned for — a green run prints six
lines, because the output is what makes a test run readable, not the speed.

```bash
# Everything (~3,000 tests, about 45 s)
cargo nextest run

# One crate, or a name (filters are substring matches, not exact)
cargo nextest run -p compiler
cargo nextest run -E 'test(basic_arithmetic)'

# Doc-tests, which nextest does not run
cargo test --doc --workspace
```

The tests that matter most are in `compiler/tests/consistency/`: they run
one program on every engine and compare. A check that only asserts the
engines *agree* can pass while all of them are wrong, so those tests pin
the output itself wherever the answer is known.

### Linting

The workspace uses `[workspace.lints.clippy]` in `Cargo.toml` for consistent lint rules. Run clippy across the entire workspace:

Clippy is expected to be **silent**; a warning means the change that just
landed introduced it.

```bash
cargo clippy --workspace --all-targets --all-features --message-format=short
```

## Language Syntax

### Basic Program Structure
```rust
fn main() -> u64 {
    fib(6u64)
}

fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}
```

### Variables, Arrays, and Dictionaries
```rust
fn collection_example() -> i64 {
    # Array example
    val numbers: [i64; 3] = [10, 20, 30]
    var sum = 0i64
    
    for i in 0u64 to 3u64 {
        sum = sum + numbers[i]
    }
    
    # Dictionary example
    val d: dict[str, i64] = dict{"key1": 100i64, "key2": 200i64}
    sum = sum + d["key1"]
    
    sum
}
```

### Structures and Methods
```rust
struct Point {
    x: i64,
    y: i64
}

impl Point {
    fn new(x: i64, y: i64) -> Point {
        Point { x: x, y: y }
    }
    
    fn distance(&self) -> i64 {
        self.x * self.x + self.y * self.y
    }
}
```

### Enums and Pattern Matching
```rust
# Unit variants, tuple variants with typed payloads, and a mix are allowed
enum Shape {
    Circle(i64),
    Rect(i64, i64),
    Point,
}

fn area(s: Shape) -> i64 {
    match s {
        # Tuple-variant patterns bind each slot to a name;
        # use `_` to discard a payload position you don't need.
        Shape::Circle(r) => r * r * 3i64,
        Shape::Rect(w, h) => w * h,
        # Unit variants use the bare path
        Shape::Point => 0i64,
    }
}

fn describe(s: Shape) -> i64 {
    match s {
        Shape::Point => 0i64,
        # `_` as an arm catches any remaining variant
        _ => -1i64,
    }
}

fn main() -> i64 {
    area(Shape::Circle(5i64)) + area(Shape::Rect(3i64, 4i64)) + area(Shape::Point)
}
```

Every `match` arm must produce the same result type, and a `match` has to
cover its scrutinee: a missing variant is a type error, and so is an arm no
value can reach. Beyond `Enum::Variant` and `Enum::Variant(x, _, y)` the
patterns are struct (`Point { x: 0i64, y }`), tuple, literal, or
(`1i64 | 2i64`), half-open range (`0i64..5i64`), binding (`n @ 3i64`) and
nesting of any of them — including inside a payload. Generic enums
(`Option<T>` / `Result<T, E>`) are ordinary declarations in the stdlib.

### Unary Operators
```rust
fn negate_example() -> i64 {
    val x: i64 = 7i64
    val y: i64 = -x          # signed-integer negation
    val z: bool = !(y == x)  # logical not
    val w: i64 = ~y          # bitwise not
    y
}
```

The parser also treats `-` at the start of a new source line as a fresh
unary expression, so the following parses as two statements rather than
`val a = 10 - b`:

```rust
val a: i64 = 10i64
-a
```

### Ownership and Destructors
```rust
struct FileResource {
    path: str,
    handle: u64
}

impl FileResource {
    fn open(path: str) -> FileResource {
        FileResource { 
            path: path, 
            handle: 42u64  # Simulated file handle
        }
    }
    
    fn read_data(self: Self) -> str {
        # Read operation using self.handle
        "file content"
    }
    
    # Custom destructor for cleanup
    fn drop(&mut self) {
        # Close file handle, release resources
        # Log cleanup actions, etc.
    }
}

fn main() -> u64 {
    val file = FileResource::open("data.txt")
    val content = file.read_data()
    # FileResource.drop() automatically called when 'file' goes out of scope
    0u64
}
```

A type with an `impl Drop` has **one owner**. Putting it somewhere that
outlives the scope — a by-value argument, a container, a struct field —
transfers it, and reading the old name afterwards is `[E0014]`. Ownership
is transitive, so a `Vec<Box<i64>>` frees its elements and their contents
when it dies.

Reading an element is where that gets interesting: `get` answers with the
element, which for an owning type is a shallow copy — two owners of one
resource. A container **lends** instead:

```rust
var conns: Vec<TcpStream> = Vec::new()
conns.push(cl)
val s: TcpStream = conns.get(0u64)      # [E0028]: both would close the socket
val s: &TcpStream = conns.borrow(0u64)  # names it without claiming it
```

The rules and what they cost are in
[`design-docs/ELEMENT_BORROW.md`](design-docs/ELEMENT_BORROW.md).

### Generic Programming
```rust
# Generic functions with automatic type inference
fn identity<T>(x: T) -> T {
    x
}

fn swap<T, U>(pair: (T, U)) -> (U, T) {
    (pair.1, pair.0)
}

# Generic structures with type parameter inference
struct Container<T> {
    value: T
}

impl<T> Container<T> {
    # Associated function with type inference
    fn new(value: T) -> Self {
        Container { value: value }
    }
    
    # Method with generic return type
    fn get_value(self: Self) -> T {
        self.value
    }
    
    # Method with additional type parameters
    fn transform<U>(self: Self, f: fn(T) -> U) -> Container<U> {
        Container { value: f(self.value) }
    }
}

fn main() -> u64 {
    # Type inference: T = u64
    val result1 = identity(42u64)      # Returns UInt64(42)
    val result2 = identity("hello")    # Returns String("hello")
    
    # Multiple type inference: T = u64, U = bool  
    val swapped = swap((42u64, true))  # Returns (true, 42u64)
    
    # Generic structure usage with type inference
    val container = Container::new(123u64)  # T = u64
    val value = container.get_value()       # Returns 123u64
    
    # Mixed types: Container<u64> and Container<bool>
    val int_container = Container { value: 42u64 }
    val bool_container = Container { value: true }
    
    result1
}
```

### Top-level Constants
```rust
# `const` declarations sit at file scope and are evaluated once at startup.
# The type annotation is mandatory; initializers may reference earlier
# consts but not later ones (no forward references).
const PI: f64 = 3.14159f64
const TWO_PI: f64 = PI + PI
const MAX_RETRIES: u64 = 3u64

fn area(r: f64) -> f64 { PI * r * r }
```

### Termination: panic and assert
```rust
# `panic("msg")` aborts the run with `panic: <msg>` on stderr and exit 1.
# `assert(cond, "msg")` is sugar for `if !cond { panic(msg) }` and runs
# the message lazily — only when the condition fails.
fn divide(a: i64, b: i64) -> i64 {
    assert(b != 0i64, "divide: divisor must be non-zero")
    a / b
}

fn unreachable_path() -> i64 {
    panic("not implemented")
}
```

`panic` is also typed as `Unknown`, so it can sit in the diverging branch
of an `if`-expression without forcing the whole expression to `Unit`:

```rust
fn safe_divide(a: i64, b: i64) -> i64 {
    if b == 0i64 { panic("division by zero") } else { a / b }
}
```

### Design by Contract
```rust
# `requires` runs at function entry; `ensures` runs at exit with `result`
# bound to the return value. Multiple clauses of either kind are AND-composed,
# and a violation aborts the call with a clause-specific error message.
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    ensures  result * b == a
{
    a / b
}

# Methods can use `self` in both clauses.
impl Counter {
    fn inc(self: Self) -> Self
        requires self.n >= 0i64
        ensures  result.n == self.n + 1i64
    {
        Counter { n: self.n + 1i64 }
    }
}
```

Contract evaluation can be tuned at runtime through the
`INTERPRETER_CONTRACTS` environment variable (the equivalent of D's
`-release` flag):

| Value (case-insensitive) | `requires` | `ensures` |
|---|---|---|
| `all` (default; also unset) | evaluated | evaluated |
| `pre` | evaluated | skipped |
| `post` | skipped | evaluated |
| `off` | skipped | skipped |

Unrecognised values print a warning to stderr and fall back to `all`
so a typo can't silently disable contracts. The mode is read once at
startup and cached on the evaluation context.

**Recommended setting: `all` (the default).** Disabling contracts in
release is the very pattern D's `-release` flag is criticised for —
invariants that fired in development go silent in production. Reach
for `pre` / `post` / `off` only for hot-path benchmarks where a
clause has measurable cost. The `panic` and `assert` builtins
intentionally have no analogous gate; both are always active so
safety checks behave identically across build profiles.

### Memory Profiling

```bash
# Print the run's allocation totals to stderr (text)
cargo run -q -p interpreter -- --profile=mem interpreter/example/memory_contract.t

# Machine-readable form — `leaks` is always present, `[]` when nothing leaked
cargo run -q -p interpreter -- --profile=mem --profile-format=json interpreter/example/memory_contract.t

# AOT-compiled binaries profile themselves, no interpreter involved
TOY_PROFILE_MEM=1 ./fib

# All backends must report the same numbers — verify with one command
cargo run -q -p compiler -- interpreter/example/allocator_list.t --all-backends --profile=mem
```

Example report:

```
memory profile
  alloc_count       2
  free_count        2
  realloc_count     0
  cumulative_bytes  96
  live_bytes        0
  peak_live_bytes   64
  peak_at_request   1
```

When allocations are never freed, a `leaks` section follows, each
line attributing the leak to the allocation *site* (`line:column`):

```
leaks (1 sites, 1 allocations, 32 bytes)
  2:18  1 allocations  32 bytes
```

What each field means and how allocator layout reports work are in
[`design-docs/MEMORY_PROFILING.md`](design-docs/MEMORY_PROFILING.md).
All counters are **request-based** — they describe what the program
asked for, not what the allocator did — which is why every backend
(tree-walker, IR VM, JIT, AOT) reports byte-identical numbers.

**Memory as a contract.** The same counters are readable from
`requires` / `ensures` clauses and `test` blocks, so allocation
becomes a property a function can promise:

```rust
fn scratch(size: u64) -> u64
    requires size >= 8u64
    requires size <= 4096u64
    ensures __builtin_live_bytes() == 0u64
{ ... }

test "scratch work leaves nothing behind" {
    val before: u64 = __builtin_live_bytes()
    val r: u64 = scratch(128u64)
    assert_eq(__builtin_live_bytes(), before)
}
```

Reading a counter does **not** require the profiling flag: a program
that uses `__builtin_live_bytes()` gets real numbers with or without
`--profile=mem` (counting is enabled for that run; the report is a
separate, opt-in output). `--check` treats a memory clause like any
other contract. See *Allocation counters* in
[`docs/language.md`](docs/language.md) for the six counters and their
exact semantics; `interpreter/example/memory_contract.t` runs
`--test` and `--check` cleanly.

### Module System
```rust
# math.t (in modules/math/math.t)
pub fn add(a: u64, b: u64) -> u64 {
    a + b
}

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b
}

# main.t
import math

fn main() -> u64 {
    math::add(10u64, 20u64)  # Returns 30
}
```

## Development Features

### Comprehensive Testing
- **Extensive Test Coverage**: All language features tested with edge cases including destruction system
- **Three-Phase Testing**: Tests run in phases (lib.rs, main.rs, doc-tests) with clear progress indication
- **Property-based Testing**: Automated testing of language invariants
- **Performance Benchmarks**: Detailed performance analysis with Criterion
- **Resource Management Tests**: Validation of automatic destruction and custom `drop` methods

### Performance Optimizations
- **Type Inference Caching**: Efficient memoization of type resolution
- **Memory Pool Design**: Reduced allocation overhead for AST nodes
- **Structured Error System**: Fast error categorization and reporting
- **Conditional Debug Logging**: Zero-overhead resource tracking in production builds

### Development Tools
- **Rich Example Suite**: Multiple example programs demonstrating language features
- **Detailed Error Messages**: Structured error reporting with context information
- **Performance Profiling**: Built-in benchmarks for interpreter performance

## Project Status

The language runs real programs. [`poc/logsearch`](poc/logsearch) is ~10k
lines of it: a log archiver with its own on-disk format, a compressor, an
inverted index, a catalog, and an HTTP server holding 128 connections —
written entirely in the language, and the source of most of the gaps that
have since been closed (a container could not hold socket handles; a
`match` arm copied its payload; reading an element freed it).

What is missing is listed, not hidden:
[`design-docs/todo.md`](design-docs/todo.md) has the unimplemented section
and the known defects, each with what it costs and what it would take. The
largest open item is real parallel execution — the semantics of
`parallel for` are in, the threads are not
([`design-docs/CONCURRENCY.md`](design-docs/CONCURRENCY.md)).

## Technical Highlights

- **Zero-cost Type Checking**: Type validation occurs before execution
- **Generic Type System**: Generic functions / structures / impls with constraint-based inference and `<A: Allocator>` bounds
- **Allocator system**: `with allocator = expr { … }` lexically-scoped allocator binding, ambient sugar, Arena / FixedBuffer / Global allocators (see [`design-docs/ALLOCATOR_PLAN.md`](design-docs/ALLOCATOR_PLAN.md))
- **Cranelift JIT** (default-on cargo feature, `INTERPRETER_JIT=1` to opt in at runtime): native-code compilation for numeric / bool / struct / tuple / `f64` subsets, with `panic("literal")` and `assert(cond, "literal")` lowered through a host helper + `trap` (see [`design-docs/JIT.md`](design-docs/JIT.md))
- **Multi-backend Architecture**: Tree-walker (reference oracle) + AOT compiler (IR → cranelift → object file) + Cranelift JIT (AST direct) + IR VM (shared IR flat-slot interpreter). 4-way consistency is continuously validated via `compiler/tests/consistency.rs` (see [`design-docs/BACKEND.md`](design-docs/BACKEND.md) for the full backend technical specification)
- **Design by Contract**: `requires` / `ensures` clauses with `result` binding and an `INTERPRETER_CONTRACTS=all|pre|post|off` runtime gate (D `-release` equivalent)
- **Memory profiling**: request-based allocation counters, leak detection (per allocation site), and allocator layout reports — `--profile=mem` / `--profile-format=json` on the interpreter and `TOY_PROFILE_MEM=1` on AOT binaries, byte-identical across all four backends. The same counters are readable from `requires` / `ensures` / `test`, so memory use can be pinned by contract (see [`design-docs/MEMORY_PROFILING.md`](design-docs/MEMORY_PROFILING.md))
- **Efficient Memory Management**: Append-only `StmtPool` / `ExprPool` plus automatic destruction with custom `drop` methods
- **Testing**: ~3,000 tests in the workspace, plus the POC's own 147. The
  interesting ones are the consistency tests, which run a program on all
  four engines and compare — and pin the *output*, because four engines
  agreeing on a wrong answer is the failure mode that costs the most
- **Debug-mode Logging**: Conditional compilation for zero-overhead production builds
- **Diagnostics built for a reader** (and for a machine): every error has a
  code, a span, and `--explain` prose with a reproduction and a fix;
  `--diagnostics=json` puts the same thing on stderr as data. A diagnostic
  from an imported module names *that* file and line
- **Ownership without a borrow checker**: one owner per resource, checked
  transfers, drop glue through containers, and `borrow` for reading an
  element. No lifetimes — an escape rule instead, shared with the region
  check for scoped allocators

All major language features are implemented and thoroughly tested. The
canonical language reference is [`docs/language.md`](docs/language.md);
this README is a high-level tour.
