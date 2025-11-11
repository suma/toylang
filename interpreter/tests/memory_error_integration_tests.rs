//! Memory Management and Error Handling Integration Tests
//!
//! This module contains integration tests for memory management, error handling,
//! and regression tests. Covers object destruction, heap allocation, type errors,
//! and previously discovered bugs that needed fixing.
//!
//! Test Categories:
//! - Object destruction and cleanup
//! - Heap-allocated values (strings, arrays)
//! - Error handling and type mismatches
//! - Function arguments and parameter passing
//! - Regression tests for fixed issues

mod common;

use common::test_program;
use serial_test::serial;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;
use interpreter::object::{Object, clear_destruction_log, get_destruction_log, is_destruction_logging_enabled};
use string_interner::{DefaultSymbol, Symbol};
use compiler_core::CompilerSession;

// Helper function for regression tests
fn execute_regression_test(test_name: &str, source: &str) -> Result<String, String> {
    let mut session = CompilerSession::new();

    // Parse the program
    let mut program = session.parse_program(source)
        .map_err(|e| format!("{} - Parse error: {:?}", test_name, e))?;

    // Type check
    interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some(test_name))
        .map_err(|e| format!("{} - Type check error: {:?}", test_name, e))?;

    // Execute
    let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some(test_name))
        .map_err(|e| format!("{} - Runtime error: {}", test_name, e))?;

    Ok(format!("{:?}", result.borrow()))
}

// Helper function for heap and val integration tests
fn execute_heap_val_test(source: &str) -> Result<String, String> {
    let mut session = CompilerSession::new();

    // Parse the program
    let mut program = session.parse_program(source)
        .map_err(|e| format!("Parse error: {:?}", e))?;

    // Type check
    interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some("heap_val_test"))
        .map_err(|e| format!("Type check error: {:?}", e))?;

    // Execute
    let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some("heap_val_test"))
        .map_err(|e| format!("Runtime error: {}", e))?;

    Ok(format!("{:?}", result.borrow()))
}

#[cfg(test)]
mod function_argument_tests {
    use super::*;
    use crate::common;

    #[test]
        fn test_function_argument_type_check_success() {
            let program = r#"
                fn add_numbers(a: i64, b: i64) -> i64 {
                    a + b
                }

                fn main() -> i64 {
                    add_numbers(10i64, 20i64)
                }
            "#;
            let result = test_program(program);
            assert!(result.is_ok());
            let value = result.unwrap().borrow().unwrap_int64();
            assert_eq!(value, 30i64);
        }

    #[test]
        fn test_function_argument_type_check_error() {
            let program = r#"
                fn add_numbers(a: i64, b: i64) -> i64 {
                    a + b
                }

                fn main() -> i64 {
                    add_numbers(10u64, 20i64)
                }
            "#;
            let result = test_program(program);
            assert!(result.is_err());
            let error = result.unwrap_err();
            assert!(error.contains("type mismatch") || error.contains("TypeError"));
        }

    #[test]
        fn test_function_multiple_arguments_type_check() {
            let program = r#"
                fn add_three_numbers(a: i64, b: i64, c: i64) -> i64 {
                    a + b + c
                }

                fn main() -> i64 {
                    add_three_numbers(10i64, 20i64, 30i64)
                }
            "#;
            let result = test_program(program);
            if result.is_err() {
                println!("Error: {}", result.as_ref().unwrap_err());
            }
            assert!(result.is_ok());
            let value = result.unwrap().borrow().unwrap_int64();
            assert_eq!(value, 60i64);
        }

    #[test]
        fn test_function_wrong_argument_type_bool() {
            let program = r#"
                fn check_positive(x: i64) -> bool {
                    x > 0i64
                }

                fn main() -> bool {
                    check_positive(true)
                }
            "#;
            let result = test_program(program);
            assert!(result.is_err());
            let error = result.unwrap_err();
            assert!(error.contains("type mismatch") || error.contains("argument"));
        }

}

#[cfg(test)]
mod heap_val_integration_tests {
    use super::*;
    use crate::common;

