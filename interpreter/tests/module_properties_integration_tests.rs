//! Module System and Property Tests Integration
//!
//! This module contains integration tests for module system functionality
//! and arithmetic/logical properties. Tests cover package declarations,
//! module organization, and mathematical property validation.
//!
//! Test Categories:
//! - Module and package system
//! - Module visibility and organization
//! - Arithmetic properties and invariants
//! - Expression evaluation consistency

mod common;

use common::test_program;

#[cfg(test)]
mod module_tests {
    use super::*;
    use crate::common;

    #[test]
        fn test_module_package_declaration() {
            let source = r"
            package math

            fn main() -> u64 {
                42u64
            }
            ";

            let result = test_program(source);
            assert!(result.is_ok(), "Program with package declaration should run");

            let obj = result.unwrap();
            let obj_borrowed = obj.borrow();
            match &*obj_borrowed {
                interpreter::object::Object::UInt64(value) => {
                    assert_eq!(*value, 42);
                }
                _ => panic!("Expected UInt64 result"),
            }
        }

    #[test] 
        fn test_module_import_declaration() {
            let source = r"
            import math

            fn main() -> u64 {
                42u64
            }
            ";

            let result = test_program(source);
            assert!(result.is_ok(), "Program with import declaration should run");

            let obj = result.unwrap();
            let obj_borrowed = obj.borrow();
            match &*obj_borrowed {
                interpreter::object::Object::UInt64(value) => {
                    assert_eq!(*value, 42);
                }
                _ => panic!("Expected UInt64 result"),
            }
        }

    #[test]
        fn test_module_package_and_import() {
            let source = r"
            package main
            import math

            fn main() -> u64 {
                42u64
            }
            ";

            let result = test_program(source);
            assert!(result.is_ok(), "Program with package and import should run");

            let obj = result.unwrap();
            let obj_borrowed = obj.borrow();
            match &*obj_borrowed {
                interpreter::object::Object::UInt64(value) => {
                    assert_eq!(*value, 42);
                }
                _ => panic!("Expected UInt64 result"),
            }
        }

}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::common;

    #[test]
        fn test_arithmetic_properties_extended() {
            // Test arithmetic properties with different values
            let test_cases = vec![
                (10i64, 20i64, "+", 30i64),
                (100i64, 50i64, "-", 50i64),
                (7i64, 8i64, "*", 56i64),
                (21i64, 3i64, "/", 7i64),
            ];

            for (a, b, op, expected) in test_cases {
                let program = format!(r"
                fn main() -> i64 {{
                    {}i64 {} {}i64
                }}
                ", a, op, b);

                let res = test_program(&program);
                assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
                assert_eq!(res.unwrap().borrow().unwrap_int64(), expected);
            }
        }

    #[test]
        fn test_comparison_properties_extended() {
            // Test comparison properties
            let test_cases = vec![
                (10i64, 20i64, "<", true),
                (20i64, 10i64, ">", true),
                (15i64, 15i64, "==", true),
                (10i64, 20i64, "!=", true),
                (25i64, 20i64, ">=", true),
                (15i64, 20i64, "<=", true),
            ];

            for (a, b, op, expected) in test_cases {
                let program = format!(r"
                fn main() -> bool {{
                    {}i64 {} {}i64
                }}
                ", a, op, b);

                let res = test_program(&program);
                assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
                assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
            }
        }

    #[test]
        fn test_logical_operations() {
            let test_cases = vec![
                (true, "&&", true, true),
                (true, "&&", false, false),
                (false, "&&", true, false),
                (false, "&&", false, false),
                (true, "||", true, true),
                (true, "||", false, true),
                (false, "||", true, true),
                (false, "||", false, false),
            ];

            for (a, op, b, expected) in test_cases {
                let program = format!(r"
                fn main() -> bool {{
                    {} {} {}
                }}
                ", a, op, b);

                let res = test_program(&program);
                assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
                assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
            }
        }

}

