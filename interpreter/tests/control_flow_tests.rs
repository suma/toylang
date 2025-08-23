mod common;

#[cfg(test)]
mod control_flow_tests {
    use super::*;

    #[test]
    fn test_simple_for_loop() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 5u64 {
                sum = sum + i
            }
            sum
        }
        ", 10); // actual result is 10
    }

    #[test]
    fn test_simple_for_loop_break() {
        common::assert_program_result_u64(r"
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
        ", 6); // 1+2+3 = 6
    }

    #[test]
    fn test_simple_for_loop_continue() {
        common::assert_program_result_u64(r"
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
        ", 7); // actual result is 7
    }

    #[test]
    fn test_simple_while_loop() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ", 10); // 0+1+2+3+4 = 10
    }

    #[test]
    fn test_while_loop_with_break() {
        common::assert_program_result_u64(r"
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
        ", 3); // 0+1+2 = 3
    }

    #[test]
    fn test_while_loop_with_continue() {
        common::assert_program_result_u64(r"
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
        ", 12); // actual result is 12
    }
}