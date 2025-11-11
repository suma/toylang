//! Advanced Language Features Integration Tests
//!
//! This module contains integration tests for advanced language features:
//! hexadecimal literals, bitwise operations, and multiline comments.
//! Tests cover numeric literals, bitwise operators, and documentation/comments.
//!
//! Test Categories:
//! - Hexadecimal literal parsing and evaluation
//! - Bitwise operations (AND, OR, XOR, NOT, shifts)
//! - Multiline comment syntax and nesting

mod common;

use common::test_program;

// Helper function for testing expressions
fn test_expr(input: &str, expected: &str) {
    let result = common::test_program(input);
    assert!(result.is_ok(), "Execution error: {:?}", result.err());
    let obj = result.unwrap();

    let actual = format!("{:?}", *obj.borrow());
    assert_eq!(actual, expected, "Input: {}", input);
}

#[cfg(test)]
mod bitwise_tests {
    use super::*;
    use crate::common;

    #[test]
    fn test_bitwise_and_uint64() {
        test_expr(
            "fn main() -> u64 { 15u64 & 7u64 }",
            "UInt64(7)"
        );
    }

    #[test]
    fn test_bitwise_and_int64() {
        test_expr(
            "fn main() -> i64 { 15i64 & 7i64 }",
            "Int64(7)"
        );
    }

    #[test]
    fn test_bitwise_or_uint64() {
        test_expr(
            "fn main() -> u64 { 8u64 | 4u64 }",
            "UInt64(12)"
        );
    }

    #[test]
    fn test_bitwise_or_int64() {
        test_expr(
            "fn main() -> i64 { 8i64 | 4i64 }",
            "Int64(12)"
        );
    }

    #[test]
    fn test_bitwise_xor_uint64() {
        test_expr(
            "fn main() -> u64 { 15u64 ^ 10u64 }",
            "UInt64(5)"
        );
    }

    #[test]
    fn test_bitwise_xor_int64() {
        test_expr(
            "fn main() -> i64 { 15i64 ^ 10i64 }",
            "Int64(5)"
        );
    }

    #[test]
    fn test_bitwise_not_uint64() {
        test_expr(
            "fn main() -> u64 { ~0u64 }",
            "UInt64(18446744073709551615)"  // 2^64 - 1
        );
    }

    #[test]
    fn test_bitwise_not_int64() {
        test_expr(
            "fn main() -> i64 { ~0i64 }",
            "Int64(-1)"
        );
    }

    #[test]
    fn test_left_shift_uint64() {
        test_expr(
            "fn main() -> u64 { 1u64 << 4u64 }",
            "UInt64(16)"
        );
    }

    #[test]
    fn test_left_shift_int64() {
        test_expr(
            "fn main() -> i64 { 1i64 << 4u64 }",
            "Int64(16)"
        );
    }

    #[test]
    fn test_right_shift_uint64() {
        test_expr(
            "fn main() -> u64 { 16u64 >> 2u64 }",
            "UInt64(4)"
        );
    }

    #[test]
    fn test_right_shift_int64() {
        test_expr(
            "fn main() -> i64 { 16i64 >> 2u64 }",
            "Int64(4)"
        );
    }

    #[test]
    fn test_complex_bitwise_expression() {
        test_expr(
            "fn main() -> u64 { (255u64 & 15u64) | (16u64 << 1u64) }",
            "UInt64(47)"  // (255 & 15) | (16 << 1) = 15 | 32 = 47
        );
    }

    #[test]
    fn test_bitwise_with_variables() {
        test_expr(
            r#"fn main() -> u64 {
        val a = 18u64
        val b = 52u64
        a ^ b
    }"#,
            "UInt64(38)"  // 18 ^ 52 = 38
        );
    }

    #[test]
    fn test_shift_with_large_values() {
        test_expr(
            "fn main() -> u64 { 1u64 << 63u64 }",
            "UInt64(9223372036854775808)"  // 2^63
        );
    }

    #[test]
    fn test_logical_not_bool() {
        test_expr(
            "fn main() -> bool { !true }",
            "Bool(false)"
        );
    }

    #[test]
    fn test_logical_not_bool_false() {
        test_expr(
            "fn main() -> bool { !false }",
            "Bool(true)"
        );
    }

}

#[cfg(test)]
mod hex_literal_tests {
    use super::*;
    use crate::common;

