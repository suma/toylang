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
| `compiler` | AOT: IR → object file → executable through **Cranelift** (debug builds) or **LLVM** (`--release`, `-O2`), and a Cranelift JIT over the same IR |
| `compiler/runtime/toylang_rt` | The runtime both compiled lanes link: output, allocator, profiler, backtraces |
| `toy` | The build tool — `toy build` / `run` / `test` / `check` for a program with its own modules |

A larger program written **in** the language lives in
[`poc/logsearch`](poc/logsearch) — a log archiver and search server, which
is where most of the gaps in the language were found.

## Language Features

### Core Language Constructs
- **Functions** with explicit return types: `fn fibonacci(n: u64) -> u64`
- **Variables**: Immutable (`val`) and mutable (`var`) declarations
- **Top-level constants**: `const PI: f64 = 3.14159f64` evaluated once at startup;
  the annotation types the initializer, so `const N: u64 = 3` and
  `const SEP: u8 = ','` need no suffix
- **Control Flow**: `if/else/elif`, `for` loops with `break/continue`, `while` loops,
  and `loop`, which **is a value** when its `break`s carry one
  (`val k = loop { .. break i }`, `break @outer v` from an inner loop)
- **Types**: `u64` / `i64` / `f64` / `f32` / `bool` / `str` / `ptr` / `usize`,
  narrow integers (`u8`–`u32`, `i8`–`i32`), `char`, 128-bit SIMD vectors
  (`f64x2` / `f32x4` / `i32x4` / `i64x2` / `u8x16`), tuples, fixed arrays,
  `dict`

### Advanced Features
- **Fixed Arrays**: `val arr: [i64; 5] = [1, 2, 3, 4, 5]` with type
  inference, and `[0u8; 64]` to repeat a literal
- **Tuples**: `val (a, b) = (1u64, 2u64)` with destructuring (including nested patterns)
- **Dictionary Type**: `dict{key1: value1, key2: value2}` with Object-keyable types
- **Structures**: `struct Point { x: i64, y: i64 }` with method implementations,
  the field shorthand `Point { x, y }`, and destructuring
  `val Point { x, y: b, .. } = p`
- **String literals**: `\"` for a quote, `{expr}` interpolation (`{{` for a
  brace), and raw literals `r"..."` / `r#"..."#` with no escapes and no
  interpolation — JSON and HTML are written as themselves. Any literal may
  span lines
- **Enums and Pattern Matching**: `enum Shape { Circle(i64), Rect(i64, i64), Point }`
  with tuple-variant binding, **struct variants** (`Syslog { host: u64, tag: u64 }`),
  **discriminants** on unit variants with `as` to an integer (`Apache = 10`,
  `k as u32`), literal / range / or / `@` / nested patterns, per-arm `if`
  guards, a `match` over **any integer width** with char literals narrowed to
  it (`match b: u8 { '0'..':' => .. }`), **named constants** as patterns, and
  string-literal arms on a `String` as well as a `str`
- **Generics with bounds**: `fn id<T>(x: T) -> T` and `fn run<A: Allocator>(a: A)`
- **Design by Contract**: `requires` (preconditions) and `ensures` (postconditions) on functions and methods, with `result` for the return value. Runtime gating via `INTERPRETER_CONTRACTS=all|pre|post|off`
- **Termination primitives**: `panic("msg")` and `assert(cond, "msg")` for explicit failure
- **Allocator system**: `with allocator = arena { … }` lexically scoped allocator binding, `<A: Allocator>` bound, arena / fixed-buffer / global allocator builtins
- **Built-in Methods**: String operations like `"hello".len()` returning `u64`
- **Unary Operators**: `-x` (signed int / `f64`), `!` (logical not), `~` (bitwise not)
- **Ownership**: a type with an `impl Drop` has one owner. Handing it to
  something that outlives the scope **transfers** it (`[E0014]`), drop glue
  frees containers recursively, and a container **lends** an element with
  `borrow` rather than handing out a second owner (`[E0028]`). An alias
  (`val b = a`, a value taken out of a `match`) hands over its owner's value,
  and a by-value argument to a function that only reads it stays the
  caller's to drop — so each value is freed exactly once
- **Effects**: what a declaration can do besides compute — `never_allocates`
  and `const fn` are checked against the same reachability walk
- **Data parallelism**: `parallel for i in 0u64..n { .. }` — the iterations
  may run in any order; the compiled engines split the range across threads,
  the interpreters run it in order, and the answer is the same
