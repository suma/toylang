mod common;
use common::test_program;

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