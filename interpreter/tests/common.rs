// Shared test helpers. Each integration-test binary imports a subset of
// these, so Rust flags the rest as dead per crate; tag the whole module
// to silence the noise.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use interpreter::object::Object;

/// Path to the repo-root `core/` directory. Computed at compile
/// time relative to the interpreter crate's `CARGO_MANIFEST_DIR`.
#[allow(dead_code)]
pub fn core_modules_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

/// Default test helper — auto-loads `<repo>/core/` modules the same
/// way the interpreter binary does when launched without
/// `TOYLANG_CORE_MODULES=`. Tests that explicitly need to opt out
/// (because they declare a function with a name that would clash
/// with a stdlib symbol) should call `test_program_no_core`.
pub fn test_program(source_code: &str) -> Result<Rc<RefCell<Object>>, String> {
    let core = core_modules_dir();
    test_program_with_core(source_code, Some(core))
}

/// Backwards-compatible alias kept while call sites migrate to the
/// default `test_program`. Identical behaviour now that the default
/// path auto-loads core.
#[allow(dead_code)]
pub fn test_program_with_core_modules(
    source_code: &str,
) -> Result<Rc<RefCell<Object>>, String> {
    test_program(source_code)
}

/// Variant that opts *out* of auto-load. Use sparingly — only when
/// the test source declares a function whose name would shadow /
/// collide with a stdlib symbol auto-loaded from `core/`.
#[allow(dead_code)]
pub fn test_program_no_core(source_code: &str) -> Result<Rc<RefCell<Object>>, String> {
    test_program_with_core(source_code, None)
}

fn test_program_with_core(
    source_code: &str,
    core: Option<std::path::PathBuf>,
) -> Result<Rc<RefCell<Object>>, String> {
    let mut parser = frontend::ParserWithInterner::new(source_code);
    let mut program = parser.parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))?;

    let string_interner = parser.get_string_interner();

    interpreter::check_typing_with_core_modules(
        &mut program,
        string_interner,
        Some(source_code),
        Some("test.t"),
        core.as_deref(),
    )
    .map_err(|errors| format!("Type check errors: {errors:?}"))?;

    // Execute program
    interpreter::execute_program(&program, string_interner, Some(source_code), Some("test.t"))
}

/// Helper function to execute a program and assert the result is a u64 value
pub fn assert_program_result_u64(source_code: &str, expected: u64) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    assert_eq!(result.borrow().unwrap_uint64(), expected);
}

/// Helper function to execute a program and assert the result is an i64 value
pub fn assert_program_result_i64(source_code: &str, expected: i64) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    assert_eq!(result.borrow().unwrap_int64(), expected);
}

/// Helper function to execute a program and assert the result is an f64 value.
/// Uses bit-equality so the assertion is deterministic for tests that
/// expect specific NaN/zero patterns.
pub fn assert_program_result_f64(source_code: &str, expected: f64) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    let actual = result.borrow().unwrap_float64();
    assert_eq!(
        actual.to_bits(),
        expected.to_bits(),
        "f64 mismatch: expected {expected}, got {actual}"
    );
}

/// Helper function to execute a program and assert the result is a u64 array
pub fn assert_program_result_array_u64(source_code: &str, expected: Vec<u64>) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::Array(elements) => {
            assert_eq!(elements.len(), expected.len(), "Array length mismatch");
            for (i, elem) in elements.iter().enumerate() {
                let elem_borrowed = elem.borrow();
                match &*elem_borrowed {
                    Object::UInt64(val) => assert_eq!(*val, expected[i], "Element {} mismatch", i),
                    other => panic!("Expected UInt64 at index {} but got {:?}", i, other),
                }
            }
        }
        other => panic!("Expected Array but got {:?}", other),
    }
}

/// Helper function to execute a program and assert the result is an i64 array
pub fn assert_program_result_array_i64(source_code: &str, expected: Vec<i64>) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::Array(elements) => {
            assert_eq!(elements.len(), expected.len(), "Array length mismatch");
            for (i, elem) in elements.iter().enumerate() {
                let elem_borrowed = elem.borrow();
                match &*elem_borrowed {
                    Object::Int64(val) => assert_eq!(*val, expected[i], "Element {} mismatch", i),
                    other => panic!("Expected Int64 at index {} but got {:?}", i, other),
                }
            }
        }
        other => panic!("Expected Array but got {:?}", other),
    }
}

/// Helper function to execute a program and expect it to fail
pub fn assert_program_fails(source_code: &str) {
    let result = test_program(source_code);
    assert!(result.is_err(), "Expected program to fail but it succeeded");
}

/// Helper function to execute a program and get the result object
pub fn get_program_result(source_code: &str) -> Rc<RefCell<Object>> {
    test_program(source_code).expect("Program execution failed")
}

/// Helper function to check if a result matches the expected Object variant
pub fn assert_object_type(obj: &Object, expected_type: &str) {
    let actual_type = match obj {
        Object::Unit => "Unit",
        Object::Int64(_) => "Int64",
        Object::UInt64(_) => "UInt64",
        Object::Float64(_) => "Float64",
        Object::Bool(_) => "Bool",
        Object::String(_) => "String",
        Object::ConstString(_) => "ConstString",
        Object::Struct { .. } => "Struct",
        Object::Array(_) => "Array",
        Object::Dict(_) => "Dict",
        Object::Tuple(_) => "Tuple",
        Object::Pointer(_) => "Pointer",
        Object::Null(_) => "Null",
        Object::Allocator(_) => "Allocator",
        Object::EnumVariant { .. } => "EnumVariant",
        Object::Range { .. } => "Range",
    };
    assert_eq!(actual_type, expected_type, "Expected {} but got {}", expected_type, actual_type);
}

