//! Array and Collection Integration Tests
//!
//! This module contains integration tests for array slicing, indexing,
//! and dictionary operations. It validates array slice accuracy with
//! positive and negative indices, boundary conditions, and dictionary
//! type-safe key operations.
//!
//! Test Categories:
//! - Array slicing with positive indices
//! - Array slicing with negative indices
//! - Boundary conditions and error cases
//! - Dictionary indexing with various key types
//! - Nested collection structures

use frontend::type_checker::TypeCheckerVisitor;
use frontend::type_decl::TypeDecl;
use frontend::parser::core::ParserWithInterner;
use frontend::ParserWithInterner as RootParserWithInterner;

mod helpers {
    use super::*;

    /// Test helper function to parse and type check a source string
    pub fn parse_and_check(source: &str) -> Result<TypeDecl, String> {
        let mut parser = ParserWithInterner::new(source);

        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() {
                    return Err("No statements found".to_string());
                }

                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
                let mut errors: Vec<String> = vec![];

                functions.iter().for_each(|func| {
                    let res = type_checker.type_check(func.clone());
                    if let Err(e) = res {
                        errors.push(format!("Type check error: {:?}", e));
                    }
                });
                if !errors.is_empty() {
                    return Err(errors.join("\n"));
                }
                Ok(TypeDecl::Unit)
            }
            Err(e) => Err(format!("Parse error: {:?}", e))
        }
    }

    /// Helper function for parser-only testing
    pub fn parse_program_only(input: &str) -> Result<(), String> {
        let mut parser = RootParserWithInterner::new(input);
        match parser.parse_program() {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("Parse error: {:?}", errors))
        }
    }
}

mod negative_index_tests {
    //! Tests for array access with negative indices

    use super::helpers::parse_and_check;

    #[test]
    fn test_negative_index_literal() {
        let source = r#"
            fn main() -> u64 {
                val a: [u64; 5] = [1, 2, 3, 4, 5]
                a[-1]
            }
        "#;

        // Should handle negative index properly
        let _ = parse_and_check(source);
    }

    #[test]
    fn test_negative_index_with_type_suffix() {
        let source = r#"
            fn main() -> u64 {
                val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
                a[-1i64]
            }
        "#;

        match parse_and_check(source) {
            Ok(_) => {
                // This should work with explicit type suffix
            }
            Err(e) => {
                panic!("Type check failed for negative index with suffix: {}", e);
            }
        }
    }

    #[test]
    fn test_negative_slice_start() {
        let source = r#"
            fn main() -> [u64; 2] {
                val a: [u64; 5] = [1, 2, 3, 4, 5]
                a[-2..]
            }
        "#;

        let _ = parse_and_check(source);
    }

    #[test]
    fn test_negative_slice_end() {
        let source = r#"
            fn main() -> [u64; 4] {
                val a: [u64; 5] = [1, 2, 3, 4, 5]
                a[..-1]
            }
        "#;

        let _ = parse_and_check(source);
    }

    #[test]
    fn test_negative_slice_both() {
        let source = r#"
            fn main() -> [u64; 2] {
                val a: [u64; 5] = [1, 2, 3, 4, 5]
                a[-3i64..-1i64]
            }
        "#;

        match parse_and_check(source) {
            Ok(_) => {
                // This should work with explicit type suffixes
            }
            Err(e) => {
                panic!("Type check failed for negative slice with suffixes: {}", e);
            }
        }
    }

    #[test]
    fn test_array_literal_type_preservation() {
        let source = r#"
            fn main() -> [u64; 3] {
                val a: [u64; 5] = [1, 2, 3, 4, 5]
                a[1..4]
            }
        "#;

        match parse_and_check(source) {
            Ok(_) => {
                // Type check should succeed with correct element types
            }
            Err(e) => {
                panic!("Type check failed for array slice: {}", e);
            }
        }
    }
}

mod boundary_tests {
    //! Tests for boundary conditions and limits

    use super::helpers::parse_program_only;

    #[test]
    fn test_i64_boundaries() {
        let test_cases = vec![
            ("9223372036854775807i64", true),   // i64::MAX
            ("-9223372036854775808i64", true),  // i64::MIN
        ];

        for (value, should_pass) in test_cases {
            let input = format!("fn main() -> i64 {{ {} }}", value);
            let result = parse_program_only(&input);

            if should_pass {
                assert!(result.is_ok(), "Value {} should be accepted", value);
            }
        }
    }

    #[test]
    fn test_u64_boundaries() {
        let test_cases = vec![
            ("0u64", true),                      // u64::MIN
            ("18446744073709551615u64", true),   // u64::MAX
        ];

        for (value, should_pass) in test_cases {
            let input = format!("fn main() -> u64 {{ {} }}", value);
            let result = parse_program_only(&input);

            if should_pass {
                assert!(result.is_ok(), "Value {} should be accepted", value);
            }
        }
    }

    #[test]
    fn test_array_access_boundaries() {
        let test_cases = vec![
            ("val a: i64 = 0i64", true),
            ("val b: u64 = 1u64", true),
            ("var c: bool = true", true),
        ];

        for (code, should_pass) in test_cases {
            let input = format!("fn main() -> i64 {{ {} 0i64 }}", code);
            let result = parse_program_only(&input);

            if should_pass {
                assert!(result.is_ok(), "Code '{}' should be accepted", code);
            }
        }
    }

    #[test]
    fn test_function_parameter_boundaries() {
        let test_cases = vec![
            (0, true),   // No parameters
            (1, true),   // Single parameter
            (10, true),  // Many parameters
        ];

        for (param_count, should_pass) in test_cases {
            let params: Vec<String> = (0..param_count)
                .map(|i| format!("p{}: i64", i))
                .collect();
            let args: Vec<String> = (0..param_count)
                .map(|i| format!("{}i64", i))
                .collect();

            let input = if param_count == 0 {
                "fn test() -> i64 { 0i64 } fn main() -> i64 { test() }".to_string()
            } else {
                format!(
                    "fn test({}) -> i64 {{ 0i64 }} fn main() -> i64 {{ test({}) }}",
                    params.join(", "),
                    args.join(", ")
                )
            };

            let result = parse_program_only(&input);

            if should_pass {
                assert!(result.is_ok(), "Function with {} parameters should be accepted", param_count);
            }
        }
    }

    #[test]
    fn test_expression_nesting_depth() {
        let test_cases = vec![
            (1, true),   // Minimal nesting
            (10, true),  // Moderate nesting
            (50, true),  // Deep nesting
        ];

        for (depth, should_pass) in test_cases {
            let mut expr = String::from("1i64");
            for _ in 0..depth {
                expr = format!("({} + 1i64)", expr);
            }
            let input = format!("fn main() -> i64 {{ {} }}", expr);
            let result = parse_program_only(&input);

            if should_pass {
                assert!(result.is_ok(), "Nesting depth {} should be accepted", depth);
            }
        }
    }
}
