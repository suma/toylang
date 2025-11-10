//! Type System Integration Tests
//!
//! This module contains integration tests for the type checking subsystem.
//! It validates context-based type inference, generic type unification,
//! and complex type interactions across the frontend.
//!
//! Test Categories:
//! - Basic type inference with explicit types
//! - Nested and complex type structures
//! - Function and struct type checking
//! - Type error detection and propagation
//! - Module system integration with type checking
//! - Qualified name resolution

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;

mod helpers {
    use super::*;

    /// Helper function to parse and type-check source code
    pub fn parse_and_check(source: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() && program.function.is_empty() {
                    return Err("No statements or functions found".to_string());
                }

                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
                let mut errors = Vec::new();

                for func in functions.iter() {
                    if let Err(e) = type_checker.type_check(func.clone()) {
                        errors.push(format!("{:?}", e));
                    }
                }

                if !errors.is_empty() {
                    Err(errors.join("\n"))
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(format!("Parse error: {:?}", e))
        }
    }
}

mod basic_functionality {
    //! Basic type inference tests with explicit types

    use super::*;
    use super::helpers::parse_and_check;

    #[test]
    fn test_basic_type_inference() {
        let source = r#"
            fn simple() -> u64 {
                val x = 10u64
                x
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_nested_array_type_inference() {
        let source = r#"
            fn test_nested() -> [[u64; 2]; 3] {
                val inner1 = [1u64, 2u64]
                val inner2 = [3u64, 4u64]
                val inner3 = [5u64, 6u64]
                [inner1, inner2, inner3]
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_function_call_type_inference() {
        let source = r#"
            fn helper(x: u64) -> u64 {
                x * 2u64
            }

            fn test_call_inference() -> u64 {
                val input = 5u64
                val result = helper(input)
                result + 10u64
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_field_type_inference() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn test_struct_inference() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                val sum = p.x + p.y
                sum
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_index_type_inference() {
        let source = r#"
            fn array_operations() -> u64 {
                val arr = [1u64, 2u64, 3u64, 4u64, 5u64]
                val element = arr[2u64]
                element
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_mutable_variable_inference() {
        let source = r#"
            fn mutability() -> u64 {
                val immut = 10u64
                var mut_var = 20u64
                mut_var = mut_var + immut
                mut_var
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_element_assignment_inference() {
        let source = r#"
            fn array_assign() -> [u64; 3] {
                var arr = [0u64, 0u64, 0u64]
                arr[0u64] = 10u64
                arr[1u64] = 20u64
                arr[2u64] = 30u64
                arr
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }
}

mod advanced_scenarios {
    //! Complex type interactions and multi-feature scenarios

    use super::*;
    use super::helpers::parse_and_check;

    #[test]
    fn test_nested_function_call_inference() {
        let source = r#"
            fn add(a: u64, b: u64) -> u64 { a + b }
            fn multiply(x: u64, y: u64) -> u64 { x * y }

            fn nested_calls() -> u64 {
                val x = 2u64
                val y = 3u64
                val z = 4u64
                add(multiply(x, y), z)
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_recursive_type_inference() {
        let source = r#"
            fn factorial(n: u64) -> u64 {
                if n <= 1u64 {
                    1u64
                } else {
                    val prev = factorial(n - 1u64)
                    n * prev
                }
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_complex_expression_inference() {
        let source = r#"
            fn complex_expr() -> u64 {
                val a = 5u64
                val b = 10u64
                val c = 15u64
                val result = (a + b) * c / (b - a)
                result
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_conditional_type_inference() {
        let source = r#"
            fn conditional_inference(flag: bool) -> u64 {
                val result = if flag {
                    100u64
                } else {
                    200u64
                }
                result
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_for_loop_type_inference() {
        let source = r#"
            fn loop_inference() -> u64 {
                var sum = 0u64
                for i in 0u64 to 10u64 {
                    sum = sum + i
                }
                sum
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bidirectional_type_inference() {
        let source = r#"
            fn bidirectional() -> [u64; 3] {
                var result = [0u64, 0u64, 0u64]
                result[0u64] = 10u64
                result[1u64] = 20u64
                result[2u64] = 30u64
                result
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_multiple_constraint_resolution() {
        let source = r#"
            fn complex_inference() -> u64 {
                val x = 10u64
                val z: u64 = x
                x + z
            }
        "#;

        assert!(parse_and_check(source).is_ok());
    }
}

mod error_cases {
    //! Error detection and type mismatch validation

    use super::*;
    use super::helpers::parse_and_check;

    #[test]
    fn test_conflicting_type_constraints() {
        let source = r#"
            fn conflicting() -> u64 {
                val x = true
                val y: u64 = x
                y
            }
        "#;

        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool to u64 conversion should fail");
    }

    #[test]
    fn test_inference_error_propagation() {
        let source = r#"
            fn error_prop() -> u64 {
                val x = "string"
                val y = x + 10u64
                y
            }
        "#;

        let result = parse_and_check(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_circular_type_dependency() {
        let source = r#"
            fn circular() -> u64 {
                val a = b
                val b = a
                a
            }
        "#;

        let result = parse_and_check(source);
        assert!(result.is_err());
    }
}

mod module_system_integration {
    //! Module system integration with type checking

    use super::*;

    #[test]
    fn test_valid_package_declaration() {
        let source = r"
        package math

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Program should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert!(type_checker.get_current_package().is_some());
    }

    #[test]
    fn test_empty_package_name_error() {
        let source = r"
        package

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();

        assert!(result.is_err(), "Empty package name should cause parse error");
    }

    #[test]
    fn test_module_qualified_function_call() {
        let source = r"
        package main
        import math

        fn main() -> u64 {
            math.add(1u64, 2u64)
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Program should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 1);
    }

    #[test]
    fn test_unknown_module_member() {
        let source = r"
        package main
        import math

        fn main() -> u64 {
            math.unknown_function()
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Program should parse successfully");

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 1);
    }

    #[test]
    fn test_package_without_main() {
        let source = r"
        package utils

        fn helper() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();

        assert!(result.is_ok(), "Package without main is valid");
    }

    #[test]
    fn test_nested_package_imports() {
        let source = r"
        package main
        import math
        import utils

        fn main() -> u64 {
            42u64
        }
        ";

        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok());

        let mut program = result.unwrap();
        let string_interner = parser.get_string_interner();

        let type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
        assert_eq!(type_checker.imported_modules.len(), 2);
    }
}