- **Tasks**: `val t: Task<T> = spawn { body }` runs the body on a thread of
  its own (compiled engines) and hands back its value with `t.join()`; owned
  values the body captures move into it. `t.as_fd()` is a descriptor a
  `Poller` can wait on beside sockets, so an event loop can hand off a slow
  write and keep serving
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
- **Default engine is the IR VM**: `interpreter` lowers the checked AST
  through `compiler_lower`, the same lowering the AOT and JIT lanes use,
  and runs the IR
- **Tree-walker as the oracle**: direct AST traversal with
  `Rc<RefCell<Object>>` values, independent of the lowering — the lane
  the consistency tests compare the others against
- **Module Integration**: imported modules are merged into one program,
  with their side tables and file ids, so diagnostics name the module's
  own file and line
- **Driver**: `--test`, `--check` (contracts as property tests),
  `--explain`, `--api`, `--effects`, `--profile=mem`, `--heap-check`
- **Queries** (`query.rs`): the type at a position, definitions,
  references and call edges, answered from the checker's own result —
  what `toy query` runs

Where each concern is implemented is mapped in
[`design-docs/CODE_MAP.md`](design-docs/CODE_MAP.md).

## Getting Started

### Prerequisites
- Rust 1.85+ (the workspace uses edition 2024)
- Cargo package manager
- `cargo-nextest` for the test suite (`cargo install cargo-nextest`)
- **LLVM 22**, only for optimised (`--release`) builds — see
  [The LLVM backend](#the-llvm-backend-release-builds) below. Everything
  else builds and runs without it

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

### The LLVM backend (release builds)

The AOT compiler has two backends behind the same lowering, runtime and
link step: **Cranelift**, which compiles fast and is what a debug build
uses, and **LLVM 22**, which `--release` uses with the `-O2` pipeline
([`design-docs/AOT_LLVM.md`](design-docs/AOT_LLVM.md)). LLVM is the
optional `llvm` cargo feature, off by default, so a plain build needs no
LLVM on the machine.

```bash
# 1. Install LLVM 22 (Homebrew keeps it out of the PATH: it is keg-only)
brew install llvm@22                      # macOS
# sudo apt install llvm-22-dev            # Debian / Ubuntu (apt.llvm.org)

# 2. Tell llvm-sys where it is, and build with the feature
export LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22   # or /usr/lib/llvm-22
cargo build --release -p toy -p compiler --features llvm
```

`LLVM_SYS_221_PREFIX` is read by the `llvm-sys` build script (the `221`
is LLVM 22.1). Without it, `llvm-sys` looks for an `llvm-config` of
version 22 on the `PATH`. It is deliberately not set in
`.cargo/config.toml`: the path differs from machine to machine.

Choosing the backend:

| Flags | Backend |
|---|---|
| *(none)* | Cranelift |
| `--release` | LLVM, `-O2`, for the host CPU (contracts are compiled out, as always with `--release`) |
| `--codegen=cranelift` / `--codegen=llvm` | the one named, with or without `--release` |

A `compiler` or `toy` built **without** the feature refuses a bare
`--release` rather than quietly building something else, and says how to
proceed: rebuild with `--features llvm`, or ask for
`--release --codegen=cranelift`, which works anywhere.

```bash
target/release/compiler interpreter/example/fib.t --release -o /tmp/fib   # LLVM -O2
target/release/toy build poc/logsearch --release                         # LLVM -O2
target/release/toy build poc/logsearch --release --codegen=cranelift     # no LLVM needed
target/release/toy test  poc/logsearch --codegen=llvm                    # LLVM -O0
```

Release against release on Apple Silicon, LLVM over Cranelift: `fib(40)`
2.1×, a full-scan query in `poc/logsearch` 1.9×, its `verify` 1.65×, and a
19% smaller binary; the release compile of the POC goes from 0.08 s to
1.95 s.

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

# Print allocation totals after the run (JSON with --format=json)
cargo run -q -p interpreter -- --profile=mem interpreter/example/allocator_list.t

# Stop at a use after free, an access past a block's end or a double
# free; `reuse` also hands freed blocks out again (docs/language.md,
# "Heap checks")
cargo run -q -p interpreter -- --heap-check=poison interpreter/example/allocator_list.t
cargo run -q -p compiler -- interpreter/example/allocator_list.t --heap-check=poison -o /tmp/al && /tmp/al
```

### A program with its own modules (`toy`)

```bash
cargo run -q -p toy -- new   mypkg                  # a package that runs, tests and checks as is
cargo run -q -p toy -- build poc/logsearch [--release]   # --release needs LLVM (above)
cargo run -q -p toy -- run   poc/logsearch -- serve /var/log/archive 8080
cargo run -q -p toy -- test  poc/logsearch          # AOT by default, in parallel
cargo run -q -p toy -- check poc/logsearch --format=json
cargo run -q -p toy -- check poc/logsearch --format=short   # one diagnostic per line
cargo run -q -p toy -- fix   poc/logsearch [--dry-run]      # apply machine-applicable fixes, re-check
```

Every subcommand takes `--format=text|json`; `json` puts the result on
stdout and the errors on stderr as JSON (`run` keeps the program's own
output and reshapes only the errors); see
[`design-docs/BUILD_TOOL.md`](design-docs/BUILD_TOOL.md).

`toy query` answers questions about a checked program without running it —
positions are `FILE:LINE:COL`, and several can be asked in one call:

```bash
cargo run -q -p toy -- query type    main.t:12:9 --in mypkg   # the type there
cargo run -q -p toy -- query def     main.t:9:20 --in mypkg   # where a name is defined
cargo run -q -p toy -- query refs    main.t:5:4  --in mypkg   # where it is used
cargo run -q -p toy -- query callers twice      --in mypkg   # / callees
```

For a loop that checks the same package over and over, build `toy` once
with `cargo build --release -p toy` and call `target/release/toy`
directly; `cargo run` pays a freshness check and a debug build each time.

### Using it from an LLM agent (Claude Code)

The diagnostics and `toy` are built to be read by a program as well as a
person: every error carries a code, a span, related locations and, where
the fix is certain, the edit itself (`--format=json`); `toy explain CODE`
gives the cause and the fix in prose. This repository also wires them
into Claude Code:

- [`.claude/skills/toylang/SKILL.md`](.claude/skills/toylang/SKILL.md) — a
  skill with the grammar traps (`elif`, float suffixes, no semicolons,
  ownership) and the check → fix → query loop
- [`.claude/settings.json`](.claude/settings.json) — a `PostToolUse` hook
  that runs `target/release/toy hook` after every edit to a `.t` file and
  hands the errors back one per line, and permission for `toy`'s
  read-only subcommands

The design is in [`design-docs/LLM_TOOLING.md`](design-docs/LLM_TOOLING.md)
and [`design-docs/CLAUDE_CODE_INTEGRATION.md`](design-docs/CLAUDE_CODE_INTEGRATION.md).

For the full CLI / env-var reference see [`interpreter/README.md`](interpreter/README.md).

### Environment variables

| Variable | Read by | What it does |
|---|---|---|
| `LLVM_SYS_221_PREFIX` | build (`llvm-sys`) | Where LLVM 22 is installed, for `--features llvm` |
| `TOYLANG_CORE_MODULES` | `compiler` / `interpreter` | The stdlib root when it is not found next to the binary |
| `TOY_PAR_THREADS` | compiled programs | Threads a `parallel for` uses (default: the core count, at most 64). Never changes the answer |
| `TOY_PROFILE_MEM` | compiled programs | `1` prints allocation totals and leaks at exit, `json` as JSON |
| `TOY_HEAP_CHECK` / `TOY_HEAP_QUARANTINE` | compiled programs | `report` / `reuse` heap checking at run time, and the quarantine size |
| `TOY_LINK_CACHE_DIR` | `compiler` | A content-addressed cache of linked executables |
| `TOYLANG_CRANELIFT_OPT_LEVEL` | `compiler` | `none` / `speed` (default) / `speed_and_size` for the Cranelift backend; the test suite uses `none` |
| `TOYLANG_CRANELIFT_VERIFY` | `compiler` | `1` / `0` forces Cranelift's IR verifier on or off (default: on in a debug build of the compiler) |
| `TOYLANG_CODEGEN_THREADS` | `compiler` | Fixes the number of Cranelift codegen threads |
| `INTERPRETER_JIT` | `interpreter` | `1` turns on the AST-level JIT |
| `INTERPRETER_CONTRACTS` | `interpreter` | `all` (default) / `pre` / `post` / `off` |
| `TOY_BLESS` | `toy test` | Set by `--bless`: record golden files instead of checking them |

The test-only ones (`PROPTEST_CASES`, `TOYLANG_CRANELIFT_OPT_LEVEL=none`,
`TOY_LINK_CACHE_DIR`) are already set in `.cargo/config.toml`.

### Testing

`cargo nextest` is what the suite is tuned for — a green run prints six
lines, because the output is what makes a test run readable, not the speed.

```bash
# Everything (~3,300 tests)
cargo nextest run

# One crate, or a name (filters are substring matches, not exact)
cargo nextest run -p compiler
cargo nextest run -E 'test(basic_arithmetic)'

# The LLVM backend: with the feature, the consistency and example suites
# also build every program with LLVM and hold it to the Cranelift binary
LLVM_SYS_221_PREFIX=/opt/homebrew/opt/llvm@22 cargo nextest run -p compiler -p toy --features llvm

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

### Strings
```rust
fn strings_example() -> u64 {
    val n = 42u64
    val json = "{{\"n\":{n}}}"               # {"n":42} -- `\"` quotes, `{n}` interpolates
    val raw = r#"{"error":"not found"}"#     # raw: no escapes, no interpolation
    val page = r#"<p class="note">
  two lines
</p>"#                                        # any literal may span lines
    json.len() + raw.len() + page.len()
}
```

A raw literal closes only at `"` followed by as many `#`s as it opened
with, so `r##"..."##` holds a `"#`. Interpolation is for ordinary literals
only, which is when `{{` / `}}` stand for a brace.

