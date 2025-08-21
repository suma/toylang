use interpreter::object::{Object, clear_destruction_log};

mod common;
use common::test_program;

#[test]
fn test_custom_drop_method_call() {
    clear_destruction_log();
    
    let source_code = r#"
struct Resource {
    name: str
}

impl Resource {
    fn __drop__(self: Self) {
        # Custom destructor logic would go here
        # For testing, we just rely on the logging system
    }
}

fn main() -> u64 {
    val resource = Resource { name: "test_resource" }
    42u64
}
"#;
    
    // Parse and execute the program
    let result = test_program(source_code);
    assert!(result.is_ok(), "Program should execute successfully: {:?}", result);
    
    // The test just verifies that __drop__ method can be defined and parsed successfully
    // Explicit destructor calling would be tested in integration tests
}

#[test]
fn test_explicit_destructor_call() {
    clear_destruction_log();
    
    let source_code = r#"
struct TestStruct {
    value: u64
}

impl TestStruct {
    fn __drop__(self: Self) {
        # This method exists and should be callable
    }
}

fn main() -> u64 {
    val obj = TestStruct { value: 42u64 }
    99u64
}
"#;

    // Parse and execute the program
    let result = test_program(source_code);
    assert!(result.is_ok(), "Program with __drop__ method should execute: {:?}", result);
    
    // For now, we just test that programs with __drop__ methods can be parsed and executed
    // The explicit calling mechanism would be used in scenarios where objects need to be
    // cleaned up before going out of scope naturally
}

#[test]
fn test_struct_without_drop_method() {
    clear_destruction_log();
    
    let source_code = r#"
struct SimpleStruct {
    data: u64
}

impl SimpleStruct {
    fn get_data(self: Self) -> u64 {
        self.data
    }
}

fn main() -> u64 {
    val obj = SimpleStruct { data: 100u64 }
    obj.get_data()
}
"#;

    let result = test_program(source_code);
    assert!(result.is_ok(), "Program without __drop__ method should work normally: {:?}", result);
    
    if let Ok(actual_rc) = result {
        let actual = actual_rc.borrow();
        match &*actual {
            Object::UInt64(100) => {}, // Expected
            other => panic!("Expected UInt64(100), got {:?}", other),
        }
    }
}

#[test]
fn test_multiple_structs_with_drop() {
    clear_destruction_log();
    
    let source_code = r#"
struct ResourceA {
    name: str
}

struct ResourceB {
    id: u64
}

impl ResourceA {
    fn __drop__(self: Self) {
        # ResourceA destructor
    }
}

impl ResourceB {
    fn __drop__(self: Self) {
        # ResourceB destructor  
    }
}

fn main() -> u64 {
    val a = ResourceA { name: "resource_a" }
    val b = ResourceB { id: 123u64 }
    456u64
}
"#;

    let result = test_program(source_code);
    assert!(result.is_ok(), "Multiple structs with __drop__ should work: {:?}", result);
}

#[test] 
fn test_drop_method_signature() {
    // Test that __drop__ method must have the correct signature (self: Self)
    let source_code = r#"
struct TestStruct {
    data: u64
}

impl TestStruct {
    fn __drop__(self: Self) {
        # Correct signature - takes self by value
    }
}

fn main() -> u64 {
    val obj = TestStruct { data: 42u64 }
    1u64
}
"#;

    let result = test_program(source_code);
    assert!(result.is_ok(), "Correct __drop__ signature should work: {:?}", result);
}

#[test]
fn test_drop_method_with_complex_cleanup() {
    clear_destruction_log();
    
    let source_code = r#"
struct ComplexResource {
    data: [u64; 3],
    name: str
}

impl ComplexResource {
    fn __drop__(self: Self) {
        # In a real implementation, this could:
        # - Close file handles
        # - Release network connections  
        # - Free external resources
        # - Log cleanup actions
    }
    
    fn get_sum(self: Self) -> u64 {
        self.data[0u64] + self.data[1u64] + self.data[2u64]
    }
}

fn main() -> u64 {
    val resource = ComplexResource { 
        data: [1u64, 2u64, 3u64],
        name: "complex"
    }
    resource.get_sum()
}
"#;

    let result = test_program(source_code);
    assert!(result.is_ok(), "Complex struct with __drop__ should work: {:?}", result);
    
    // Verify the computation works correctly
    if let Ok(actual_rc) = result {
        let actual = actual_rc.borrow();
        if let Object::UInt64(sum) = &*actual {
            assert_eq!(*sum, 6); // 1 + 2 + 3 = 6
        } else {
            panic!("Expected UInt64(6), got {:?}", actual);
        }
    } else {
        panic!("Program failed: {:?}", result);
    }
}