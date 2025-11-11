//! OOP Features Integration Tests
//!
//! This module contains integration tests for object-oriented programming features:
//! associated functions, self keyword, method calls, and destructors.
//! Tests cover struct methods, associated functions, and lifecycle management.
//!
//! Test Categories:
//! - Associated function definitions and calls
//! - Self keyword and method binding
//! - Custom destructor implementation
//! - Method chaining and composition

mod common;

use common::test_program;
use interpreter::object::{Object, clear_destruction_log};
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(test)]
mod associated_function_tests {
    use super::*;
    use crate::common;

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

#[cfg(test)]
mod custom_destructor_tests {
    use super::*;
    use crate::common;

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

#[cfg(test)]
mod self_keyword_tests {
    use super::*;
    use crate::common;

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

