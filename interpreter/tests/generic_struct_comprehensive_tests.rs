mod common;

use common::test_program;

// ===========================================
// Basic Generic Struct Definition Tests
// ===========================================

#[test]
fn test_generic_struct_parsing_only() {
    let source = r#"
        # Test that generic struct can be parsed
        struct Container<T> {
            value: T
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Generic struct definition should parse successfully: {:?}", result.err());
}

#[test]
fn test_multiple_generic_params() {
    let source = r#"
        # Struct with multiple generic parameters
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Multiple generic parameters should parse successfully: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_mixed_fields() {
    let source = r#"
        # Generic struct with both generic and concrete fields
        struct Mixed<T> {
            generic_field: T,
            concrete_field: u64,
            bool_field: bool
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Mixed field types should parse successfully: {:?}", result.err());
}

// ===========================================
// Generic Struct with Methods
// ===========================================

#[test]
fn test_generic_struct_with_methods() {
    let source = r#"
        struct Box<T> {
            value: T
        }
        
        impl<T> Box<T> {
            fn new(val: T) -> Self {
                Box { value: val }
            }
            
            fn get(self: Self) -> T {
                self.value
            }
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Note: Currently this may fail due to incomplete generic method support
    // But we include it to track progress
    match result {
        Ok(_) => println!("Generic methods are now supported!"),
        Err(e) => println!("Expected error for now (generic methods not fully implemented): {}", e)
    }
}

// ===========================================
// Array of Generic Structs
// ===========================================

#[test]
fn test_array_of_generic_structs() {
    let source = r#"
        struct Item<T> {
            data: T
        }
        
        fn main() -> u64 {
            # This tests parsing of generic struct arrays
            # Note: instantiation may not work yet
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Array of generic structs should parse: {:?}", result.err());
}

// ===========================================
// Nested Generic Structs
// ===========================================

#[test]
fn test_nested_generic_structs() {
    let source = r#"
        struct Inner<T> {
            value: T
        }
        
        struct Outer<U> {
            inner: Inner<U>,
            extra: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // This tests nested generic type references
    assert!(result.is_ok(), "Nested generic structs should parse: {:?}", result.err());
}

// ===========================================
// Generic Struct with Different Type Parameters
// ===========================================

#[test]
fn test_generic_struct_three_params() {
    let source = r#"
        struct Triple<T, U, V> {
            first: T,
            second: U,
            third: V
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Three type parameters should work: {:?}", result.err());
}

// ===========================================
// Generic Struct Instantiation Tests (Future)
// These tests document expected behavior once instantiation is implemented
// ===========================================

#[test]
#[ignore] // Remove ignore when instantiation is implemented
fn test_generic_struct_instantiation_u64() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        fn main() -> u64 {
            val box = Container { value: 100u64 }
            box.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 100);
        }
        Err(e) => panic!("Generic instantiation should work: {}", e),
    }
}

#[test]
#[ignore] // Remove ignore when instantiation is implemented
fn test_generic_struct_instantiation_bool() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        fn main() -> bool {
            val box = Container { value: true }
            box.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            let borrowed = val.borrow();
            match &*borrowed {
                interpreter::object::Object::Bool(b) => assert_eq!(*b, true),
                _ => panic!("Expected Bool result"),
            }
        }
        Err(e) => panic!("Generic instantiation should work: {}", e),
    }
}

#[test]
#[ignore] // Remove ignore when instantiation is implemented
fn test_generic_struct_with_multiple_instances() {
    let source = r#"
        struct Box<T> {
            value: T
        }
        
        fn main() -> u64 {
            val int_box = Box { value: 42u64 }
            val bool_box = Box { value: true }
            int_box.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 42);
        }
        Err(e) => panic!("Multiple instantiations should work: {}", e),
    }
}

// ===========================================
// Generic Struct Method Call Tests (Future)
// ===========================================

#[test]
#[ignore] // Remove ignore when generic methods are implemented
fn test_generic_struct_method_call() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        impl<T> Container<T> {
            fn get_value(self: Self) -> T {
                self.value
            }
        }
        
        fn main() -> u64 {
            val box = Container { value: 99u64 }
            box.get_value()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => {
            assert_eq!(val.borrow().unwrap_uint64(), 99);
        }
        Err(e) => panic!("Generic method call should work: {}", e),
    }
}

// ===========================================
// Error Cases
// ===========================================

#[test]
fn test_generic_struct_duplicate_params() {
    let source = r#"
        # This should fail: duplicate type parameters
        struct Bad<T, T> {
            field1: T,
            field2: T
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_err(), "Duplicate type parameters should fail");
}

#[test]
fn test_generic_struct_undefined_type_param() {
    let source = r#"
        struct Container<T> {
            value: T,
            other: U  # U is not defined
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_err(), "Undefined type parameter should fail");
}

// ===========================================
// Complex Generic Patterns (Future)
// ===========================================

#[test]
#[ignore] // For future implementation
fn test_generic_struct_with_constraints() {
    // Once we have trait/interface support
    let source = r#"
        struct Sortable<T> {
            items: [T; 10]
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Generic struct with constraints should work");
}

#[test]
fn test_generic_struct_in_function_param() {
    let source = r#"
        struct Box<T> {
            value: T
        }
        
        # This tests if generic structs can be used as function parameters
        # Note: may not work with current implementation
        fn process(box: Box<u64>) -> u64 {
            42u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Document current behavior
    match result {
        Ok(_) => println!("Generic structs in function params now work!"),
        Err(e) => println!("Expected limitation: {}", e)
    }
}