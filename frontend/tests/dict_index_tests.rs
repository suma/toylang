use frontend::type_checker::TypeCheckerVisitor;
use frontend::type_decl::TypeDecl;
use frontend::visitor::AstVisitor;
use frontend::ast::StmtRef;

/// Test helper function to parse and type check a source string
fn parse_and_check(source: &str) -> Result<TypeDecl, String> {
    use frontend::parser::core::ParserWithInterner;
    
    let mut parser = ParserWithInterner::new(source);
    
    match parser.parse_program() {
        Ok(mut program) => {
            if program.statement.0.is_empty() {
                return Err("No statements found".to_string());
            }
            
            let string_interner = parser.get_string_interner();
            let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
            
            let main_stmt_ref = StmtRef(0);
            type_checker.visit_stmt(&main_stmt_ref).map_err(|e| format!("{:?}", e))
        }
        Err(e) => Err(format!("Parse error: {:?}", e))
    }
}

/// Test helper to check if parsing succeeds
fn parse_succeeds(source: &str) -> bool {
    use frontend::parser::core::ParserWithInterner;
    
    let mut parser = ParserWithInterner::new(source);
    parser.parse_program().is_ok()
}

#[test]
fn test_empty_dict_literal_parsing() {
    assert!(parse_succeeds("dict{}"));
}

