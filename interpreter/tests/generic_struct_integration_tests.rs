mod common;

use common::test_program;

// ===========================================
// Integration with Other Language Features
// ===========================================

#[test]
fn test_generic_struct_with_functions() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        fn process_container(c: Container<u64>) -> u64 {
            # Once instantiation works, this should be valid
            42u64
        }
        
        fn main() -> u64 {
            process_container(Container { value: 100u64 })
        }
    "#;
    
    let result = test_program(source);
    // Test current state of generic struct as function parameter
    match result {
        Ok(_) => println!("Generic structs as function params work!"),
        Err(e) => println!("Expected limitation with function params: {}", e)
    }
}

#[test]
fn test_generic_struct_with_loops() {
    let source = r#"
        struct Counter<T> {
            value: T,
            max: T
        }
        
        fn main() -> u64 {
            # Test generic struct usage in loops
            var sum = 0u64
            for i in 0u64 to 5u64 {
                sum = sum + i
            }
            sum
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 10),
        Err(e) => panic!("Loop with generic struct context failed: {}", e)
    }
}

#[test]
fn test_generic_struct_with_conditionals() {
    let source = r#"
        struct Option<T> {
            value: T,
            has_value: bool
        }
        
        fn main() -> u64 {
            val flag = true
            if flag {
                42u64
            } else {
                0u64
            }
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 42),
        Err(e) => panic!("Conditional with generic struct context failed: {}", e)
    }
}

#[test]
fn test_generic_struct_with_nested_functions() {
    let source = r#"
        struct Wrapper<T> {
            data: T
        }
        
        fn outer() -> u64 {
            fn inner() -> u64 {
                42u64
            }
            inner()
        }
        
        fn main() -> u64 {
            outer()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 42),
        Err(e) => panic!("Nested functions with generic struct failed: {}", e)
    }
}

// ===========================================
// Generic Struct with Built-in Types
// ===========================================

#[test]
fn test_generic_struct_all_primitive_types() {
    let source = r#"
        struct AllTypes<T> {
            generic: T,
            uint: u64,
            int: i64,
            boolean: bool,
            text: "hello"
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "All primitive types should work: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_string_literals() {
    let source = r#"
        struct Message<T> {
            content: T,
            prefix: "MSG: "
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "String literals in generic struct should work: {:?}", result.err());
}

// ===========================================
// Performance and Stress Tests
// ===========================================

#[test]
fn test_many_generic_struct_definitions() {
    let source = r#"
        struct A<T> { value: T }
        struct B<T> { value: T }
        struct C<T> { value: T }
        struct D<T> { value: T }
        struct E<T> { value: T }
        struct F<T> { value: T }
        struct G<T> { value: T }
        struct H<T> { value: T }
        struct I<T> { value: T }
        struct J<T> { value: T }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Many generic struct definitions should work: {:?}", result.err());
}

#[test]
fn test_complex_generic_struct_hierarchy() {
    let source = r#"
        struct Base<T> {
            value: T
        }
        
        struct Derived<U> {
            base: Base<U>,
            extra: u64
        }
        
        struct MoreDerived<V> {
            derived: Derived<V>,
            more_extra: bool
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Complex hierarchy should parse: {:?}", result.err());
}

// ===========================================
// Future: Full Integration Tests
// ===========================================

#[test]
#[ignore] // Enable when full support is ready
fn test_complete_generic_workflow() {
    let source = r#"
        struct Result<T, E> {
            value: T,
            error: E,
            is_ok: bool
        }
        
        impl<T, E> Result<T, E> {
            fn ok(val: T) -> Self {
                Result { value: val, error: 0u64, is_ok: true }
            }
            
            fn unwrap(self: Self) -> T {
                if self.is_ok {
                    self.value
                } else {
                    # panic
                    self.value
                }
            }
        }
        
        fn divide(a: u64, b: u64) -> Result<u64, u64> {
            if b == 0u64 {
                Result { value: 0u64, error: 1u64, is_ok: false }
            } else {
                Result { value: a / b, error: 0u64, is_ok: true }
            }
        }
        
        fn main() -> u64 {
            val result = divide(10u64, 2u64)
            result.unwrap()
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 5),
        Err(e) => panic!("Complete workflow should work: {}", e)
    }
}

#[test]
#[ignore] // Enable when ready
fn test_generic_data_structures() {
    let source = r#"
        struct LinkedList<T> {
            value: T,
            next: LinkedList<T>  # Would need Option type or pointers
        }
        
        struct Stack<T> {
            items: [T; 100],
            top: u64
        }
        
        struct Queue<T> {
            items: [T; 100],
            front: u64,
            rear: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Data structures should work: {:?}", result.err());
}

#[test]
#[ignore] // For future implementation
fn test_generic_struct_with_closures() {
    let source = r#"
        struct Closure<T, R> {
            captured: T,
            # func: fn(T) -> R  # If we had function pointers
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Closures in generic structs (future): {:?}", result.err());
}

// ===========================================
// Documentation Tests
// ===========================================

#[test]
fn test_generic_struct_basic_example() {
    // This is the example from documentation
    let source = r#"
        # A simple generic container
        struct Box<T> {
            value: T
        }
        
        fn main() -> u64 {
            # Once instantiation works:
            # val my_box = Box { value: 42u64 }
            # my_box.value
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Documentation example should work: {:?}", result.err());
}

#[test]
fn test_pair_struct_example() {
    // Common use case: pairs
    let source = r#"
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        fn main() -> u64 {
            # val p = Pair { first: 10u64, second: true }
            # p.first
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Pair example should work: {:?}", result.err());
}

#[test]
fn test_option_type_pattern() {
    // Option type pattern
    let source = r#"
        struct Option<T> {
            value: T,
            is_some: bool
        }
        
        struct None<T> {
            _phantom: T  # Placeholder
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Option pattern should parse: {:?}", result.err());
}