### Structures and Methods
```rust
struct Point {
    x: i64,
    y: i64
}

impl Point {
    fn new(x: i64, y: i64) -> Point {
        Point { x, y }                  # shorthand for `Point { x: x, y: y }`
    }

    fn distance(&self) -> i64 {
        self.x * self.x + self.y * self.y
    }
}

fn swap(p: Point) -> Point {
    val Point { x, y } = p              # destructure; `..` would skip fields
    Point { x: y, y: x }
}
```

A destructuring is checked like a `match` arm: the struct's name must be
the value's, and every field must be named unless the pattern ends in `..`.

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

```rust
# A unit variant may name the number it stands for; `as` yields it
enum Kind { Plain, Syslog, Apache = 10, Epoch }     # 0, 1, 10, 11
fn code(k: Kind) -> u32 { k as u32 }

# A variant may name its fields
enum Rec {
    Syslog { host: u64, tag: u64 },
    Plain,
}
fn host_of(r: Rec) -> u64 {
    match r {
        Rec::Syslog { host, .. } => host,    # `..` skips the rest
        Rec::Plain => 0u64,
    }
}

const SPACE: u8 = 32u8
fn classify(b: u8) -> u64 {
    match b {
        SPACE => 0u64,        # a const name compares against its value
        '0'..':' => 1u64,     # a char literal narrows to the `u8` scrutinee
        _ => 2u64,
    }
}
```

