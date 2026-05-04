mod common;
use common::test_program;

// ============================================================================
// Bitwise operation tests
// ============================================================================

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

// ============================================================================
// Hex literal tests
// ============================================================================

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

// ============================================================================
// Multiline comment tests
// ============================================================================

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

#[test]
fn test_drop_trait_auto_called_at_scope_exit() {
    // Phase 5 (汎用 RAII): a user struct that impls `Drop` gets
    // its `drop(&mut self)` called automatically when the
    // binding goes out of scope. Verifies LIFO order across
    // multiple bindings (last-bound drops first), and that the
    // drop fires on linear function exit.
    //
    // Mechanism: each `Marker::drop` mutates a shared cell via
    // a heap pointer, encoding the order of drops as a base-10
    // sequence. After both bindings drop, the sequence is
    // `b.id then a.id` = `21` (i.e. b dropped first, then a).
    //
    // AOT auto-drop is a future phase — this test is interpreter-
    // only on purpose.
    let program = r#"
struct Marker { id: u64, log: ptr }
impl Drop for Marker {
    fn drop(&mut self) {
        val cur: u64 = __builtin_ptr_read(self.log, 0u64)
        __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
    }
}
fn run(log: ptr) {
    val a = Marker { id: 1u64, log: log }
    val b = Marker { id: 2u64, log: log }
}
fn main() -> u64 {
    val log: ptr = __builtin_heap_alloc(8u64)
    __builtin_ptr_write(log, 0u64, 0u64)
    run(log)
    val recorded: u64 = __builtin_ptr_read(log, 0u64)
    recorded
}
    "#;

    let result = test_program(program).unwrap();
    assert_eq!(result.borrow().unwrap_uint64(), 21);
}

#[test]
fn test_drop_trait_fires_on_early_return() {
    // Phase 5 (汎用 RAII): early `return` from inside the
    // function body still triggers auto-drop of locals
    // declared before the return. Order: b dropped before a.
    let program = r#"
struct Marker { id: u64, log: ptr }
impl Drop for Marker {
    fn drop(&mut self) {
        val cur: u64 = __builtin_ptr_read(self.log, 0u64)
        __builtin_ptr_write(self.log, 0u64, cur * 10u64 + self.id)
    }
}
fn run(log: ptr) -> u64 {
    val a = Marker { id: 3u64, log: log }
    val b = Marker { id: 4u64, log: log }
    return 7u64
}
fn main() -> u64 {
    val log: ptr = __builtin_heap_alloc(8u64)
    __builtin_ptr_write(log, 0u64, 0u64)
    val r: u64 = run(log)
    val recorded: u64 = __builtin_ptr_read(log, 0u64)
    if r != 7u64 { return 1u64 }
    recorded
}
    "#;

    let result = test_program(program).unwrap();
    assert_eq!(result.borrow().unwrap_uint64(), 43);
}