    #[test]
    fn test_hex_literal_u64() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = 0xFFu64
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 255);
    }

    #[test]
    fn test_hex_literal_i64() {
        let program = r#"
    fn main() -> i64 {
        val x: i64 = 0x7Fi64
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_int64(), 127);
    }

    #[test]
    fn test_hex_literal_auto_type() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = 0x100
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 256);
    }

    #[test]
    fn test_hex_literal_arithmetic() {
        let program = r#"
    fn main() -> u64 {
        val a: u64 = 0xF0
        val b: u64 = 0x0F
        a + b
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 255);
    }

    #[test]
    fn test_hex_literal_comparison() {
        let program = r#"
    fn main() -> bool {
        val x: u64 = 0xFF
        x == 255u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_hex_literal_uppercase() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = 0XABC
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 2748);
    }

    #[test]
    fn test_hex_literal_mixed_case() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = 0xAbCdEf
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 11259375);
    }

    #[test]
    fn test_hex_literal_bitwise_operations() {
        let program = r#"
    fn main() -> u64 {
        val a: u64 = 0xFF00
        val b: u64 = 0x00FF
        val and_result = a & b
        val or_result = a | b
        val xor_result = a ^ b
        if and_result == 0u64 && or_result == 0xFFFFu64 && xor_result == 0xFFFFu64 {
            1u64
        } else {
            0u64
        }
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 1);
    }

    #[test]
    fn test_hex_literal_array_index() {
        let program = r#"
    fn main() -> i64 {
        val arr: [i64; 5] = [10i64, 20i64, 30i64, 40i64, 50i64]
        arr[0x02u64]
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_int64(), 30);
    }

    #[test]
    fn test_hex_literal_in_loop() {
        let program = r#"
    fn main() -> u64 {
        var sum: u64 = 0u64
        for i in 0u64 to 0x10u64 {
            sum = sum + i
        }
        sum
    }
        "#;

        let result = test_program(program).unwrap();
        // Sum from 0 to 15 = 15 * 16 / 2 = 120
        assert_eq!(result.borrow().unwrap_uint64(), 120);
    }

    #[test]
    fn test_hex_literal_negative_i64() {
        let program = r#"
    fn main() -> i64 {
        val x: i64 = -0x10i64
        x
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_int64(), -16);
    }

    #[test]
    fn test_hex_literal_max_values() {
        let program = r#"
    fn main() -> bool {
        val max_u64: u64 = 0xFFFFFFFFFFFFFFFFu64
        val expected: u64 = 18446744073709551615u64
        max_u64 == expected
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_bool(), true);
    }

}

#[cfg(test)]
mod multiline_comment_tests {
    use super::*;
    use crate::common;

    #[test]
    fn test_basic_multiline_comment() {
        let program = r#"
    /* This is a simple multi-line comment */
    fn main() -> u64 {
        42u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_multiline_comment_with_content() {
        let program = r#"
    /*
     * This is a multi-line comment
     * with multiple lines
     * and some formatting
     */
    fn main() -> u64 {
        val x: u64 = 10u64
        x * 5u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 50);
    }

    #[test]
    fn test_inline_multiline_comment() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = /* inline comment */ 25u64
        x + 17u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_multiline_comment_between_statements() {
        let program = r#"
    fn main() -> u64 {
        val x: u64 = 10u64
        /*
        This comment is between statements
        It can span multiple lines
        */
        val y: u64 = 20u64
        x + y
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 30);
    }

    #[test]
    fn test_multiline_comment_with_special_chars() {
        let program = r#"
    /*
     * Comment with special characters: @#$%^&*()
     * Numbers: 123456789
     * Symbols: +-=<>{}[]
     */
    fn main() -> u64 {
        100u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 100);
    }

    #[test]
    fn test_multiline_comment_with_asterisks() {
        let program = r#"
    /*
     * This comment has asterisks: * * * *
     * But they don't end the comment
     * Only */ ends it
     */
    fn main() -> u64 {
        val result: u64 = 7u64 * 6u64
        result
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_mixed_comment_types() {
        let program = r#"
    /* Multi-line comment at the top */
    fn main() -> u64 {
        # Single line comment
        val a: u64 = 5u64
        /* Another multi-line comment */
        val b: u64 = 8u64
        # Final single line comment
        a + b
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 13);
    }

    #[test]
    fn test_multiline_comment_in_function_body() {
        let program = r#"
    fn calculate(x: u64, y: u64) -> u64 {
        /*
         * This function multiplies two numbers
         * and then adds a constant
         */
        val product: u64 = x * y
        val constant: u64 = 10u64
        product + constant
    }

    fn main() -> u64 {
        calculate(3u64, 4u64)
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 22);
    }

    #[test]
    fn test_multiline_comment_with_newlines() {
        let program = r#"
    /*

    This comment has empty lines


    in between

    */
    fn main() -> u64 {
        42u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_multiline_comment_minimal() {
        let program = r#"
    /**/
    fn main() -> u64 {
        123u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 123);
    }

    #[test]
    fn test_multiline_comment_with_single_char() {
        let program = r#"
    /*x*/
    fn main() -> u64 {
        999u64
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 999);
    }

    #[test]
    fn test_complex_expression_with_comments() {
        let program = r#"
    fn main() -> u64 {
        val a: u64 = /* first value */ 10u64
        val b: u64 = /* second value */ 20u64
        /*
         * Calculate the result using
         * a complex expression
         */
        val result: u64 = (a + b) * /* multiplier */ 2u64
        result
    }
        "#;

        let result = test_program(program).unwrap();
        assert_eq!(result.borrow().unwrap_uint64(), 60);
    }

}

