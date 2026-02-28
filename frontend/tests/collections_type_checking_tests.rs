//! Collections Type Checking Tests
//!
//! Tests for array, dictionary, tuple, cast, and slice type checking.
//! Covers array literal validation, dict consistency, tuple operations,
//! cast restrictions, and slice element access.
//!
//! Target: src/type_checker/collections.rs (803 lines, indirect tests only)

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

mod array_literal_type_checking {
    //! Tests for array literal type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_u64_array_literal() {
        let source = r#"
            fn main() -> u64 {
                val arr = [1u64, 2u64, 3u64]
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_array_literal() {
        let source = r#"
            fn main() -> i64 {
                val arr = [1i64, 2i64, 3i64]
                arr[0i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bool_array_literal() {
        let source = r#"
            fn main() -> bool {
                val arr = [true, false, true]
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_type_hint_inference() {
        let source = r#"
            fn main() -> u64 {
                val arr: [u64; 3] = [1, 2, 3]
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_array_type_hint_inference() {
        let source = r#"
            fn main() -> i64 {
                val arr: [i64; 3] = [1, 2, 3]
                arr[0i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_nested_array() {
        let source = r#"
            fn main() -> [[u64; 2]; 2] {
                val inner1 = [1u64, 2u64]
                val inner2 = [3u64, 4u64]
                [inner1, inner2]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_mixed_i64_u64_error() {
        let source = r#"
            fn main() -> u64 {
                val arr = [1u64, 2i64, 3u64]
                arr[0u64]
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixing i64 and u64 in array should fail");
    }

    #[test]
    fn test_array_mixed_bool_u64_error() {
        let source = r#"
            fn main() -> u64 {
                val arr = [1u64, true, 3u64]
                arr[0u64]
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixing bool and u64 in array should fail");
    }
}

mod dict_type_checking {
    //! Tests for dictionary type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_string_string_dict() {
        let source = r#"
            fn main() -> u64 {
                val d = dict{"name": "Alice", "city": "Tokyo"}
                0u64
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_string_number_dict() {
        let source = r#"
            fn main() -> u64 {
                val d = dict{"one": 1, "two": 2}
                0u64
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_dict_value_type_mismatch_error() {
        let source = r#"
            fn main() -> u64 {
                val d = dict{"str": "value", "num": 42u64}
                0u64
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixed value types in dict should fail");
    }

    #[test]
    fn test_dict_key_type_mismatch_error() {
        let source = r#"
            fn main() -> u64 {
                val d = dict{"str": "value1", 42: "value2"}
                0u64
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixed key types in dict should fail");
    }
}

mod cast_type_checking {
    //! Tests for cast expression type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_i64_to_u64_cast() {
        let source = r#"
            fn main() -> u64 {
                val x = 42i64
                x as u64
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_u64_to_i64_cast() {
        let source = r#"
            fn main() -> i64 {
                val x = 42u64
                x as i64
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_identity_cast_u64() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                x as u64
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bool_to_u64_cast_error() {
        let source = r#"
            fn main() -> u64 {
                val x = true
                x as u64
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool to u64 cast should fail");
    }

    #[test]
    fn test_string_to_i64_cast_error() {
        let source = r#"
            fn main() -> i64 {
                val x = "hello"
                x as i64
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String to i64 cast should fail");
    }
}

mod slice_access_type_checking {
    //! Tests for array slice and element access type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_single_element_access() {
        let source = r#"
            fn main() -> u64 {
                val arr = [10u64, 20u64, 30u64]
                arr[1u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_negative_index_access() {
        let source = r#"
            fn main() -> u64 {
                val arr = [10u64, 20u64, 30u64]
                arr[-1i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_array_element_assignment() {
        let source = r#"
            fn main() -> u64 {
                var arr = [0u64, 0u64, 0u64]
                arr[0u64] = 42u64
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_range_slice_access() {
        let source = r#"
            fn main() -> [u64; 2] {
                val arr = [10u64, 20u64, 30u64, 40u64]
                arr[1i64..3i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_multiple_array_operations() {
        let source = r#"
            fn main() -> u64 {
                var arr = [1u64, 2u64, 3u64, 4u64, 5u64]
                val first = arr[0u64]
                val last = arr[4u64]
                first + last
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}
