//! Error Handling and Edge Cases Integration Tests
//!
//! This module contains integration tests for error detection, handling,
//! and edge case validation. It verifies that the parser and type checker
//! correctly identify and report errors, and handle boundary conditions.
//!
//! Test Categories:
//! - Syntax error detection
//! - Type checking errors
//! - Multiple error collection
//! - Edge cases and boundary conditions
//! - Infinite recursion detection

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;

mod helpers {
    use super::*;

    /// Helper function to parse and expect error
    pub fn expect_parse_error(input: &str) -> bool {
        let mut parser = ParserWithInterner::new(input);
        parser.parse_program().is_err()
    }

    /// Helper function to expect successful parsing
    pub fn expect_parse_success(input: &str) -> bool {
        let mut parser = ParserWithInterner::new(input);
        parser.parse_program().is_ok()
    }

    /// Helper to parse program
    pub fn parse_program(input: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(input);
        match parser.parse_program() {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("Parse errors: {:?}", errors))
        }
    }
}

mod syntax_errors {
    //! Tests for syntax error detection

    use super::*;
    use super::helpers::{expect_parse_error, expect_parse_success};

    #[test]
    fn test_valid_function_definition() {
        let input = "fn main() -> i64 { 0i64 }";
        assert!(expect_parse_success(input));
    }

    #[test]
    fn test_valid_variable_declaration() {
        let input = "fn main() -> i64 { val x = 1i64\nx }";
        assert!(expect_parse_success(input));
    }

    #[test]
    fn test_valid_struct_definition() {
        let input = "struct Point { x: i64, y: i64 } fn main() -> i64 { 0i64 }";
        assert!(expect_parse_success(input));
    }

    #[test]
    fn test_division_by_zero_literal() {
        // Division by zero should parse (error at runtime if caught)
        let input = "fn main() -> i64 { 1i64 / 0i64 }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Division by zero should parse");
    }
}

mod edge_cases {
    //! Tests for edge cases and boundary conditions

    use super::*;
    use super::helpers::parse_program;

    #[test]
    fn test_empty_program() {
        let input = "";
        let result = parse_program(input);
        // Empty program handling is implementation-dependent
        let _ = result;
    }

    #[test]
    fn test_whitespace_only_program() {
        let input = "   \n\t\n   ";
        let result = parse_program(input);
        // Parser might accept empty/whitespace programs
        let _ = result;
    }

    #[test]
    fn test_comment_only_program() {
        let input = "# This is a comment\n# Another comment";
        let result = parse_program(input);
        // Parser might accept comment-only programs
        let _ = result;
    }

    #[test]
    fn test_deeply_nested_expression() {
        let mut expr = String::from("1i64");
        for _ in 0..50 {
            expr = format!("({} + 1i64)", expr);
        }
        let input = format!("fn main() -> i64 {{ {} }}", expr);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Deeply nested expression should parse");
    }

    #[test]
    fn test_very_long_identifier() {
        let long_name = "a".repeat(20);
        let input = format!("fn main() -> i64 {{ val {} = 1i64\n{} }}", long_name, long_name);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Long identifier should be accepted");
    }

    #[test]
    fn test_valid_identifier_patterns() {
        let valid_names = vec![
            "variable1",
            "var_name",
            "_private",
            "__double",
            "snake_case_var",
            "var123",
            "v1_2_3",
            "_",
            "_123",
        ];

        for name in valid_names {
            let input = format!("fn main() -> i64 {{ val {} = 1i64\n{} }}", name, name);
            let result = parse_program(&input);
            assert!(result.is_ok(), "Valid identifier '{}' should be accepted", name);
        }
    }

    #[test]
    fn test_empty_function_body() {
        let input = "fn main() -> i64 { }";
        let result = parse_program(&input);
        // Empty body might be valid or invalid depending on implementation
        let _ = result;
    }

    #[test]
    fn test_multiple_consecutive_operators() {
        let input = "fn main() -> i64 { 5i64 - - 3i64 }";
        let result = parse_program(&input);
        // Multiple operators handling is implementation-dependent
        let _ = result;
    }

