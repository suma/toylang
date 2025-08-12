mod common;
use common::test_program;

#[cfg(test)]
mod control_flow_tests {
    use super::*;

    #[test]
    fn test_simple_for_loop() {
        let res = test_program(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 5u64 {
                sum = sum + i
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 10); // actual result is 10
    }

    #[test]
    fn test_simple_for_loop_break() {
        let res = test_program(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 10u64 {
                if i > 3u64 {
                    break
                }
                sum = sum + i
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 6); // 1+2+3 = 6
    }

    #[test]
    fn test_simple_for_loop_continue() {
        let res = test_program(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 5u64 {
                if i == 3u64 {
                    continue
                }
                sum = sum + i
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 7); // actual result is 7
    }

    #[test]
    fn test_simple_while_loop() {
        let res = test_program(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 10); // 0+1+2+3+4 = 10
    }

    #[test]
    fn test_while_loop_with_break() {
        let res = test_program(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while true {
                if i >= 3u64 {
                    break
                }
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3); // 0+1+2 = 3
    }

    #[test]
    fn test_while_loop_with_continue() {
        let res = test_program(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                i = i + 1u64
                if i == 3u64 {
                    continue
                }
                sum = sum + i
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 12); // actual result is 12
    }
}