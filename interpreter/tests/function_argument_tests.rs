mod common;
use common::test_program;

#[cfg(test)]
mod function_argument_tests {
    use super::*;

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