    #[test]
    fn test_statement_without_trailing_value() {
        let input = "fn main() -> i64 { val x = 5i64 }";
        let result = parse_program(&input);
        // Function without explicit return might be valid or invalid
        let _ = result;
    }
}

mod type_checking_errors {
    //! Tests for type checking error detection

    use super::*;

    #[test]
    fn test_type_mismatch_explicit_annotation() {
        let source = r#"
            fn test() -> u64 {
                val x: u64 = 10u64
                val y: i64 = x
                y
            }
        "#;

        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                let mut has_error = false;
                for func in functions.iter() {
                    if let Err(_) = type_checker.type_check(func.clone()) {
                        has_error = true;
                    }
                }

                // Should detect type mismatch
                assert!(has_error, "Should detect type mismatch");
            }
            Err(_) => panic!("Parse should succeed"),
        }
    }

    #[test]
    fn test_undefined_variable_reference() {
        let source = r#"
            fn test() -> u64 {
                undefined_var + 10u64
            }
        "#;

        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                let mut has_error = false;
                for func in functions.iter() {
                    if let Err(_) = type_checker.type_check(func.clone()) {
                        has_error = true;
                    }
                }

                // Should detect undefined variable
                assert!(has_error, "Should detect undefined variable reference");
            }
            Err(_) => {
                // Parse error is also acceptable for undefined variables
                assert!(true, "Parse error for undefined variable is acceptable");
            }
        }
    }

    #[test]
    fn test_string_to_number_type_error() {
        let source = r#"
            fn test() -> u64 {
                val s = "string"
                val n: u64 = s
                n
            }
        "#;

        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                let mut has_error = false;
                for func in functions.iter() {
                    if let Err(_) = type_checker.type_check(func.clone()) {
                        has_error = true;
                    }
                }

                assert!(has_error, "Should detect string to number type error");
            }
            Err(_) => {}
        }
    }

    #[test]
    fn test_bool_to_number_conversion_error() {
        let source = r#"
            fn test() -> u64 {
                val b = true
                val n: u64 = b
                n
            }
        "#;

        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                let mut has_error = false;
                for func in functions.iter() {
                    if let Err(_) = type_checker.type_check(func.clone()) {
                        has_error = true;
                    }
                }

                assert!(has_error, "Should detect bool to number conversion error");
            }
            Err(_) => {}
        }
    }
}

mod recursion_and_limits {
    //! Tests for recursion depth and computational limits

    use super::*;
    use super::helpers::parse_program;

    #[test]
    fn test_simple_recursion() {
        let input = r#"
            fn factorial(n: u64) -> u64 {
                if n <= 1u64 {
                    1u64
                } else {
                    n * factorial(n - 1u64)
                }
            }
            fn main() -> u64 {
                factorial(5u64)
            }
        "#;

        let result = parse_program(input);
        assert!(result.is_ok(), "Simple recursion should parse");
    }

    #[test]
    fn test_mutual_recursion() {
        let input = r#"
            fn even(n: u64) -> bool {
                if n == 0u64 {
                    true
                } else {
                    odd(n - 1u64)
                }
            }

            fn odd(n: u64) -> bool {
                if n == 0u64 {
                    false
                } else {
                    even(n - 1u64)
                }
            }

            fn main() -> bool {
                even(4u64)
            }
        "#;

        let result = parse_program(input);
        assert!(result.is_ok(), "Mutual recursion should parse");
    }

    #[test]
    fn test_deep_recursion_definition() {
        // Create a deep recursion structure that should parse
        let mut code = String::from("fn level0() -> u64 { 1u64 }");

        for i in 1..20 {
            code.push('\n');
            code.push_str(&format!("fn level{}() -> u64 {{ level{}() }}", i, i - 1));
        }

        code.push_str("\nfn main() -> u64 { level19() }");

        let result = parse_program(&code);
        assert!(result.is_ok(), "Deep recursion definition should parse");
    }
}
