mod common;

use common::test_program;

/// OOP (Object-Oriented Programming) integration tests.
///
/// This file consolidates the following test files:
/// - associated_function_tests.rs (5 tests) -> mod associated_functions
/// - self_keyword_tests.rs (8 tests) -> mod self_keyword
/// - custom_destructor_tests.rs (6 tests) -> mod custom_destructor
/// - destruction_tests.rs (8 tests) -> mod destruction

// =============================================================================
// Associated Functions
// =============================================================================
mod associated_functions {
    use super::*;

    #[test]
    fn test_associated_function_with_different_name() {
        let source = r#"
            struct Point<T> {
                x: T,
                y: T
            }

            impl<T> Point<T> {
                fn origin(value: T) -> Self {
                    Point { x: value, y: value }
                }

                fn get_x(self: Self) -> T {
                    self.x
                }
            }

            fn main() -> u64 {
                val point = Point::origin(5u64)
                point.get_x()
            }
        "#;

        let result = test_program(source);
        match result {
            Ok(val) => {
                assert_eq!(val.borrow().unwrap_uint64(), 5);
            }
            Err(e) => panic!("Program failed: {}", e),
        }
    }

    #[test]
    fn test_associated_function_multiple_parameters() {
        let source = r#"
            struct Pair<T> {
                first: T,
                second: T
            }

            impl<T> Pair<T> {
                fn create(first: T, second: T) -> Self {
                    Pair { first: first, second: second }
                }

                fn sum(self: Self) -> T {
                    self.first + self.second
                }
            }

            fn main() -> u64 {
                val pair = Pair::create(15u64, 25u64)
                pair.sum()
            }
        "#;

        let result = test_program(source);
        match result {
            Ok(val) => {
                assert_eq!(val.borrow().unwrap_uint64(), 40); // 15 + 25
            }
            Err(e) => panic!("Program failed: {}", e),
        }
    }

    #[test]
    fn test_associated_function_complex_return_type() {
        let source = r#"
            struct Container<T> {
                value: T
            }

            impl<T> Container<T> {
                fn wrap(value: T) -> Self {
                    Container { value: value }
                }

                fn double_wrap(value: T) -> Container<Container<T>> {
                    val inner = Container::wrap(value)
                    Container { value: inner }
                }
            }

            fn main() -> u64 {
                val nested = Container::double_wrap(42u64)
                nested.value.value
            }
        "#;

        let result = test_program(source);
        match result {
            Ok(val) => {
                assert_eq!(val.borrow().unwrap_uint64(), 42);
            }
            Err(e) => panic!("Program failed: {}", e),
        }
    }

    #[test]
    fn test_associated_function_type_inference_accuracy() {
        let source = r#"
            struct TypeTest<T> {
                data: T
            }

            impl<T> TypeTest<T> {
                fn from_value(data: T) -> Self {
                    TypeTest { data: data }
                }

                fn get_data(self: Self) -> T {
                    self.data
                }
            }

            fn main() -> u64 {
                # Test that type inference works correctly with different numeric types
                val uint_test = TypeTest::from_value(123u64)
                val int_test = TypeTest::from_value(-456i64)

                # Should correctly infer and convert types
                val uint_result = uint_test.get_data()
                val int_result = int_test.get_data()

                # Convert to common type for return
                if int_result < 0i64 {
                    uint_result
                } else {
                    uint_result + 1u64
                }
            }
        "#;

        let result = test_program(source);
        match result {
            Ok(val) => {
                assert_eq!(val.borrow().unwrap_uint64(), 123); // int_result is -456 < 0, so returns uint_result (123)
            }
            Err(e) => panic!("Program failed: {}", e),
        }
    }

    #[test]
    fn test_associated_function_mixed_with_regular_methods() {
        let source = r#"
            struct Calculator<T> {
                value: T
            }

            impl<T> Calculator<T> {
                fn with_value(value: T) -> Self {
                    Calculator { value: value }
                }

                fn add(self: Self, other: T) -> Self {
                    Calculator { value: self.value + other }
                }

                fn result(self: Self) -> T {
                    self.value
                }
            }

            fn main() -> u64 {
                val calc = Calculator::with_value(10u64)
                val calc2 = calc.add(20u64)
                val calc3 = calc2.add(30u64)
                calc3.result()
            }
        "#;

        let result = test_program(source);
        match result {
            Ok(val) => {
                assert_eq!(val.borrow().unwrap_uint64(), 60); // 10 + 20 + 30
            }
            Err(e) => panic!("Program failed: {}", e),
        }
    }
}

