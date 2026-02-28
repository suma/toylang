mod common;
use common::test_program;
use interpreter::object::Object;

// Helper to execute and return debug string (preserves existing test assertion style)
fn execute_regression_test(source: &str) -> Result<String, String> {
    let result = test_program(source)?;
    Ok(format!("{:?}", result.borrow()))
}

// ============================================================================
// Val/Var regression tests
// ============================================================================

/// Regression test for the specific val statement bug where identifiers
/// were converted to empty struct literals during type inference
#[test]
fn test_regression_val_struct_literal_bug() {
    let source = r#"
        fn main() -> u64 {
            val x = true
            if x {
                1u64
            } else {
                0u64
            }
        }
    "#;

    // This test specifically checks for the bug where:
    // - val x = true would store Bool(true) correctly
    // - but if x would be interpreted as StructLiteral(SymbolU32 { value: 11 }, [])
    // - causing a type mismatch error "expected Bool, found Struct"

    let result = execute_regression_test(source)
        .expect("Val statement should work without struct literal conversion bug");

    assert!(result.contains("UInt64(1)"),
            "Expected UInt64(1), got: {}. The val statement bug may have regressed.", result);
}

/// Regression test for val statement with heap operations
#[test]
fn test_regression_val_heap_operations() {
    let source = r#"
        fn main() -> u64 {
            val heap_ptr = __builtin_heap_alloc(8u64)
            val is_null = __builtin_ptr_is_null(heap_ptr)
            if is_null {
                0u64
            } else {
                __builtin_ptr_write(heap_ptr, 0u64, 42u64)
                val value = __builtin_ptr_read(heap_ptr, 0u64)
                __builtin_heap_free(heap_ptr)
                value
            }
        }
    "#;

    let result = execute_regression_test(source)
        .expect("Val statement with heap operations should work");

    assert!(result.contains("UInt64(42)"),
            "Expected UInt64(42), got: {}. Val + heap integration may have regressed.", result);
}

/// Test that the workaround doesn't break normal struct literal usage
#[test]
fn test_regression_normal_struct_literals_still_work() {
    let source = r#"
        struct Point {
            x: u64,
            y: u64
        }

        fn main() -> u64 {
            val p = Point { x: 10u64, y: 20u64 }
            p.x + p.y
        }
    "#;

    let result = execute_regression_test(source)
        .expect("Normal struct literals should still work");

    assert!(result.contains("UInt64(30)"),
            "Expected UInt64(30), got: {}. Normal struct literals may have been broken by the fix.", result);
}

/// Regression test for var vs val behavior consistency
#[test]
fn test_regression_var_val_behavior_consistency() {
    // Test with var
    let var_source = r#"
        fn test_with_var() -> u64 {
            var x = true
            if x {
                1u64
            } else {
                0u64
            }
        }

        fn main() -> u64 {
            test_with_var()
        }
    "#;

    // Test with val
    let val_source = r#"
        fn test_with_val() -> u64 {
            val x = true
            if x {
                1u64
            } else {
                0u64
            }
        }

        fn main() -> u64 {
            test_with_val()
        }
    "#;

    let var_result = execute_regression_test(var_source)
        .expect("Var statement should work");
    let val_result = execute_regression_test(val_source)
        .expect("Val statement should work");

    assert!(var_result.contains("UInt64(1)") && val_result.contains("UInt64(1)"),
            "Both var and val should produce same result. Var: {}, Val: {}", var_result, val_result);
}

/// Test that empty struct literals don't accidentally resolve as variables
#[test]
fn test_regression_empty_struct_literal_safety() {
    let source = r#"
        struct Empty {
        }

        fn main() -> u64 {
            val x = 42u64
            val empty_struct = Empty { }
            x
        }
    "#;

    let result = execute_regression_test(source)
        .expect("Empty struct literals should be handled correctly");

    assert!(result.contains("UInt64(42)"),
            "Expected UInt64(42), got: {}. Empty struct handling may be incorrect.", result);
}

/// Regression test for multiple val statements in sequence
#[test]
fn test_regression_multiple_val_statements() {
    let source = r#"
        fn main() -> u64 {
            val a = true
            val b = false
            val c = true
            val d = false

            val result1 = if a { 1u64 } else { 0u64 }
            val result2 = if b { 1u64 } else { 0u64 }
            val result3 = if c { 1u64 } else { 0u64 }
            val result4 = if d { 1u64 } else { 0u64 }

            result1 + result2 + result3 + result4
        }
    "#;

    let result = execute_regression_test(source)
        .expect("Multiple val statements should work");

    assert!(result.contains("UInt64(2)"),
            "Expected UInt64(2), got: {}. Multiple val statements may have issues.", result);
}

/// Test val statement type inference with complex expressions
#[test]
fn test_regression_val_complex_type_inference() {
    let source = r#"
        fn main() -> u64 {
            val sum = 10u64 + 20u64
            val product = 5u64 * 6u64
            val comparison = sum > product

            if comparison {
                sum
            } else {
                product
            }
        }
    "#;

    let result = execute_regression_test(source)
        .expect("Val with complex type inference should work");

    assert!(result.contains("UInt64(30)"),
            "Expected UInt64(30), got: {}. Complex val type inference may have issues.", result);
}

