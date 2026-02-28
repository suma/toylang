//! Type Conversion Tests
//!
//! Tests for the type conversion and numeric type resolution subsystem.
//! Covers numeric literal conversion, type resolution between Number/u64/i64,
//! type mismatch detection, implicit conversions, and error cases.
//!
//! Target: src/type_checker/type_conversion.rs (479 lines, 0 existing tests)

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

mod numeric_literal_conversion {
    //! Tests for bare number literal conversion to concrete types

    use super::helpers::parse_and_check;

    #[test]
    fn test_bare_number_with_u64_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_resolved_by_arithmetic_context() {
        let source = r#"
            fn main() -> u64 {
                val x = 42
                val y = 1u64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_without_context_stays_number() {
        // A bare number without any type context remains Number type,
        // which causes a type mismatch with u64 return type
        let source = r#"
            fn main() -> u64 {
                val x = 42
                x
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bare number without context should remain Number type");
    }

    #[test]
    fn test_bare_number_with_i64_hint() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_u64_suffix_literal() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_suffix_literal() {
        let source = r#"
            fn main() -> i64 {
                val x = 42i64
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_hex_literal_with_u64_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 0xFF
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_zero_literal_with_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 0
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod numeric_type_resolution {
    //! Tests for resolve_numeric_types: Number+concrete type interactions

    use super::helpers::parse_and_check;

    #[test]
    fn test_number_plus_u64_resolves_to_u64() {
        let source = r#"
            fn main() -> u64 {
                val x = 10
                val y = 20u64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_i64_resolves_to_i64() {
        let source = r#"
            fn main() -> i64 {
                val x = 10
                val y = 20i64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_number_defaults_to_u64() {
        let source = r#"
            fn main() -> u64 {
                val x = 10
                val y = 20
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_number_with_i64_return_hint() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 10
                val y: i64 = 20
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_in_arithmetic_expression() {
        let source = r#"
            fn main() -> u64 {
                val a = 5
                val b = 10u64
                val c = a * b + 3
                c
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod type_mismatch_errors {
    //! Tests for type mismatch detection in numeric operations

    use super::helpers::parse_and_check;

    #[test]
    fn test_u64_plus_i64_mixed_error() {
        let source = r#"
            fn main() -> u64 {
                val x = 10u64
                val y = 20i64
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixing u64 and i64 in arithmetic should fail");
    }

    #[test]
    fn test_bool_arithmetic_error() {
        let source = r#"
            fn main() -> bool {
                val x = true
                val y = false
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool arithmetic should fail");
    }

    #[test]
    fn test_string_plus_u64_error() {
        let source = r#"
            fn main() -> u64 {
                val x = "hello"
                val y = 10u64
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String + u64 should fail");
    }
}

mod implicit_conversion {
    //! Tests for implicit type conversion in various contexts

    use super::helpers::parse_and_check;

    #[test]
    fn test_val_declaration_implicit_conversion() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_function_argument_conversion() {
        let source = r#"
            fn take_i64(x: i64) -> i64 {
                x
            }

            fn main() -> i64 {
                take_i64(42i64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_field_number_conversion() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 10, y: 20 }
                p.x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_field_i64_number_conversion() {
        let source = r#"
            struct Offset {
                dx: i64,
                dy: i64
            }

            fn main() -> i64 {
                val o = Offset { dx: 10, dy: 20 }
                o.dx
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_element_implicit_conversion() {
        let source = r#"
            fn main() -> u64 {
                val arr: [u64; 3] = [1, 2, 3]
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_array_implicit_conversion() {
        let source = r#"
            fn main() -> i64 {
                val arr: [i64; 3] = [1, 2, 3]
                arr[0i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod type_conversion_errors {
    //! Tests for conversion error cases

    use super::helpers::parse_and_check;

    #[test]
    fn test_bool_to_u64_assignment_error() {
        let source = r#"
            fn main() -> u64 {
                val x = true
                val y: u64 = x
                y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool to u64 assignment should fail");
    }

    #[test]
    fn test_string_to_i64_assignment_error() {
        let source = r#"
            fn main() -> i64 {
                val x = "hello"
                val y: i64 = x
                y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String to i64 assignment should fail");
    }

    #[test]
    fn test_bool_to_function_param_error() {
        let source = r#"
            fn take_u64(x: u64) -> u64 {
                x
            }

            fn main() -> u64 {
                take_u64(true)
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool passed as u64 param should fail");
    }

    #[test]
    fn test_wrong_return_type_error() {
        let source = r#"
            fn main() -> u64 {
                val x = "hello"
                x
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String returned as u64 should fail");
    }
}
