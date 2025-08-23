mod common;

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_simple_program() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a = 1u64
            val b = 2u64
            val c = a + b
            c
        }
        ", 3);
    }

    #[test]
    fn test_i64_basic() {
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val a: i64 = 42i64
            val b: i64 = -10i64
            a + b
        }
        ", 32);
    }

    #[test]
    fn test_simple_if_then_else_1() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            if true {
                1u64
            } else {
                2u64
            }
        }
        ", 1);
    }

    #[test]
    fn test_simple_if_then_else_2() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                2u64
            }
        }
        ", 2);
    }
}