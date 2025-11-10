# toylang Compiler Test Plan Design Document

## 1. Overview

### 1.1 Project Goal
toylang is a toy programming language implemented in Rust. It supports functions, variables, control flow, and advanced type checking, consisting of two main components: frontend (parser and type checker) and interpreter.

### 1.2 Overall Test Strategy
- **Quality Assurance**: Ensure correctness of each layer (parser, type checker, interpreter)
- **Regression Prevention**: Continuously verify existing functionality
- **Early Detection**: Identify compilation errors at development stage
- **Practical Verification**: Confirm that actual programs execute correctly

---

## 2. Test Layer Design

### 2.1 Unit Test Layer

**Purpose**: Verify correctness of individual components

#### 2.1.1 Parser Tests
- **Target Component**: `frontend/src/parser/`
- **Test Items**:
  - Basic syntax parsing (functions, variable declarations, control flow)
  - Generic syntax parsing (`fn identity<T>(x: T) -> T`)
  - Array slice syntax (`arr[1..3]`, `arr[-1]`)
  - Struct declarations and impl blocks
  - Comment processing (line comments `#`, multi-line comments `/* */` - future support)
  - Error cases (syntax errors, invalid expressions)

**Implementation Example**:
```rust
#[test]
fn test_parse_generic_function() {
    // Parse fn identity<T>(x: T) -> T
    let code = "fn identity<T>(x: T) -> T { x }";
    let result = parse(code);
    assert!(result.is_ok());
    assert_eq!(result.generic_params.len(), 1);
}
```

#### 2.1.2 Type Checker Tests
- **Target Component**: `frontend/src/type_checker.rs`
- **Test Items**:
  - Basic type inference (u64, i64, bool)
  - Complex type inference (arrays, dictionaries, structs)
  - Generic type unification
  - Context-based type inference
  - Type error detection and reporting

**Implementation Example**:
```rust
#[test]
fn test_generic_type_unification() {
    // For fn identity<T>(x: T) -> T
    // Verify that identity(42u64) infers T = u64
    let result = infer_generic_type(generic_fn, &[UInt64]);
    assert_eq!(result, Some(UInt64));
}
```

#### 2.1.3 Lexer Tests
- **Target Component**: `frontend/src/lexer.l` and generated lexer
- **Test Items**:
  - Token generation accuracy
  - Proper comment skipping
  - Numeric literal identification (u64, i64 suffixes)
  - Operator distinction

### 2.2 Integration Test Layer

**Purpose**: Verify correctness of entire frontend (parsing → type checking)

#### 2.2.1 Frontend Integration Tests
- **Test Items**:
  - Complex generic struct type checking
  - Type inference chains (inference across multiple variables)
  - Slice operation and type checking coordination
  - Struct method call type safety
  - Error message quality

**Test Example**:
```rust
#[test]
fn test_complex_generic_struct_type_check() {
    // struct Container<T> { value: T }
    // let c = Container { value: 42u64 }
    // Verify c.value is inferred as u64 type
}
```

#### 2.2.2 Module System Integration Tests
- **Test Items**:
  - Package declaration and import parsing
  - Namespace resolution
  - Qualified name (`module::function`) type checking

### 2.3 End-to-End Test Layer

**Purpose**: Verify interpreter execution behavior

#### 2.3.1 Program Execution Tests
- **Test Items**:
  - Generic function execution (execution with multiple types)
  - Generic struct instantiation and usage
  - Array slice creation and manipulation accuracy
  - Dictionary operations (type-safe keys)
  - Complex nested structure processing
  - Recursive program execution

**Test Example**:
```rust
#[test]
fn test_generic_function_execution() {
    let code = r#"
        fn identity<T>(x: T) -> T { x }
        fn main() -> u64 {
            val result = identity(42u64)
            result
        }
    "#;
    let output = execute(code);
    assert_eq!(output, 42u64);
}
```

#### 2.3.2 Error Handling Tests
- **Test Items**:
  - Proper runtime error reporting
  - Out-of-bounds access detection
  - Type error message clarity

### 2.4 Property-Based Test Layer

**Purpose**: Verify mathematical properties of language systems

#### 2.4.1 Property Tests
- **Test Items**:
  - **Type Inference Consistency**: Same expressions always infer to same type
  - **Generic Type Substitution**: Correct behavior with different type parameters
  - **Array Slice Implementation**: Accuracy of positive/negative indices and ranges

**Implementation Example**:
```rust
proptest! {
    #[test]
    fn prop_generic_inference_consistency(values in any::<Vec<u64>>()) {
        // Same generic function call always returns same type inference result
        let result1 = infer_generic_call(&identity_fn, &values);
        let result2 = infer_generic_call(&identity_fn, &values);
        assert_eq!(result1, result2);
    }
}
```

---

## 3. Edge Cases and Non-Functional Tests

### 3.1 Edge Case Verification

#### 3.1.1 Boundary Conditions
- **Array Slices**:
  - Empty array slicing: `[][..]`
  - Single element slicing: `[1][0..1]`
  - Negative index ranges: `arr[-5..-1]`
  - Range reversal error: `arr[3..1]`

- **Generic Types**:
  - Unused type parameters: `fn unused<T>() -> u64`
  - Multiple type parameter conflicts: `fn conflict<T>(a: T, b: T) -> T` with `conflict(1u64, true)`
  - Nested types: `[[T; 3]; 5]`

- **Dictionary Operations**:
  - Empty dictionary: `dict{}`
  - Mixed key types: Bool, Int64, UInt64, String combinations