#[test]
fn test_simple_dict_literal_parsing() {
    assert!(parse_succeeds(r#"dict{"key": "value"}"#));
}

#[test]
fn test_multi_entry_dict_literal_parsing() {
    let source = r#"dict{
        "name": "Alice",
        "age": "30",
        "city": "Tokyo"
    }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_dict_literal_with_trailing_comma() {
    let source = r#"dict{
        "key1": "value1",
        "key2": "value2",
    }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_dict_literal_with_newlines() {
    let source = r#"dict{
        
        "key1": 
            "value1",
        
        "key2": 
            "value2"
            
    }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_index_access_parsing() {
    assert!(parse_succeeds(r#"x["key"]"#));
}

#[test]
fn test_chained_index_access_parsing() {
    assert!(parse_succeeds(r#"matrix[1][2]"#));
}

#[test]
fn test_index_assignment_parsing() {
    assert!(parse_succeeds(r#"x["key"] = "value""#));
}

#[test]
fn test_array_index_access_parsing() {
    assert!(parse_succeeds("arr[0]"));
}

#[test]
fn test_dict_type_checking_string_keys_values() {
    let source = r#"fn main() -> u64 {
        val d = dict{"name": "Alice", "city": "Tokyo"}
        1u64
    }"#;
    let result = parse_and_check(source);
    
    match result {
        Ok(_) => {
            // Should succeed - type checking passed
        }
        Err(e) => panic!("Type checking failed: {}", e)
    }
}

#[test] 
fn test_dict_type_checking_string_keys_number_values() {
    let source = r#"fn main() -> u64 {
        val d = dict{"one": 1, "two": 2}
        1u64
    }"#;
    let result = parse_and_check(source);
    
    match result {
        Ok(_) => {
            // Should succeed - consistent value types
        }
        Err(e) => panic!("Type checking failed: {}", e)
    }
}

#[test]
fn test_empty_dict_type_checking() {
    let source = r#"fn main() -> u64 {
        val d = dict{}
        1u64
    }"#;
    let result = parse_and_check(source);
    
    match result {
        Ok(_) => {
            // Empty dict should parse and type check successfully
        }
        Err(e) => panic!("Type checking failed: {}", e)
    }
}

#[test]
fn test_mixed_dict_value_types_should_fail() {
    let source = r#"fn main() -> u64 {
        val d = dict{"str": "value", "num": 42}
        1u64
    }"#;
    let result = parse_and_check(source);
    
    // Should fail due to inconsistent value types (string vs int)
    assert!(result.is_err(), "Expected type error for mixed value types, but got: {:?}", result);
    
    if let Err(error_msg) = result {
        assert!(error_msg.contains("type mismatch") || error_msg.contains("same type"), 
               "Error message should mention type mismatch: {}", error_msg);
    }
}

#[test]
fn test_mixed_dict_key_types_should_fail() {
    let source = r#"val d = dict{"str": "value1", 42: "value2"}"#;
    let result = parse_and_check(source);
    
    // Should fail due to inconsistent key types (string vs int)
    assert!(result.is_err(), "Expected type error for mixed key types, but got: {:?}", result);
}

#[test] 
fn test_dict_with_number_keys() {
    let source = r#"dict{1: "one", 2: "two"}"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_nested_dict_literal() {
    let source = r#"dict{
        "person": dict{
            "name": "Alice",
            "age": "30"
        },
        "location": "Tokyo"
    }"#;
    
    // Note: This should fail because the outer dict has mixed value types
    // (inner dict vs string)
    let result = parse_and_check(&format!("val d = {}", source));
    assert!(result.is_err(), "Expected error for mixed value types in nested dict");
}

#[test]
fn test_dict_keyword_is_reserved() {
    let source = "val dict = 42";
    assert!(!parse_succeeds(source), "Should not allow 'dict' as variable name");
}

#[test]
fn test_complex_index_expression_parsing() {
    assert!(parse_succeeds(r#"data["users"][0]["name"]"#));
}

#[test]
fn test_index_with_expression() {
    assert!(parse_succeeds(r#"dict[key + "suffix"]"#));
}

#[test]
fn test_consistent_dict_operations() {
    let source = r#"
val numbers = dict{"one": 1, "two": 2}
numbers["three"] = 3
"#;
    let result = parse_and_check(source);
    
    // Should succeed - all operations maintain type consistency
    match result {
        Ok(_) => {},
        Err(e) => panic!("Type checking should succeed for consistent operations: {}", e)
    }
}

#[test]
fn test_inconsistent_dict_assignment_should_fail() {
    let source = r#"
val numbers = dict{"one": 1, "two": 2}
numbers["three"] = "three"
"#;
    let result = parse_and_check(source);
    
    // Should fail - trying to assign string to number dict
    assert!(result.is_err(), "Expected error for inconsistent assignment, but got: {:?}", result);
    
    if let Err(error_msg) = result {
        assert!(error_msg.contains("type mismatch") || error_msg.contains("same type"), 
               "Error should mention type mismatch: {}", error_msg);
    }
}

#[test]
fn test_array_index_operations() {
    let source = r#"
val arr = [1, 2, 3]
arr[0] = 42
"#;
    let result = parse_and_check(source);
    
    // Should succeed - consistent array operations
    match result {
        Ok(_) => {},
        Err(e) => panic!("Array operations should succeed: {}", e)
    }
}

#[test]
fn test_array_type_mismatch_assignment() {
    let source = r#"
val arr = [1, 2, 3]
arr[0] = "text"
"#;
    let result = parse_and_check(source);
    
    // Should fail - string assignment to number array
    assert!(result.is_err(), "Expected error for array type mismatch, but got: {:?}", result);
}

#[test]
fn test_dict_type_syntax_parsing() {
    let source = r#"fn test(param: dict[str, u64]) -> u64 { 1u64 }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_dict_type_declaration_parsing() {
    let source = r#"fn main() -> u64 {
        val d: dict[str, u64] = dict{}
        1u64
    }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_nested_dict_type_parsing() {
    let source = r#"fn main() -> u64 {
        val d: dict[str, dict[str, u64]] = dict{}
        1u64
    }"#;
    assert!(parse_succeeds(source));
}

#[test]
fn test_dict_and_array_different_contexts() {
    let source = r#"
val dict_data = dict{"key": "value"}
val array_data = ["item1", "item2"]
val dict_item = dict_data["key"]
val array_item = array_data[0]
"#;
    let result = parse_and_check(source);
    
    // Should succeed - proper usage of both dict and array indexing
    match result {
        Ok(_) => {},
        Err(e) => panic!("Mixed dict and array usage should work: {}", e)
    }
}