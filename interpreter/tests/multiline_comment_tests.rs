mod common;
use common::test_program;

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