    #[test]
    fn test_val_heap_alloc_free_cycle() {
        let source = r#"
            fn main() -> u64 {
                val iterations = 5u64
                var i = 0u64
                var success_count = 0u64

                while i < iterations {
                    val heap_ptr = __builtin_heap_alloc(8u64)
                    val is_null = __builtin_ptr_is_null(heap_ptr)

                    if is_null {
                        # Allocation failed - shouldn't happen for small allocations
                        success_count = success_count + 0u64
                    } else {
                        val test_value = i * 10u64 + 1u64
                        __builtin_ptr_write(heap_ptr, 0u64, test_value)
                        val read_value = __builtin_ptr_read(heap_ptr, 0u64)

                        if read_value == test_value {
                            success_count = success_count + 1u64
                        }

                        __builtin_heap_free(heap_ptr)
                    }

                    i = i + 1u64
                }

                success_count
            }
        "#;

        let result = execute_heap_val_test(source).expect("Heap allocation cycle should work");
        assert!(result.contains("UInt64(5)"), "Expected 5 successful iterations, got: {}", result);
    }

    #[test]
    fn test_val_heap_memory_consistency() {
        let source = r#"
            fn main() -> u64 {
                val heap_ptr1 = __builtin_heap_alloc(16u64)
                val heap_ptr2 = __builtin_heap_alloc(16u64)

                # Write different patterns to each allocation
                val pattern1 = 1234567890123456789u64  # Some large number
                val pattern2 = 9876543210987654321u64  # Another large number

                __builtin_ptr_write(heap_ptr1, 0u64, pattern1)
                __builtin_ptr_write(heap_ptr1, 8u64, pattern2)

                __builtin_ptr_write(heap_ptr2, 0u64, pattern2)
                __builtin_ptr_write(heap_ptr2, 8u64, pattern1)

                # Verify data integrity
                val read1_0 = __builtin_ptr_read(heap_ptr1, 0u64)
                val read1_8 = __builtin_ptr_read(heap_ptr1, 8u64)
                val read2_0 = __builtin_ptr_read(heap_ptr2, 0u64)
                val read2_8 = __builtin_ptr_read(heap_ptr2, 8u64)

                val check1 = if read1_0 == pattern1 { 1u64 } else { 0u64 }
                val check2 = if read1_8 == pattern2 { 1u64 } else { 0u64 }
                val check3 = if read2_0 == pattern2 { 1u64 } else { 0u64 }
                val check4 = if read2_8 == pattern1 { 1u64 } else { 0u64 }

                __builtin_heap_free(heap_ptr1)
                __builtin_heap_free(heap_ptr2)

                check1 + check2 + check3 + check4
            }
        "#;

        let result = execute_heap_val_test(source).expect("Memory consistency test should work");
        assert!(result.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", result);
    }

    #[test]
    fn test_val_heap_realloc_preserve_data() {
        let source = r#"
            fn main() -> u64 {
                val original_heap_ptr = __builtin_heap_alloc(8u64)
                val test_value = 1311768467463790319u64

                __builtin_ptr_write(original_heap_ptr, 0u64, test_value)

                # Reallocate to larger size
                val new_heap_ptr = __builtin_heap_realloc(original_heap_ptr, 16u64)

                # Check if data was preserved
                val preserved_value = __builtin_ptr_read(new_heap_ptr, 0u64)

                # Write new data to the expanded area
                val new_value = 1147797409030816545u64
                __builtin_ptr_write(new_heap_ptr, 8u64, new_value)
                val second_value = __builtin_ptr_read(new_heap_ptr, 8u64)

                __builtin_heap_free(new_heap_ptr)

                val data_preserved = if preserved_value == test_value { 1u64 } else { 0u64 }
                val new_data_ok = if second_value == new_value { 1u64 } else { 0u64 }

                data_preserved + new_data_ok
            }
        "#;

        let result = execute_heap_val_test(source).expect("Realloc data preservation test should work");
        assert!(result.contains("UInt64(2)"), "Expected both checks to pass, got: {}", result);
    }

    #[test]
    fn test_val_heap_mem_copy_operations() {
        let source = r#"
            fn main() -> u64 {
                val src_heap_ptr = __builtin_heap_alloc(32u64)
                val dst_heap_ptr = __builtin_heap_alloc(32u64)

                # Fill source with a pattern
                val value1 = 1229782938247303441u64
                val value2 = 2459565876494606882u64
                val value3 = 3689348814741910323u64
                val value4 = 4919131752989213764u64

                __builtin_ptr_write(src_heap_ptr, 0u64, value1)
                __builtin_ptr_write(src_heap_ptr, 8u64, value2)
                __builtin_ptr_write(src_heap_ptr, 16u64, value3)
                __builtin_ptr_write(src_heap_ptr, 24u64, value4)

                # Copy entire content
                __builtin_mem_copy(src_heap_ptr, dst_heap_ptr, 32u64)

                # Verify copied data
                val copied1 = __builtin_ptr_read(dst_heap_ptr, 0u64)
                val copied2 = __builtin_ptr_read(dst_heap_ptr, 8u64)
                val copied3 = __builtin_ptr_read(dst_heap_ptr, 16u64)
                val copied4 = __builtin_ptr_read(dst_heap_ptr, 24u64)

                val check1 = if copied1 == value1 { 1u64 } else { 0u64 }
                val check2 = if copied2 == value2 { 1u64 } else { 0u64 }
                val check3 = if copied3 == value3 { 1u64 } else { 0u64 }
                val check4 = if copied4 == value4 { 1u64 } else { 0u64 }

                __builtin_heap_free(src_heap_ptr)
                __builtin_heap_free(dst_heap_ptr)

                check1 + check2 + check3 + check4
            }
        "#;

        let result = execute_heap_val_test(source).expect("Memory copy test should work");
        assert!(result.contains("UInt64(4)"), "Expected all 4 checks to pass, got: {}", result);
    }

    #[test]
    fn test_val_heap_mem_set_operations() {
        let source = r#"
            fn main() -> u64 {
                val heap_ptr = __builtin_heap_alloc(16u64)
                val fill_byte = 170u64  # 170 = 0xAA in binary: 10101010

                # Fill first 8 bytes with pattern
                __builtin_mem_set(heap_ptr, fill_byte, 8u64)

                # Read as u64 (should be all 0xAA bytes = 12297829382473034410)
                val filled_value = __builtin_ptr_read(heap_ptr, 0u64)
                val expected = 12297829382473034410u64

                # Write a different pattern to second 8 bytes
                val different_value = 6148914691236517205u64
                __builtin_ptr_write(heap_ptr, 8u64, different_value)
                val second_value = __builtin_ptr_read(heap_ptr, 8u64)

                __builtin_heap_free(heap_ptr)

                val first_ok = if filled_value == expected { 1u64 } else { 0u64 }
                val second_ok = if second_value == different_value { 1u64 } else { 0u64 }

                first_ok + second_ok
            }
        "#;

        let result = execute_heap_val_test(source).expect("Memory set test should work");
        assert!(result.contains("UInt64(2)"), "Expected both checks to pass, got: {}", result);
    }

    #[test]
    fn test_val_heap_null_pointer_safety() {
        let source = r#"
            fn main() -> u64 {
                # Test null pointer detection
                val null_heap_ptr = __builtin_heap_alloc(0u64)  # Should return null for 0-size allocation
                val is_null = __builtin_ptr_is_null(null_heap_ptr)

                if is_null {
                    # Null pointer correctly detected
                    val normal_heap_ptr = __builtin_heap_alloc(8u64)
                    val is_normal_null = __builtin_ptr_is_null(normal_heap_ptr)

                    if is_normal_null {
                        # Normal allocation also null - system issue
                        0u64
                    } else {
                        # Normal allocation worked
                        __builtin_ptr_write(normal_heap_ptr, 0u64, 42u64)
                        val value = __builtin_ptr_read(normal_heap_ptr, 0u64)
                        __builtin_heap_free(normal_heap_ptr)

                        if value == 42u64 { 1u64 } else { 0u64 }
                    }
                } else {
                    # Null pointer not detected - implementation issue
                    __builtin_heap_free(null_heap_ptr)
                    0u64
                }
            }
        "#;

        let result = execute_heap_val_test(source).expect("Null pointer safety test should work");
        assert!(result.contains("UInt64(1)"), "Expected null pointer safety to work, got: {}", result);
    }

    #[test]
    fn test_val_heap_stress_small_allocations() {
        let source = r#"
            fn main() -> u64 {
                var success_count = 0u64
                var iteration = 0u64
                val max_iterations = 10u64

                while iteration < max_iterations {
                    val heap_ptr1 = __builtin_heap_alloc(8u64)
                    val heap_ptr2 = __builtin_heap_alloc(16u64)
                    val heap_ptr3 = __builtin_heap_alloc(32u64)

                    val all_allocated = if __builtin_ptr_is_null(heap_ptr1) { 0u64 } else {
                        if __builtin_ptr_is_null(heap_ptr2) { 0u64 } else {
                            if __builtin_ptr_is_null(heap_ptr3) { 0u64 } else { 1u64 }
                        }
                    }

                    if all_allocated == 1u64 {
                        # Test write/read to each allocation
                        val test_val = iteration * 100u64

                        __builtin_ptr_write(heap_ptr1, 0u64, test_val + 1u64)
                        __builtin_ptr_write(heap_ptr2, 0u64, test_val + 2u64)
                        __builtin_ptr_write(heap_ptr3, 0u64, test_val + 3u64)

                        val read1 = __builtin_ptr_read(heap_ptr1, 0u64)
                        val read2 = __builtin_ptr_read(heap_ptr2, 0u64)
                        val read3 = __builtin_ptr_read(heap_ptr3, 0u64)

                        val data_ok = if read1 == (test_val + 1u64) {
                            if read2 == (test_val + 2u64) {
                                if read3 == (test_val + 3u64) { 1u64 } else { 0u64 }
                            } else { 0u64 }
                        } else { 0u64 }

                        if data_ok == 1u64 {
                            success_count = success_count + 1u64
                        }
                    }

                    # Always try to free (free should handle null pointers safely)
                    __builtin_heap_free(heap_ptr1)
                    __builtin_heap_free(heap_ptr2)
                    __builtin_heap_free(heap_ptr3)

                    iteration = iteration + 1u64
                }

                success_count
            }
        "#;

        let result = execute_heap_val_test(source).expect("Stress test should work");
        assert!(result.contains("UInt64(10)"), "Expected 10 successful iterations, got: {}", result);
    }

}

#[cfg(test)]
mod integration_new_features_tests {
    use super::*;
    use crate::common;

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

}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::common;

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

