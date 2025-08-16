mod common;
use common::test_program;

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_simple_program() {
        let mut parser = frontend::ParserWithInterner::new(r"
        fn main() -> u64 {
            val a = 1u64
            val b = 2u64
            val c = a + b
            c
        }
        ");
        let program = parser.parse_program();
        assert!(program.is_ok());

        let program = program.unwrap();
        let string_interner = parser.get_string_interner();

        let res = interpreter::execute_program(&program, string_interner, Some("fn main() -> u64 { 1u64 + 2u64 }"), Some("test.t"));
        assert!(res.is_ok());
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    #[test]
    fn test_i64_basic() {
        let res = test_program(r"
        fn main() -> i64 {
            val a: i64 = 42i64
            val b: i64 = -10i64
            a + b
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 32);
    }

    #[test]
    fn test_simple_if_then_else_1() {
        let res = test_program(r"
        fn main() -> u64 {
            if true {
                1u64
            } else {
                2u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1u64);
    }

    #[test]
    fn test_simple_if_then_else_2() {
        let res = test_program(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                2u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 2u64);
    }
}