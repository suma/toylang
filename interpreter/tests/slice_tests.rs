mod common;

#[cfg(test)]
mod slice_tests {
    use super::*;

    // Tests with explicit u64 suffix
    #[test]
    fn test_slice_basic_range_u64() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
            a[1u64..3u64]
        }
        ", vec![2, 3]);
    }

    #[test]
    fn test_slice_from_start_u64() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 3] {
            val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
            a[..3u64]
        }
        ", vec![1, 2, 3]);
    }

    #[test]
    fn test_slice_to_end_u64() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 3] {
            val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
            a[2u64..]
        }
        ", vec![3, 4, 5]);
    }

    // Tests without u64 suffix (using type inference)
    #[test]
    fn test_slice_basic_range_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[1..3]
        }
        ", vec![2, 3]);
    }

    #[test]
    fn test_slice_from_start_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 3] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[..3]
        }
        ", vec![1, 2, 3]);
    }

    #[test]
    fn test_slice_to_end_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 3] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[2..]
        }
        ", vec![3, 4, 5]);
    }

    #[test]
    fn test_slice_entire_array() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 5] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[..]
        }
        ", vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_slice_empty_range_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 0] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[2..2]
        }
        ", vec![]);
    }

    #[test]
    fn test_slice_single_element_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 1] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[2..3]
        }
        ", vec![3]);
    }

    #[test]
    fn test_slice_sum_elements_mixed() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            val slice: [u64; 3] = a[1..4]  # No suffix for indices
            slice[0] + slice[1] + slice[2]  # No suffix for access
        }
        ", 9); // 2 + 3 + 4 = 9
    }

    #[test]
    fn test_slice_assignment_to_variable_no_suffix() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 5] = [10, 20, 30, 40, 50]
            val b: [u64; 2] = a[1..3]
            b[0] + b[1]
        }
        ", 50); // 20 + 30 = 50
    }

    #[test]
    fn test_slice_nested_no_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 3] {
            val a: [u64; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            val b: [u64; 6] = a[2..8]  # [3, 4, 5, 6, 7, 8]
            b[1..4]  # [4, 5, 6]
        }
        ", vec![4, 5, 6]);
    }

    #[test]
    fn test_slice_mixed_suffix() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
            a[1..3u64]  # Mixed: no suffix and u64 suffix
        }
        ", vec![2, 3]);
    }

    // Error cases
    #[test]
    fn test_slice_out_of_bounds_start() {
        common::assert_program_fails(r"
        fn main() -> [u64; 2] {
            val a: [u64; 3] = [1, 2, 3]
            a[5..6]
        }
        ");
    }

    #[test]
    fn test_slice_out_of_bounds_end() {
        common::assert_program_fails(r"
        fn main() -> [u64; 9] {
            val a: [u64; 3] = [1, 2, 3]
            a[1..10]
        }
        ");
    }

    #[test]
    fn test_slice_invalid_range() {
        common::assert_program_fails(r"
        fn main() -> [u64; 0] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[3..1]  # start > end
        }
        ");
    }

    #[test]
    fn test_slice_with_i64() {
        common::assert_program_result_array_i64(r"
        fn main() -> [i64; 3] {
            val a: [i64; 5] = [-10i64, -5i64, 0i64, 5i64, 10i64]
            a[1..4]
        }
        ", vec![-5, 0, 5]);
    }

    // Negative indexing tests
    #[test]
    fn test_negative_index_access() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-1i64]  # Last element
        }
        ", 5);
    }

    #[test]
    fn test_negative_index_second_last() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 5] = [10, 20, 30, 40, 50]
            a[-2i64]  # Second to last element
        }
        ", 40);
    }

    #[test]
    fn test_slice_negative_start() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-2i64..]  # Last two elements
        }
        ", vec![4, 5]);
    }

    #[test]
    fn test_slice_negative_end() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 4] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[..-1i64]  # All except last element
        }
        ", vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_slice_negative_both() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-3i64..-1i64]  # From 3rd last to last (exclusive)
        }
        ", vec![3, 4]);
    }

    #[test]
    fn test_slice_mixed_positive_negative() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[1..-1i64]  # From 1st to last (exclusive)
        }
        ", vec![2, 3, 4]);
    }

    #[test]
    fn test_negative_index_assignment() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var a: [u64; 3] = [1, 2, 3]
            a[-1i64] = 99  # Set last element
            a[-1i64]       # Get last element
        }
        ", 99);
    }

    #[test]
    fn test_negative_index_inference() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-1]  # Should infer as i64
        }
        ", 5);
    }

    #[test]
    fn test_slice_negative_inference() {
        common::assert_program_result_array_u64(r"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-2..]  # Should infer as i64
        }
        ", vec![4, 5]);
    }

    // Error cases for negative indexing
    #[test]
    fn test_negative_index_out_of_bounds() {
        common::assert_program_fails(r"
        fn main() -> u64 {
            val a: [u64; 3] = [1, 2, 3]
            a[-5i64]  # More negative than array length
        }
        ");
    }

    #[test]
    fn test_slice_negative_out_of_bounds() {
        common::assert_program_fails(r"
        fn main() -> [u64; 0] {
            val a: [u64; 3] = [1, 2, 3]
            a[-5i64..]  # Start too negative
        }
        ");
    }
}