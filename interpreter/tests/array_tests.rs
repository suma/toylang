mod common;

#[cfg(test)]
mod array_tests {
    use super::*;

    #[test]
    fn test_array_basic_operations() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 3] = [1u64, 2u64, 3u64]
            a[0u64] + a[1u64] + a[2u64]
        }
        ", 6);
    }

    #[test]
    fn test_array_assignment() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var a: [u64; 3] = [1u64, 2u64, 3u64]
            a[1u64] = 10u64
            a[0u64] + a[1u64] + a[2u64]
        }
        ", 14);
    }

    #[test]
    fn test_empty_array_error() {
        common::assert_program_fails(r"
        fn main() -> u64 {
            val a: [u64; 0] = []
            42u64
        }
        "); // Empty arrays are not supported
    }

    #[test]
    fn test_array_single_element() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 1] = [100u64]
            a[0u64]
        }
        ", 100);
    }

    #[test]
    fn test_array_index_out_of_bounds() {
        common::assert_program_fails(r"
        fn main() -> u64 {
            val a: [u64; 2] = [1u64, 2u64]
            a[5u64]
        }
        "); // Should return error for out of bounds access
    }
}