// =============================================================================
// Self Keyword
// =============================================================================
mod self_keyword {
    use super::*;
    use interpreter::object::Object;

    #[test]
    fn test_self_in_method_parameters() {
        let source = r#"
struct Person {
    age: u64
}

impl Person {
    fn get_age(self: Self) -> u64 {
        self.age
    }
}

fn main() -> u64 {
    val person = Person { age: 25u64 }
    person.get_age()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 25);
    }

    #[test]
    fn test_self_in_return_type() {
        let source = r#"
struct Builder {
    value: u64
}

impl Builder {
    fn create(self: Self) -> u64 {
        # Return the value since we can't return Self in current implementation
        self.value
    }
}

fn main() -> u64 {
    val builder = Builder { value: 42u64 }
    builder.create()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_self_field_access() {
        let source = r#"
struct Point {
    x: u64,
    y: u64
}

impl Point {
    fn sum(self: Self) -> u64 {
        self.x + self.y
    }
}

fn main() -> u64 {
    val point = Point { x: 10u64, y: 15u64 }
    point.sum()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 25);
    }

    #[test]
    fn test_self_with_complex_expressions() {
        let source = r#"
struct Calculator {
    base: u64
}

impl Calculator {
    fn multiply_by_base(self: Self, factor: u64) -> u64 {
        self.base * factor
    }
}

fn main() -> u64 {
    val calc = Calculator { base: 7u64 }
    calc.multiply_by_base(6u64)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_self_in_multiple_methods() {
        let source = r#"
struct Data {
    value: u64
}

impl Data {
    fn get_value(self: Self) -> u64 {
        self.value
    }

    fn double_value(self: Self) -> u64 {
        self.value * 2u64
    }
}

fn main() -> u64 {
    val data = Data { value: 21u64 }
    val original = data.get_value()
    val doubled = data.double_value()
    original + doubled  # 21 + 42 = 63
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 63);
    }

    #[test]
    fn test_self_with_array_field() {
        let source = r#"
struct ArrayHolder {
    numbers: [u64; 3]
}

impl ArrayHolder {
    fn get_sum(self: Self) -> u64 {
        self.numbers[0u64] + self.numbers[1u64] + self.numbers[2u64]
    }
}

fn main() -> u64 {
    val holder = ArrayHolder { numbers: [5u64, 10u64, 15u64] }
    holder.get_sum()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_uint64(), 30);
    }

    #[test]
    fn test_self_with_boolean_logic() {
        let source = r#"
struct Validator {
    min_value: u64,
    max_value: u64
}

impl Validator {
    fn is_valid(self: Self, value: u64) -> bool {
        value >= self.min_value && value <= self.max_value
    }
}

fn main() -> bool {
    val validator = Validator { min_value: 10u64, max_value: 20u64 }
    validator.is_valid(15u64)
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        assert_eq!(result.borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_self_string_operations() {
        let source = r#"
struct TextProcessor {
    prefix: str
}

impl TextProcessor {
    fn get_prefix(self: Self) -> str {
        self.prefix
    }
}

fn main() -> str {
    val processor = TextProcessor { prefix: "Hello" }
    processor.get_prefix()
}
"#;
        let result = test_program(source).expect("Program should execute successfully");
        let borrowed = result.borrow();
        match &*borrowed {
            Object::String(_) | Object::ConstString(_) => {}, // Success - we got a string (either type)
            other => panic!("Expected String or ConstString but got {:?}", other),
        }
    }
}

// =============================================================================
// Custom Destructor
// =============================================================================
mod custom_destructor {
    use super::*;
    use interpreter::object::{Object, clear_destruction_log};

    #[test]
    fn test_custom_drop_method_call() {
        clear_destruction_log();

        let source_code = r#"
struct Resource {
    name: str
}

impl Resource {
    fn __drop__(self: Self) {
        # Custom destructor logic would go here
        # For testing, we just rely on the logging system
    }
}

fn main() -> u64 {
    val resource = Resource { name: "test_resource" }
    42u64
}
"#;

        // Parse and execute the program
        let result = test_program(source_code);
        assert!(result.is_ok(), "Program should execute successfully: {:?}", result);

        // The test just verifies that __drop__ method can be defined and parsed successfully
        // Explicit destructor calling would be tested in integration tests
    }

    #[test]
    fn test_explicit_destructor_call() {
        clear_destruction_log();

        let source_code = r#"
struct TestStruct {
    value: u64
}

impl TestStruct {
    fn __drop__(self: Self) {
        # This method exists and should be callable
    }
}

fn main() -> u64 {
    val obj = TestStruct { value: 42u64 }
    99u64
}
"#;

        // Parse and execute the program
        let result = test_program(source_code);
        assert!(result.is_ok(), "Program with __drop__ method should execute: {:?}", result);

        // For now, we just test that programs with __drop__ methods can be parsed and executed
        // The explicit calling mechanism would be used in scenarios where objects need to be
        // cleaned up before going out of scope naturally
    }

    #[test]
    fn test_struct_without_drop_method() {
        clear_destruction_log();

        let source_code = r#"
struct SimpleStruct {
    data: u64
}

impl SimpleStruct {
    fn get_data(self: Self) -> u64 {
        self.data
    }
}

fn main() -> u64 {
    val obj = SimpleStruct { data: 100u64 }
    obj.get_data()
}
"#;

        let result = test_program(source_code);
        assert!(result.is_ok(), "Program without __drop__ method should work normally: {:?}", result);

        if let Ok(actual_rc) = result {
            let actual = actual_rc.borrow();
            match &*actual {
                Object::UInt64(100) => {}, // Expected
                other => panic!("Expected UInt64(100), got {:?}", other),
            }
        }
    }

    #[test]
    fn test_multiple_structs_with_drop() {
        clear_destruction_log();

        let source_code = r#"
struct ResourceA {
    name: str
}

struct ResourceB {
    id: u64
}

impl ResourceA {
    fn __drop__(self: Self) {
        # ResourceA destructor
    }
}

impl ResourceB {
    fn __drop__(self: Self) {
        # ResourceB destructor
    }
}

fn main() -> u64 {
    val a = ResourceA { name: "resource_a" }
    val b = ResourceB { id: 123u64 }
    456u64
}
"#;

        let result = test_program(source_code);
        assert!(result.is_ok(), "Multiple structs with __drop__ should work: {:?}", result);
    }

    #[test]
    fn test_drop_method_signature() {
        // Test that __drop__ method must have the correct signature (self: Self)
        let source_code = r#"
struct TestStruct {
    data: u64
}

impl TestStruct {
    fn __drop__(self: Self) {
        # Correct signature - takes self by value
    }
}

fn main() -> u64 {
    val obj = TestStruct { data: 42u64 }
    1u64
}
"#;

        let result = test_program(source_code);
        assert!(result.is_ok(), "Correct __drop__ signature should work: {:?}", result);
    }

    #[test]
    fn test_drop_method_with_complex_cleanup() {
        clear_destruction_log();

        let source_code = r#"
struct ComplexResource {
    data: [u64; 3],
    name: str
}

impl ComplexResource {
    fn __drop__(self: Self) {
        # In a real implementation, this could:
        # - Close file handles
        # - Release network connections
        # - Free external resources
        # - Log cleanup actions
    }

    fn get_sum(self: Self) -> u64 {
        self.data[0u64] + self.data[1u64] + self.data[2u64]
    }
}

fn main() -> u64 {
    val resource = ComplexResource {
        data: [1u64, 2u64, 3u64],
        name: "complex"
    }
    resource.get_sum()
}
"#;

        let result = test_program(source_code);
        assert!(result.is_ok(), "Complex struct with __drop__ should work: {:?}", result);

        // Verify the computation works correctly
        if let Ok(actual_rc) = result {
            let actual = actual_rc.borrow();
            if let Object::UInt64(sum) = &*actual {
                assert_eq!(*sum, 6); // 1 + 2 + 3 = 6
            } else {
                panic!("Expected UInt64(6), got {:?}", actual);
            }
        } else {
            panic!("Program failed: {:?}", result);
        }
    }
}

// =============================================================================
// Destruction (low-level object destruction logging tests)
// =============================================================================
mod destruction {
    use serial_test::serial;
    use std::rc::Rc;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use interpreter::object::{Object, clear_destruction_log, get_destruction_log, is_destruction_logging_enabled};
    use string_interner::{DefaultSymbol, Symbol};

    #[test]
    #[serial]
    fn test_destruction_logging_status() {
        // Test that we can check if logging is enabled
        let logging_enabled = is_destruction_logging_enabled();

        // In debug builds or when debug-logging feature is enabled, logging should be active
        #[cfg(any(debug_assertions, feature = "debug-logging"))]
        assert!(logging_enabled, "Logging should be enabled in debug mode or with debug-logging feature");

        // In release builds without debug-logging feature, logging should be disabled
        #[cfg(not(any(debug_assertions, feature = "debug-logging")))]
        assert!(!logging_enabled, "Logging should be disabled in release mode without debug-logging feature");
    }

    #[test]
    #[serial]
    fn test_struct_destruction_logging() {
        // Clear any previous destruction logs
        clear_destruction_log();

        // Create a struct object
        let type_name = DefaultSymbol::try_from_usize(1).unwrap();
        let struct_obj = {
            let mut fields = HashMap::new();
            fields.insert("x".to_string(), Rc::new(RefCell::new(Object::Int64(42))));
            fields.insert("y".to_string(), Rc::new(RefCell::new(Object::Int64(24))));
            Rc::new(RefCell::new(Object::Struct {
                type_name,
                fields: Box::new(fields),
            }))
        };

        // Check reference count before dropping
        assert_eq!(Rc::strong_count(&struct_obj), 1, "Struct should have exactly 1 reference");

        // Drop the struct
        drop(struct_obj);

        // Check destruction log
        let log = get_destruction_log();

        // Only check log content if logging is enabled
        if is_destruction_logging_enabled() {
            let struct_destructions = log.iter().filter(|entry| entry.contains("Destructing struct_")).collect::<Vec<_>>();
            assert!(log.len() >= 1, "Expected at least 1 log entry, found {}. Log: {:?}", log.len(), log);
            assert!(
                log.iter().any(|entry| entry.contains("Destructing struct_")),
                "Expected 'Destructing struct_' in log. Found: {:?}. Full log: {:?}",
                struct_destructions, log
            );
        } else {
            // In release mode without debug-logging feature, log should be empty
            // (though the log storage still exists for API compatibility)
        }
    }

    #[test]
    #[serial]
    fn test_array_destruction_logging() {
        clear_destruction_log();

        let array_obj = {
            let elements = vec![
                Rc::new(RefCell::new(Object::Int64(1))),
                Rc::new(RefCell::new(Object::Int64(2))),
                Rc::new(RefCell::new(Object::Int64(3))),
            ];
            Rc::new(RefCell::new(Object::Array(Box::new(elements))))
        };

        // Check reference count before dropping
        assert_eq!(Rc::strong_count(&array_obj), 1, "Array should have exactly 1 reference");

        // Drop the array
        drop(array_obj);

        let log = get_destruction_log();
        if is_destruction_logging_enabled() {
            let array_destructions = log.iter().filter(|entry| entry.contains("Destructing array")).collect::<Vec<_>>();
            assert!(
                log.iter().any(|entry| entry.contains("Destructing array with 3 elements")),
                "Expected 'Destructing array with 3 elements' in log. Found: {:?}. Full log: {:?}",
                array_destructions, log
            );
        }
    }

    #[test]
    #[serial]
    fn test_dict_destruction_logging() {
        clear_destruction_log();

        let dict_obj = {
            let mut dict = HashMap::new();
            dict.insert(
                interpreter::object::ObjectKey::new(Object::Int64(1)),
                Rc::new(RefCell::new(Object::String("value1".to_string())))
            );
            dict.insert(
                interpreter::object::ObjectKey::new(Object::Int64(2)),
                Rc::new(RefCell::new(Object::String("value2".to_string())))
            );
            Rc::new(RefCell::new(Object::Dict(Box::new(dict))))
        };

        // Check reference count before dropping
        assert_eq!(Rc::strong_count(&dict_obj), 1, "Dict should have exactly 1 reference");

        // Drop the dict
        drop(dict_obj);

        let log = get_destruction_log();
        if is_destruction_logging_enabled() {
            let dict_destructions = log.iter().filter(|entry| entry.contains("Destructing dict")).collect::<Vec<_>>();
            assert!(
                log.iter().any(|entry| entry.contains("Destructing dict with 2 entries")),
                "Expected 'Destructing dict with 2 entries' in log. Found: {:?}. Full log: {:?}",
                dict_destructions, log
            );
        }
    }

    #[test]
    #[serial]
    fn test_string_destruction_logging() {
        clear_destruction_log();

        let string_obj = Rc::new(RefCell::new(Object::String("test dynamic string".to_string())));

        // Check reference count before dropping
        assert_eq!(Rc::strong_count(&string_obj), 1, "String should have exactly 1 reference");

        // Drop the string
        drop(string_obj);

        let log = get_destruction_log();
        if is_destruction_logging_enabled() {
            let string_destructions = log.iter().filter(|entry| entry.contains("Destructing dynamic string")).collect::<Vec<_>>();
            assert!(
                log.iter().any(|entry| entry.contains("Destructing dynamic string: test dynamic string")),
                "Expected 'Destructing dynamic string: test dynamic string' in log. Found: {:?}. Full log: {:?}",
                string_destructions, log
            );
        }
    }

    #[test]
    #[serial]
    fn test_primitive_types_no_logging() {
        clear_destruction_log();

        {
            let _bool_obj = Rc::new(RefCell::new(Object::Bool(true)));
            let _int_obj = Rc::new(RefCell::new(Object::Int64(42)));
            let _uint_obj = Rc::new(RefCell::new(Object::UInt64(24)));
            let _const_str_obj = Rc::new(RefCell::new(Object::ConstString(DefaultSymbol::try_from_usize(1).unwrap())));
            let _null_obj = Rc::new(RefCell::new(Object::Null));
            let _unit_obj = Rc::new(RefCell::new(Object::Unit));
            // All objects will be dropped when they go out of scope
        }

        let log = get_destruction_log();
        // Primitive types should not generate destruction logs
        assert!(log.is_empty() || log.iter().all(|entry|
            !entry.contains("Bool") &&
            !entry.contains("Int64") &&
            !entry.contains("UInt64") &&
            !entry.contains("ConstString") &&
            !entry.contains("Null") &&
            !entry.contains("Unit")
        ));
    }

    #[test]
    #[serial]
    fn test_reference_counting_destruction() {
        clear_destruction_log();

        // Create a struct with shared references
        let type_name = DefaultSymbol::try_from_usize(1).unwrap();
        let shared_value = Rc::new(RefCell::new(Object::Int64(100)));

        // Create two structs sharing the same field value (wrapped in Rc<RefCell<>>)
        let struct1 = {
            let mut fields1 = HashMap::new();
            fields1.insert("shared".to_string(), shared_value.clone());
            Rc::new(RefCell::new(Object::Struct {
                type_name,
                fields: Box::new(fields1),
            }))
        };

        let struct2 = {
            let mut fields2 = HashMap::new();
            fields2.insert("shared".to_string(), shared_value.clone());
            Rc::new(RefCell::new(Object::Struct {
                type_name,
                fields: Box::new(fields2),
            }))
        };

        // Check reference counts before dropping
        assert_eq!(Rc::strong_count(&struct1), 1, "struct1 should have exactly 1 reference");
        assert_eq!(Rc::strong_count(&struct2), 1, "struct2 should have exactly 1 reference");
        assert_eq!(Rc::strong_count(&shared_value), 3, "shared_value should have 3 references (local + 2 structs)");

        // Drop struct2 first
        drop(struct2);
        assert_eq!(Rc::strong_count(&shared_value), 2, "shared_value should have 2 references after dropping struct2");

        // Drop struct1
        drop(struct1);
        assert_eq!(Rc::strong_count(&shared_value), 1, "shared_value should have 1 reference after dropping both structs");

        let log = get_destruction_log();

        // Check destruction logging if available
        if is_destruction_logging_enabled() {
            let struct_destructions = log.iter().filter(|entry| entry.contains("Destructing struct_")).collect::<Vec<_>>();
            assert!(
                struct_destructions.len() >= 2,
                "Expected at least 2 struct destructions, found {}. Destructions: {:?}. Full log: {:?}",
                struct_destructions.len(), struct_destructions, log
            );
        }
    }

    #[test]
    #[serial]
    fn test_nested_object_destruction() {
        clear_destruction_log();

        {
            // Create a struct containing an array containing other objects
            let type_name = DefaultSymbol::try_from_usize(1).unwrap();
            let inner_array = vec![
                Rc::new(RefCell::new(Object::Int64(1))),
                Rc::new(RefCell::new(Object::String("inner".to_string()))),
            ];

            let mut fields = HashMap::new();
            fields.insert("data".to_string(), Rc::new(RefCell::new(Object::Array(Box::new(inner_array)))));

            let _complex_struct = Rc::new(RefCell::new(Object::Struct {
                type_name,
                fields: Box::new(fields),
            }));
            // All nested objects should be properly destroyed

            drop(_complex_struct);
        }

        let log = get_destruction_log();
        // Should see destruction of struct, array, and string
        if is_destruction_logging_enabled() {
            assert!(log.iter().any(|entry| entry.contains("Destructing struct_")));
            assert!(log.iter().any(|entry| entry.contains("Destructing array with")));
            assert!(log.iter().any(|entry| entry.contains("Destructing dynamic string: inner")));
        }
    }
}
