mod common;

fn test_expr(input: &str, expected: &str) {
    let result = common::test_program(input);
    assert!(result.is_ok(), "Execution error: {:?}", result.err());
    let obj = result.unwrap();
    
    let actual = format!("{:?}", *obj.borrow());
    assert_eq!(actual, expected, "Input: {}", input);
}

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