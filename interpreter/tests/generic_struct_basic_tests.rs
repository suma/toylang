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
            val box = Container { data: 100u64, size: 1u64 }
            box.data
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::UInt64(n) => assert_eq!(*n, 100),
                _ => panic!("Expected UInt64 result, got {:?}", borrowed),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test] 
fn test_generic_struct_with_bool() {
    let source = r#"
        struct Wrapper<T> {
            item: T,
            is_valid: bool
        }
        
        fn main() -> bool {
            val wrapper = Wrapper { item: true, is_valid: true }
            wrapper.item
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::Bool(b) => assert_eq!(*b, true),
                _ => panic!("Expected Bool result, got {:?}", borrowed),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_multiple_type_params() {
    let source = r#"
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        fn main() -> u64 {
            val pair = Pair { first: 42u64, second: true }
            pair.first
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::UInt64(n) => assert_eq!(*n, 42),
                _ => panic!("Expected UInt64 result, got {:?}", borrowed),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}

#[test]
fn test_generic_struct_with_arrays() {
    let source = r#"
        struct ArrayContainer<T> {
            items: [T; 3],
            count: u64
        }
        
        fn main() -> u64 {
            val container = ArrayContainer { 
                items: [1u64, 2u64, 3u64], 
                count: 3u64 
            }
            container.items[0u64]
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::UInt64(n) => assert_eq!(*n, 1),
                _ => panic!("Expected UInt64 result, got {:?}", borrowed),
            }
        }
        Err(e) => panic!("Program failed: {}", e),
    }
}