A `String` matches string literals the way a `str` does, without
allocating for the comparison:

```rust
fn verb(method: &String) -> u64 {
    match method {
        "GET" => 1u64,
        "PUT" | "POST" => 2u64,
        _ => 0u64,
    }
}
```

`as` needs every variant to be a unit variant and every number to fit the
target, and a struct variant is a tuple variant whose positions have names
(`Rec::Syslog(3u64, 7u64)` works too). A `u8` covered by ranges needs no
`_`: exhaustiveness counts values.

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

Two more rules keep each value to one drop. A name that **aliases** another
binding's value hands that binding's value over when it is handed over:

```rust
val taken = listener.accept()
var conn = match taken {                 # `conn` aliases `taken`'s payload
    Result::Ok(c) => c,
    Result::Err(e) => { panic("accept: {e}") }
}
serve(conn)                              # `taken` stops owning it too
```

And a by-value argument to a function that only **reads** the parameter
(fields, `&self` methods, printing, passing it on as `&T`) is **lent**: the
callee keeps nothing, so the caller keeps the drop. Reading the binding
after the call is still `[E0014]` — as far as the language goes, it moved.

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

### Loops as Values
```rust
fn first_square_over(n: u64) -> u64 {
    var i = 0u64
    loop {                              # the loop's value is the function's
        i = i + 1u64
        if i * i > n { break i }
    }
}

fn find(grid: u64) -> u64 {
    var a = 1u64
    val hit = @outer: loop {
        if a > grid { break @outer 0u64 }
        var b = 1u64
        while b <= grid {
            if a * b == 12u64 { break @outer a * 10u64 + b }  # out of the `while`
            b = b + 1u64
        }
        a = a + 1u64
    }
    hit
}
```

