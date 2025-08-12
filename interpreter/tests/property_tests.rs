mod common;
use common::test_program;

#[cfg(test)]
mod property_tests {
    use super::*;
    
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