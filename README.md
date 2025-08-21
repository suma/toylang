# Programming Language Implementation in Rust

A complete programming language implementation featuring a frontend library and tree-walking interpreter, built as a learning project to explore language design and implementation techniques.

## Overview

This project implements a statically-typed programming language with comprehensive type checking, automatic type inference, and modern language features. The implementation is split into two main components:

- **Frontend Library**: Shared parser, AST, and type checker with automatic lexer generation
- **Interpreter**: Tree-walking interpreter with comprehensive test suite and performance optimizations

## Language Features

### Core Language Constructs
- **Functions** with explicit return types: `fn fibonacci(n: u64) -> u64`
- **Variables**: Immutable (`val`) and mutable (`var`) declarations
- **Control Flow**: `if/else/elif`, `for` loops with `break/continue`, `while` loops
- **Types**: `u64`, `i64`, `str`, `bool` with automatic conversion support

### Advanced Features
- **Fixed Arrays**: `val arr: [i64; 5] = [1, 2, 3, 4, 5]` with type inference
- **Structures**: `struct Point { x: i64, y: i64 }` with method implementations
- **Built-in Methods**: String operations like `"hello".len()` returning `u64`
- **Resource Management**: Automatic destruction system with custom `__drop__` methods
- **Comments**: Line comments with `#` symbol support
- **Module System**: Go-style modules with `package`/`import` declarations
- **Qualified Identifiers**: Rust-style `module::function` syntax for module access

### Type System
- **Context-based Type Inference**: Automatic type resolution based on usage context
- **Automatic Type Conversion**: Seamless conversion between compatible numeric types  
- **Advanced Type Checking**: Comprehensive validation with detailed error reporting
- **Memory Pool Architecture**: Efficient AST storage with `StmtPool` and `ExprPool`

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
# Execute a program file
cd interpreter && cargo run example/fib.t

# Available example programs
cargo run example/fibonacci_array.t    # Array-based fibonacci
cargo run example/string_len_test.t    # String operations
cargo run example/array_test.t         # Array manipulation
cargo run example/test_qualified_identifier.t  # Module system with qualified identifiers
```

### Testing

```bash
# Run all tests with comprehensive coverage
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

### Variables and Arrays
```rust
fn array_example() -> i64 {
    val numbers: [i64; 3] = [10, 20, 30]
    var sum = 0i64
    
    for i in 0u64 to 3u64 {
        sum = sum + numbers[i]
    }
    
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
- **Efficient Memory Management**: Minimal allocation overhead with pool-based design and automatic destruction
- **Extensible Architecture**: Clean separation between frontend and backend components
- **Production-quality Testing**: Comprehensive test suite with full pass rate
- **Resource Management**: Automatic object destruction with custom `__drop__` method support
- **Debug-mode Logging**: Conditional compilation for zero-overhead production builds

All major language features are implemented and thoroughly tested, providing a solid foundation for both learning and practical use.