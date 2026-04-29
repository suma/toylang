# Programming Language Implementation in Rust

A complete programming language implementation featuring a frontend library and tree-walking interpreter, built as a learning project to explore language design and implementation techniques.

## Overview

This project implements a statically-typed programming language with comprehensive type checking, automatic type inference, and modern language features. The implementation is split into three main components:

- **Frontend Library**: Shared parser, AST, and type checker with automatic lexer generation
- **Interpreter**: Tree-walking interpreter with comprehensive test suite and performance optimizations

## Language Features

### Core Language Constructs
- **Functions** with explicit return types: `fn fibonacci(n: u64) -> u64`
- **Variables**: Immutable (`val`) and mutable (`var`) declarations
- **Control Flow**: `if/else/elif`, `for` loops with `break/continue`, `while` loops
- **Types**: `u64`, `i64`, `str`, `bool`, `Dict`

### Advanced Features
- **Fixed Arrays**: `val arr: [i64; 5] = [1, 2, 3, 4, 5]` with type inference
- **Dictionary Type**: `val dict: Dict = {key1: value1, key2: value2}` with Object key support
- **Structures**: `struct Point { x: i64, y: i64 }` with method implementations
- **Enums and Pattern Matching**: `enum Shape { Circle(i64), Rect(i64, i64), Point }` with `match` expressions that support tuple variants, binding patterns, and wildcards
- **Generics**: Type-safe generic functions and structures with automatic type inference
- **Design by Contract**: `requires` (preconditions) and `ensures` (postconditions) clauses on functions and methods, with `result` referring to the return value inside `ensures`
- **Built-in Methods**: String operations like `"hello".len()` returning `u64`
- **Unary Operators**: `-x` for signed integer negation, `!` / `~` for logical and bitwise not
- **Resource Management**: Automatic destruction system with custom `__drop__` methods
- **Comments**: Line comments with `#` symbol support
- **No Semicolons**: Statements are separated by newlines, not semicolons
- **Module System**: Go-style modules with `package`/`import` declarations
- **Qualified Identifiers**: Rust-style `module::function` syntax for module access

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

```bash
# Build frontend library
cd frontend && cargo build

# Build interpreter
cd interpreter && cargo build

# Build with debug logging enabled (for development)
cd interpreter && cargo build --features debug-logging

# Build release version (production-optimized, no logging overhead)
cd interpreter && cargo build --release
```

### Running Programs

```bash
# Execute a program file with interpreter
cd interpreter && cargo run example/fib.t

# Available example programs
cargo run example/fibonacci_array.t    # Array-based fibonacci
cargo run example/string_len_test.t    # String operations
cargo run example/array_test.t         # Array manipulation
cargo run example/test_qualified_identifier.t  # Module system with qualified identifiers
```

### Testing

```bash
# Run all tests with comprehensive coverage (3 phases: lib.rs, main.rs, doc-tests)
cd interpreter && cargo test

# Run frontend library tests  
cd frontend && cargo test

# Run property-based tests
cd interpreter && cargo test proptest

# Run destruction system tests specifically
cd interpreter && cargo test destruction_tests custom_destructor_tests

# Run tests with logging enabled (useful for debugging)
cd interpreter && cargo test --features test-logging
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

Every `match` arm must produce the same result type. Patterns supported today:
`Enum::Variant`, `Enum::Variant(x, _, y)` (with binding / discard slots), and
`_` catch-all. Exhaustiveness checking, generic enums, and nested structural
patterns are on the roadmap.

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

### Resource Management with Custom Destructors
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
    fn __drop__(self: Self) {
        # Close file handle, release resources
        # Log cleanup actions, etc.
    }
}

fn main() -> u64 {
    val file = FileResource::open("data.txt")
    val content = file.read_data()
    # FileResource.__drop__() automatically called when 'file' goes out of scope
    0u64
}
```

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
- **Resource Management Tests**: Validation of automatic destruction and custom `__drop__` methods

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

This is a fully functional programming language implementation suitable for:
- Educational purposes and language design study
- Experimenting with type system design
- Understanding interpreter implementation techniques
- Exploring modern language features in a controlled environment
The implementation includes comprehensive documentation, extensive testing, and performance optimizations, making it a robust foundation for further language development.

## Technical Highlights

- **Zero-cost Type Checking**: Type validation occurs before execution
- **Generic Type System**: Full support for generic functions and structures with constraint-based type inference
- **Unification Algorithm**: Sophisticated type parameter resolution from usage context
- **Efficient Memory Management**: Minimal allocation overhead with pool-based design and automatic destruction
- **Extensible Architecture**: Clean separation between frontend and backend components
- **Production-quality Testing**: Comprehensive test suite with full pass rate
- **Resource Management**: Automatic object destruction with custom `__drop__` method support
- **Debug-mode Logging**: Conditional compilation for zero-overhead production builds

All major language features including a complete generics system are implemented and thoroughly tested, providing a solid foundation for both learning and practical use.