#### 3.1.2 Error Cases
- **Parse-time Errors**:
  - Incomplete expressions
  - Invalid type declarations
  - Suffix format errors: `42x64` etc.

- **Type Check-time Errors**:
  - Type mismatches
  - Undefined variable/function references
  - Out-of-scope access

- **Runtime Errors**:
  - IndexOutOfBounds
  - DivisionByZero (future implementation)
  - Stack overflow detection

### 3.2 Non-Functional Tests

#### 3.2.1 Performance Criteria
- **Parse Speed**: Analyze 1000-line programs in < 100ms
- **Type Checking**: Complete complex type inference in < 50ms
- **Memory Usage**: Standard programs use < 10MB

**Measurement Method**:
```bash
# Run benchmark tests
cd interpreter && cargo bench

# Memory profiling
/usr/bin/time -v cargo run example/fib.t
```

#### 3.2.2 Error Message Quality
- **Clarity**: Error messages clearly identify the problem
- **Position Information**: Include line and column numbers
- **Suggestions**: Provide hints for fixes (future implementation)

### 3.3 Compatibility Tests

#### 3.3.1 Backward Compatibility
- Verify all existing tests pass when adding new features
- Regression testing: Run tests on latest 5 commits

---

## 4. Test Execution and Reporting

### 4.1 Test Environment Setup

#### 4.1.1 Local Development Environment
```bash
# Frontend tests
cd frontend && cargo test --lib
cd frontend && cargo test --doc

# Interpreter tests
cd interpreter && cargo test

# Property-based tests
cd interpreter && cargo test proptest

# Full test statistics
cd interpreter && cargo test -- --test-threads=1 --nocapture
```

#### 4.1.2 Test Environment Requirements
- Rust 1.70+
- cargo test framework
- proptest crate (for property-based testing)
- Runtime: Minimum 100MB available memory

### 4.2 Test Execution and Reporting

#### 4.2.1 Daily Test Execution
```bash
# Full test run during development
cd frontend && cargo test && cd ../interpreter && cargo test
```

#### 4.2.2 Test Report Format

**Target Metrics**:
- Test success rate: > 98%
- Coverage: > 85% (core features)
- Build time: < 30 seconds (frontend)
- Test execution time: < 60 seconds (all tests)

**Report Contents**:
- Total tests and pass count
- Failed test details (file, line number, error content)
- Coverage statistics
- Performance measurement results

---

## 5. Future Extension Plans

### 5.1 Pattern Matching and Enum Tests

**Implementation Target**: Q1 2026

**Test Items**:
- Enum type declaration parsing
- Pattern matching type safety
- Verification of exhaustive case coverage

**Test Example**:
```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}

fn test_enum_pattern_matching() {
    let result: Result<u64, str> = Ok(42u64);
    match result {
        Ok(val) => { /* ... */ },
        Err(msg) => { /* ... */ },
    }
}
```

### 5.2 Option Type and Null Safety Tests

**Implementation Target**: Q2 2026

**Test Items**:
- Option<T> type checking
- Some/None pattern verification
- Null pointer reference prevention

### 5.3 Dynamic Arrays (List Type) Tests

**Implementation Target**: Q2 2026

**Test Items**:
- List<T> creation and manipulation
- push/pop/get operation accuracy
- Memory efficiency

### 5.4 Advanced Type System Tests

**Implementation Target**: Q3 2026

**Test Items**:
- Trait definition and implementation
- Type constraint (bounds) verification
- Complex generics (multiple type parameters with bounds)

### 5.5 Performance Optimization Tests

**Implementation Target**: Q4 2026

**Test Items**:
- Speed improvement through monomorphization
- Memory pool efficiency measurement
- Large program scalability

---

## 6. Test Implementation Checklist

### 6.1 Unit Tests
- [ ] Parser tests (functions, structs, generics)
- [ ] Type checker tests (type inference, unification)
- [ ] Lexer tests (token generation)

### 6.2 Integration Tests
- [ ] Frontend integration tests
- [ ] Module resolution tests
- [ ] Error message tests

### 6.3 End-to-End Tests
- [ ] Generic function execution tests
- [ ] Generic struct execution tests
- [ ] Array slice execution tests
- [ ] Dictionary operation tests
- [ ] Complex nested structure tests
- [ ] Recursive program tests

### 6.4 Edge Case Tests
- [ ] Empty and single-element array slices
- [ ] Unused type parameter errors
- [ ] Mixed key type dictionaries
- [ ] Out-of-bounds access detection

### 6.5 Non-Functional Tests
- [ ] Performance measurement (parsing, type checking)
- [ ] Memory usage measurement
- [ ] Error message quality verification

---

## 7. Reference Materials

### 7.1 Project Structure
```
toylang/
├── frontend/          # Parser and type checker
│   ├── src/
│   │   ├── parser/
│   │   ├── type_checker.rs
│   │   └── ast.rs
│   └── Cargo.toml
├── interpreter/       # Tree-walking interpreter
│   ├── src/
│   ├── tests/
│   └── Cargo.toml
├── compiler_core/     # Shared core library
├── lua_backend/       # Lua backend
├── Cargo.toml         # Workspace configuration
├── TEST_PLAN.md       # This test plan design document
└── todo.md            # Task management
```

### 7.2 Primary Technology Stack
- **Language Implementation**: Rust
- **Test Framework**: cargo test, proptest
- **Type Inference**: Unification algorithm
- **Execution**: Tree-walking interpreter
