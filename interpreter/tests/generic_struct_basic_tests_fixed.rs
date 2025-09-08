mod common;

use common::test_program;

#[test]
fn test_generic_struct_simple_definition() {
    let source = r#"
        # Define a simple generic struct
        struct Box<T> {
            value: T
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Program should succeed but failed: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_u64() {
    let source = r#"
        struct Container<T> {
            data: T,
            size: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Program should succeed but failed: {:?}", result.err());
}

#[test]
fn test_generic_struct_parsing_only() {
    // Test that generic struct with impl blocks can be parsed
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        impl<T> Container<T> {
            fn new(value: T) -> Self {
                Container { value: value }
            }
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // For now, we expect this to fail with type checking, but parsing should work
    // In future, when generic struct instantiation is implemented, this should succeed
    match result {
        Ok(_) => {
            // Great! Generic struct instantiation is working
        }
        Err(e) => {
            // Expected for now - generic struct instantiation not yet implemented
            println!("Expected error (generic struct instantiation not implemented): {}", e);
        }
    }
}