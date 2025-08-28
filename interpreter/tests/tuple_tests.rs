mod common;

#[cfg(test)]
mod tuple_tests {
    use super::*;
    use interpreter::object::Object;
    use std::cell::RefCell;
    use std::rc::Rc;

#[test]
fn test_tuple_literal_basic() {
    let source = r#"
        fn main() -> u64 {
            val tuple = (10u64, true, "hello")
            val first = tuple.0
            first
        }
    "#;
    
    common::assert_program_result_u64(source, 10);
}

#[test]
fn test_tuple_literal_empty() {
    let source = r#"
        fn main() -> u64 {
            val empty = ()
            42u64
        }
    "#;
    
    common::assert_program_result_u64(source, 42);
}

#[test]
fn test_tuple_access_multiple_elements() {
    let source = r#"
        fn main() -> u64 {
            val tuple = (5u64, 10u64, 15u64)
            val sum = tuple.0 + tuple.1 + tuple.2
            sum
        }
    "#;
    
    common::assert_program_result_u64(source, 30); // 5 + 10 + 15 = 30
}

#[test]
fn test_tuple_nested() {
    let source = r#"
        fn main() -> u64 {
            val inner = (1u64, 2u64)
            val outer = (inner, 3u64)
            val nested_access = outer.0.1
            nested_access
        }
    "#;
    
    common::assert_program_result_u64(source, 2);
}

#[test]
fn test_tuple_with_different_types() {
    let source = r#"
        fn main() -> u64 {
            val mixed = (42u64, true, false)
            val number = mixed.0
            number
        }
    "#;
    
    common::assert_program_result_u64(source, 42);
}

#[test]
fn test_tuple_function_return() {
    let source = r#"
        fn get_point() -> (u64, u64) {
            (100u64, 200u64)
        }
        
        fn main() -> u64 {
            val point = get_point()
            point.0 + point.1
        }
    "#;
    
    common::assert_program_result_u64(source, 300); // 100 + 200
}

#[test]
fn test_tuple_assignment() {
    let source = r#"
        fn main() -> u64 {
            var point = (10u64, 20u64)
            point = (30u64, 40u64)
            point.0 + point.1
        }
    "#;
    
    common::assert_program_result_u64(source, 70); // 30 + 40
}

#[test]
fn test_tuple_complex_nested() {
    let source = r#"
        fn main() -> u64 {
            val data = ((1u64, 2u64), (3u64, 4u64))
            val first_pair = data.0
            val second_pair = data.1
            val result = first_pair.0 + first_pair.1 + second_pair.0 + second_pair.1
            result
        }
    "#;
    
    common::assert_program_result_u64(source, 10); // 1 + 2 + 3 + 4 = 10
}

#[test]
fn test_tuple_single_element() {
    let source = r#"
        fn main() -> u64 {
            val single = (99u64,)
            single.0
        }
    "#;
    
    common::assert_program_result_u64(source, 99);
}

#[test]
fn test_tuple_type_checking() {
    // Test that tuple elements can have different types and are properly typed
    let source = r#"
        fn main() -> u64 {
            val tuple = (123u64, true, "test")
            tuple.0
        }
    "#;
    
    let result = common::get_program_result(source);
    let borrowed = result.borrow();
    match &*borrowed {
        Object::UInt64(value) => assert_eq!(*value, 123),
        _ => panic!("Expected UInt64, got {:?}", *borrowed),
    }
}

#[test] 
fn test_tuple_with_variables() {
    let source = r#"
        fn main() -> u64 {
            val x = 5u64
            val y = 10u64
            val tuple = (x, y, x + y)
            tuple.2
        }
    "#;
    
    common::assert_program_result_u64(source, 15); // x + y = 5 + 10
}

#[test]
fn test_empty_tuple_type() {
    let source = r#"
        fn main() -> u64 {
            val empty = ()
            # Empty tuple exists, but we return a different value
            123u64
        }
    "#;
    
    common::assert_program_result_u64(source, 123);
}

// Error case tests

#[test]
fn test_tuple_index_out_of_bounds() {
    let _source = r#"
        fn main() -> u64 {
            val tuple = (1u64, 2u64)
            tuple.5  # Index 5 is out of bounds
        }
    "#;
    
    // This should cause an interpreter error
    // This should cause an interpreter error, so we'll use a different approach
    // Since we can't easily test runtime errors with the current test setup,
    // we'll skip this test for now or implement it differently
    // TODO: Implement proper error testing framework
}

#[test]
fn test_tuple_object_type() {
    let source = r#"
        fn main() -> u64 {
            val tuple = (1u64, 2u64, 3u64)
            tuple.0  # Return first element for the test
        }
    "#;
    
    // We'll create a tuple object and test its type
    
    // Create a tuple manually to test the Object::Tuple variant
    let elem1 = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2 = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_obj = Object::Tuple(Box::new(vec![elem1, elem2]));
    
    common::assert_object_type(&tuple_obj, "Tuple");
}

#[test]
fn test_tuple_equality() {
    // Test that tuples with same elements are equal
    
    let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));
    
    let elem1_b = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));
    
    assert_eq!(tuple_a, tuple_b);
}

#[test]
fn test_tuple_inequality() {
    // Test that tuples with different elements are not equal
    
    let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));
    
    let elem1_b = Rc::new(RefCell::new(Object::UInt64(20)));
    let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));
    
    assert_ne!(tuple_a, tuple_b);
}

#[test]
fn test_tuple_hash_consistency() {
    // Test that equal tuples have the same hash
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let elem1_a = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2_a = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_a = Object::Tuple(Box::new(vec![elem1_a, elem2_a]));
    
    let elem1_b = Rc::new(RefCell::new(Object::UInt64(10)));
    let elem2_b = Rc::new(RefCell::new(Object::Bool(true)));
    let tuple_b = Object::Tuple(Box::new(vec![elem1_b, elem2_b]));
    
    let mut hasher_a = DefaultHasher::new();
    tuple_a.hash(&mut hasher_a);
    let hash_a = hasher_a.finish();
    
    let mut hasher_b = DefaultHasher::new();
    tuple_b.hash(&mut hasher_b);
    let hash_b = hasher_b.finish();
    
    assert_eq!(hash_a, hash_b, "Equal tuples should have the same hash");
}

} // end of tuple_tests module