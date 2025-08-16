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