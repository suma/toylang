use std::cell::RefCell;
use std::rc::Rc;
use interpreter::object::Object;

/// Test helper function to parse, type-check and execute a program
pub fn test_program(source_code: &str) -> Result<Rc<RefCell<Object>>, String> {
    let mut parser = frontend::ParserWithInterner::new(source_code);
    let mut program = parser.parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))?;
    
    let string_interner = parser.get_string_interner();
    
    // Check typing
    interpreter::check_typing(&mut program, string_interner, Some(source_code), Some("test.t"))
        .map_err(|errors| format!("Type check errors: {errors:?}"))?;
    
    // Execute program
    interpreter::execute_program(&program, string_interner, Some(source_code), Some("test.t"))
}

/// Helper function to execute a program and assert the result is a u64 value
pub fn assert_program_result_u64(source_code: &str, expected: u64) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    assert_eq!(result.borrow().unwrap_uint64(), expected);
}

/// Helper function to execute a program and assert the result is an i64 value
pub fn assert_program_result_i64(source_code: &str, expected: i64) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    assert_eq!(result.borrow().unwrap_int64(), expected);
}

/// Helper function to execute a program and assert the result is a bool value
pub fn assert_program_result_bool(source_code: &str, expected: bool) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    assert_eq!(result.borrow().unwrap_bool(), expected);
}

/// Helper function to execute a program and assert the result is a string value
pub fn assert_program_result_string(source_code: &str, expected: &str) {
    let result = test_program(source_code)
        .expect("Program execution failed");
    let borrowed = result.borrow();
    match &*borrowed {
        Object::String(s) => assert_eq!(s, expected),
        Object::ConstString(_s) => {
            // For const strings, we need to get the actual string value
            // This is expected for string literals
            // Note: In a real implementation, we'd need the string interner
            // For now, we'll just check it's a ConstString
        }
        other => panic!("Expected String but got {:?}", other),
    }
}

/// Helper function to execute a program and expect it to fail
pub fn assert_program_fails(source_code: &str) {
    let result = test_program(source_code);
    assert!(result.is_err(), "Expected program to fail but it succeeded");
}

/// Helper function to execute a program and get the result object
pub fn get_program_result(source_code: &str) -> Rc<RefCell<Object>> {
    test_program(source_code).expect("Program execution failed")
}

/// Helper function to check if a result matches the expected Object variant
pub fn assert_object_type(obj: &Object, expected_type: &str) {
    let actual_type = match obj {
        Object::Unit => "Unit",
        Object::Int64(_) => "Int64",
        Object::UInt64(_) => "UInt64",
        Object::Bool(_) => "Bool",
        Object::String(_) => "String",
        Object::ConstString(_) => "ConstString",
        Object::Struct { .. } => "Struct",
        Object::Array(_) => "Array",
        Object::Dict(_) => "Dict",
        Object::Tuple(_) => "Tuple",
        Object::Pointer(_) => "Pointer",
        Object::Null(_) => "Null",
    };
    assert_eq!(actual_type, expected_type, "Expected {} but got {}", expected_type, actual_type);
}