Only a `loop` has a value (a `while` or `for` can end without a `break`),
and once one `break` out of it carries a value, every one must. The value
is the expression on the `break`'s own line.

### Top-level Constants
```rust
# `const` declarations sit at file scope and are evaluated once at startup.
# The type annotation is mandatory and types the initializer; initializers
# may reference earlier consts but not later ones (no forward references).
const PI: f64 = 3.14159f64
const TWO_PI: f64 = PI + PI
const MAX_RETRIES: u64 = 3
const SEP: u8 = ','

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
cargo run -q -p interpreter -- --profile=mem --format=json interpreter/example/memory_contract.t

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
- **One test binary per crate**: `tests/suite.rs` gathers each crate's test files, run with `cargo nextest`; doc-tests separately with `cargo test --doc`
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

It also uses the language's concurrency: the server hands a full segment
to a `spawn`ed task and keeps ingesting, waiting for the task's descriptor
in the same poller as its sockets — the worst `/v1/stats` latency during a
heavy ingest went from 24 ms to 2.5 ms
([`design-docs/CONCURRENCY.md`](design-docs/CONCURRENCY.md)).

What is missing is listed, not hidden:
[`design-docs/todo.md`](design-docs/todo.md) has the unimplemented section
and the known defects, each with what it costs and what it would take. The
largest open items are trait inheritance and associated types, and the
compiled engines' refusal of a compound-returning call outside a `val`.

## Technical Highlights

- **Zero-cost Type Checking**: Type validation occurs before execution
- **Generic Type System**: Generic functions / structures / impls with constraint-based inference and `<A: Allocator>` bounds
- **Allocator system**: `with allocator = expr { … }` lexically-scoped allocator binding, ambient sugar, Arena / FixedBuffer / Global allocators (see [`design-docs/ALLOCATOR_PLAN.md`](design-docs/ALLOCATOR_PLAN.md))
- **Cranelift JIT** (default-on cargo feature, `INTERPRETER_JIT=1` to opt in at runtime): native-code compilation for numeric / bool / struct / tuple / `f64` subsets, with `panic("literal")` and `assert(cond, "literal")` lowered through a host helper + `trap` (see [`design-docs/JIT.md`](design-docs/JIT.md))
- **Multi-backend Architecture**: Tree-walker (reference oracle) + AOT compiler (IR → Cranelift or LLVM → object file) + Cranelift JIT (AST direct) + IR VM (shared IR flat-slot interpreter). 4-way consistency is continuously validated via `compiler/tests/consistency/` (see [`design-docs/BACKEND.md`](design-docs/BACKEND.md) for the full backend technical specification)
- **Design by Contract**: `requires` / `ensures` clauses with `result` binding and an `INTERPRETER_CONTRACTS=all|pre|post|off` runtime gate (D `-release` equivalent)
- **Memory profiling**: request-based allocation counters, leak detection (per allocation site), and allocator layout reports — `--profile=mem` / `--format=json` on the interpreter and `TOY_PROFILE_MEM=1` on AOT binaries, byte-identical across all four backends. The same counters are readable from `requires` / `ensures` / `test`, so memory use can be pinned by contract (see [`design-docs/MEMORY_PROFILING.md`](design-docs/MEMORY_PROFILING.md))
- **Efficient Memory Management**: Append-only `StmtPool` / `ExprPool` plus automatic destruction with custom `drop` methods
- **Testing**: ~3,300 tests in the workspace, plus the POC's own 152. The
  interesting ones are the consistency tests, which run a program on all
  four engines and compare — and pin the *output*, because four engines
  agreeing on a wrong answer is the failure mode that costs the most
- **Debug-mode Logging**: Conditional compilation for zero-overhead production builds
- **Diagnostics built for a reader** (and for a machine): every error has a
  code, a span, and `--explain` prose with a reproduction and a fix;
  `--format=json` puts the same thing on stderr as data. A diagnostic
  from an imported module names *that* file and line
- **Ownership without a borrow checker**: one owner per resource, checked
  transfers, drop glue through containers, and `borrow` for reading an
  element. No lifetimes — an escape rule instead, shared with the region
  check for scoped allocators

All major language features are implemented and thoroughly tested. The
canonical language reference is [`docs/language.md`](docs/language.md);
this README is a high-level tour.