// ============================================================================
// Integration new features tests (dict, struct, self keyword)
// ============================================================================

#[test]
fn test_dict_and_struct_integration() {
    let source = r#"
struct DataStore {
    name: str
}

impl DataStore {
    fn __getitem__(self: Self, key: u64) -> str {
        if key == 0u64 {
            self.name
        } else {
            "default"
        }
    }
}

fn main() -> str {
    val store = DataStore { name: "MyStore" }
    val data = dict{"store_name": store[0u64], "version": "1.0"}
    data["store_name"]
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
        other => panic!("Expected String or ConstString but got {:?}", other),
    }
}

#[test]
fn test_self_with_dict_field() {
    let source = r#"
fn create_config() -> dict[str, str] {
    dict{"host": "localhost", "port": "8080"}
}

struct Server {
    id: u64
}

impl Server {
    fn get_config(self: Self) -> str {
        val config = create_config()
        config["host"]
    }
}

fn main() -> str {
    val server = Server { id: 1u64 }
    server.get_config()
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
        other => panic!("Expected String or ConstString but got {:?}", other),
    }
}

#[test]
fn test_complex_struct_indexing_with_self() {
    let source = r#"
struct Matrix2x2 {
    data: [u64; 4]  # [a, b, c, d] representing [[a,b], [c,d]]
}

impl Matrix2x2 {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.data[index]
    }

    fn get_determinant(self: Self) -> u64 {
        # det = a*d - b*c
        val a = self[0u64]
        val b = self[1u64]
        val c = self[2u64]
        val d = self[3u64]
        a * d - b * c
    }
}

fn main() -> u64 {
    val matrix = Matrix2x2 { data: [3u64, 2u64, 1u64, 4u64] }  # [[3,2], [1,4]]
    matrix.get_determinant()  # 3*4 - 2*1 = 12 - 2 = 10
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    assert_eq!(result.borrow().unwrap_uint64(), 10);
}

#[test]
fn test_dict_with_computed_keys() {
    let source = r#"
struct KeyGenerator {
    base: str
}

impl KeyGenerator {
    fn generate_key(self: Self, suffix: str) -> str {
        # In a real implementation, this would concatenate strings
        # For now, just return the suffix
        suffix
    }
}

fn main() -> str {
    val generator = KeyGenerator { base: "prefix" }
    val key = generator.generate_key("test")
    val data = dict{"test": "success", "other": "fail"}
    data[key]
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
        other => panic!("Expected String or ConstString but got {:?}", other),
    }
}

#[test]
fn test_nested_struct_indexing() {
    let source = r#"
struct InnerContainer {
    values: [u64; 2]
}

impl InnerContainer {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.values[index]
    }
}

struct OuterContainer {
    inner: InnerContainer
}

impl OuterContainer {
    fn __getitem__(self: Self, index: u64) -> u64 {
        self.inner[index]
    }
}

fn main() -> u64 {
    val inner = InnerContainer { values: [100u64, 200u64] }
    val outer = OuterContainer { inner: inner }
    outer[1u64]
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    assert_eq!(result.borrow().unwrap_uint64(), 200);
}

#[test]
fn test_dict_type_annotations_with_structs() {
    let source = r#"
struct Config {
    debug: bool
}

impl Config {
    fn is_debug(self: Self) -> bool {
        self.debug
    }
}

fn process_settings(settings: dict[str, str], config: Config) -> str {
    if config.is_debug() {
        settings["debug_mode"]
    } else {
        settings["normal_mode"]
    }
}

fn main() -> str {
    val settings: dict[str, str] = dict{
        "debug_mode": "verbose",
        "normal_mode": "quiet"
    }
    val config = Config { debug: true }
    process_settings(settings, config)
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
        other => panic!("Expected String or ConstString but got {:?}", other),
    }
}

#[test]
fn test_array_dict_struct_combination() {
    let source = r#"
struct Item {
    id: u64
}

impl Item {
    fn get_id(self: Self) -> u64 {
        self.id
    }
}

fn main() -> u64 {
    val items = [
        Item { id: 10u64 },
        Item { id: 20u64 },
        Item { id: 30u64 }
    ]

    val lookup = dict{
        "first": "0",
        "second": "1",
        "third": "2"
    }

    # Get the second item (index 1)
    val item = items[1u64]
    item.get_id()
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    assert_eq!(result.borrow().unwrap_uint64(), 20);
}

#[test]
fn test_self_keyword_type_resolution() {
    let source = r#"
struct TypeDemo {
    data: u64
}

impl TypeDemo {
    fn identity(self: Self) -> u64 {
        self.data
    }

    fn process(self: Self, multiplier: u64) -> u64 {
        self.identity() * multiplier
    }
}

fn main() -> u64 {
    val demo = TypeDemo { data: 6u64 }
    demo.process(7u64)
}
"#;
    let result = test_program(source).expect("Program should execute successfully");
    assert_eq!(result.borrow().unwrap_uint64(), 42);
}