        let result = execute_regression_test("val_struct_literal_bug", source)
            .expect("Val statement should work without struct literal conversion bug");

        assert!(result.contains("UInt64(1)"), 
                "Expected UInt64(1), got: {}. The val statement bug may have regressed.", result);
    }

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

        let result = execute_regression_test("val_heap_operations", source)
            .expect("Val statement with heap operations should work");

        assert!(result.contains("UInt64(42)"), 
                "Expected UInt64(42), got: {}. Val + heap integration may have regressed.", result);
    }

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

        let result = execute_regression_test("normal_struct_literals", source)
            .expect("Normal struct literals should still work");

        assert!(result.contains("UInt64(30)"), 
                "Expected UInt64(30), got: {}. Normal struct literals may have been broken by the fix.", result);
    }

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

        let var_result = execute_regression_test("var_behavior", var_source)
            .expect("Var statement should work");
        let val_result = execute_regression_test("val_behavior", val_source)
            .expect("Val statement should work");

        assert!(var_result.contains("UInt64(1)") && val_result.contains("UInt64(1)"),
                "Both var and val should produce same result. Var: {}, Val: {}", var_result, val_result);
    }

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

        let result = execute_regression_test("empty_struct_safety", source)
            .expect("Empty struct literals should be handled correctly");

        assert!(result.contains("UInt64(42)"), 
                "Expected UInt64(42), got: {}. Empty struct handling may be incorrect.", result);
    }

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

        let result = execute_regression_test("multiple_val_statements", source)
            .expect("Multiple val statements should work");

        assert!(result.contains("UInt64(2)"), 
                "Expected UInt64(2), got: {}. Multiple val statements may have issues.", result);
    }

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

        let result = execute_regression_test("val_complex_inference", source)
            .expect("Val with complex type inference should work");

        assert!(result.contains("UInt64(30)"), 
                "Expected UInt64(30), got: {}. Complex val type inference may have issues.", result);
    }

}

