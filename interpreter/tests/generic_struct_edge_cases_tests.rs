mod common;

use common::test_program;

// ===========================================
// Edge Cases and Special Scenarios
// ===========================================

#[test]
fn test_empty_generic_params() {
    // Struct with generic declaration but no actual type params
    // This should be rejected by the parser
    let source = r#"
        struct Empty<> {
            value: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_err(), "Empty generic params should fail");
}

#[test]
fn test_generic_struct_with_self_reference() {
    let source = r#"
        struct Node<T> {
            value: T,
            next: Node<T>  # Self-referential generic - should this work?
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Document current behavior - this likely needs special handling
    match result {
        Ok(_) => println!("Self-referential generics are supported"),
        Err(e) => println!("Self-referential generic error (expected): {}", e)
    }
}

#[test]
fn test_generic_struct_shadowing() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        fn test<T>(x: T) -> T {
            # T here shadows the struct's T - is this allowed?
            x
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Type parameter shadowing should be handled: {:?}", result.err());
}

#[test]
fn test_generic_struct_partial_specialization() {
    let source = r#"
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        # Can we have a struct where one param is concrete?
        struct IntPair<T> {
            first: T,
            second: i64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Partial specialization pattern should work: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_array_field() {
    let source = r#"
        struct ArrayContainer<T> {
            items: [T; 5],
            count: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Generic array fields should parse: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_tuple_field() {
    let source = r#"
        struct TupleContainer<T, U> {
            pair: (T, U),
            flag: bool
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Test if tuples with generic types work
    match result {
        Ok(_) => println!("Generic tuples in structs work!"),
        Err(e) => println!("Generic tuple field error (may be expected): {}", e)
    }
}

#[test]
fn test_deeply_nested_generics() {
    let source = r#"
        struct Level1<T> {
            value: T
        }
        
        struct Level2<U> {
            inner: Level1<U>
        }
        
        struct Level3<V> {
            inner: Level2<V>
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Deeply nested generics should parse: {:?}", result.err());
}

#[test]
fn test_generic_struct_name_collision() {
    let source = r#"
        struct T<T> {  # Struct named T with param T
            value: T
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // This is confusing but technically valid in some languages
    match result {
        Ok(_) => println!("Name collision is allowed"),
        Err(e) => println!("Name collision rejected (may be good): {}", e)
    }
}

#[test]
fn test_generic_impl_without_struct() {
    let source = r#"
        # Impl block for non-existent generic struct
        impl<T> NonExistent<T> {
            fn method(self: Self) -> T {
                self.value
            }
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_err(), "Impl for non-existent struct should fail");
}

#[test]
fn test_generic_struct_with_dict_field() {
    let source = r#"
        struct DictContainer<K, V> {
            mapping: {K: V},
            size: u64
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Test if generic dict types work
    match result {
        Ok(_) => println!("Generic dict fields work!"),
        Err(e) => println!("Generic dict field error: {}", e)
    }
}

#[test]
fn test_generic_struct_zero_fields() {
    let source = r#"
        struct Empty<T> {
            # No fields - is this valid?
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Some languages allow empty structs
    match result {
        Ok(_) => println!("Empty generic structs are allowed"),
        Err(e) => println!("Empty generic struct error: {}", e)
    }
}

#[test]
fn test_generic_struct_long_param_list() {
    let source = r#"
        # Test with many type parameters
        struct Many<A, B, C, D, E, F, G, H> {
            a: A,
            b: B,
            c: C,
            d: D,
            e: E,
            f: F,
            g: G,
            h: H
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Many type parameters should work: {:?}", result.err());
}

#[test]
fn test_generic_struct_with_unit_type() {
    let source = r#"
        struct UnitContainer<T> {
            value: T,
            unit_field: ()  # Unit type field
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // Test if unit type is supported in generic structs
    match result {
        Ok(_) => println!("Unit type in generic struct works"),
        Err(e) => println!("Unit type error: {}", e)
    }
}

#[test]
fn test_generic_struct_recursive_type_alias() {
    let source = r#"
        struct Recursive<T> {
            value: T,
            child: Recursive<Recursive<T>>  # Very recursive
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    // This is likely problematic
    match result {
        Ok(_) => println!("Recursive generic nesting allowed (surprising)"),
        Err(e) => println!("Recursive generic error (expected): {}", e)
    }
}

#[test]
fn test_generic_visibility_modifiers() {
    let source = r#"
        pub struct PublicGeneric<T> {
            value: T
        }
        
        struct PrivateGeneric<U> {
            value: U
        }
        
        fn main() -> u64 {
            42u64
        }
    "#;
    
    let result = test_program(source);
    assert!(result.is_ok(), "Visibility modifiers with generics should work: {:?}", result.err());
}

// ===========================================
// Type Inference Edge Cases (Future)
// ===========================================

#[test]
#[ignore] // For when instantiation works
fn test_conflicting_type_inference() {
    let source = r#"
        struct Container<T> {
            value: T
        }
        
        fn get_container<T>(val: T) -> Container<T> {
            Container { value: val }
        }
        
        fn main() -> u64 {
            # Type inference conflict?
            val c = get_container(42u64)
            c.value
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 42),
        Err(e) => panic!("Type inference should resolve: {}", e)
    }
}

#[test]
#[ignore] // For when instantiation works
fn test_partial_type_specification() {
    let source = r#"
        struct Pair<T, U> {
            first: T,
            second: U
        }
        
        fn main() -> u64 {
            # Can we specify just one type?
            val p = Pair { first: 42u64, second: true }
            p.first
        }
    "#;
    
    let result = test_program(source);
    match result {
        Ok(val) => assert_eq!(val.borrow().unwrap_uint64(), 42),
        Err(e) => panic!("Partial type inference should work: {}", e)
    }
}