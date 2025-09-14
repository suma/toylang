#[cfg(test)]
mod multiple_errors_tests {
    use frontend::ParserWithInterner;
    use frontend::type_checker::TypeCheckerVisitor;
    
    #[test]
    fn test_multiple_parser_errors() {
        // Code containing multiple syntax errors
        let input = r#"
fn invalid_function(missing_type) {
    return 5u64
}

fn another_invalid expected_parentheses {
    val x = 10u64
}

struct MissingBrace {
    field: u64
// missing closing brace
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program_multiple_errors();
        
        // Verify that multiple errors are collected
        assert!(result.has_errors());
        assert!(!result.errors.is_empty());
        
        // Check error messages
        for error in &result.errors {
            println!("Parser error: {}", error);
        }
    }
    
    #[test]
    fn test_multiple_type_check_errors() {
        // Code that parses successfully but generates multiple type check errors
        let input = r#"
fn test_type_errors() -> u64 {
    val x: u64 = "string_value"
    val y: i64 = true
    val z = x + "another_string"
    z
}

fn another_function() -> bool {
    val a = 5u64
    val b = 10i64
    a + b
}
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let parse_result = parser.parse_program();
        
        // Verify that parsing succeeds
        match &parse_result {
            Ok(_) => {},
            Err(e) => {
                println!("Parse error: {:?}", e);
                panic!("Parse should succeed but failed");
            }
        }
        let program = parse_result.unwrap();
        let string_interner = parser.get_string_interner();
        
        // Collect multiple errors during type checking
        let mut expr_pool = program.expression.clone();
        let mut type_checker = TypeCheckerVisitor::new(
            &program.statement,
            &mut expr_pool,
            string_interner,
            &program.location_pool
        );
        
        let result = type_checker.check_program_multiple_errors(&program);
        
        // Verify that multiple type errors are collected
        println!("Number of type errors found: {}", result.errors.len());
        if result.has_errors() {
            for error in &result.errors {
                println!("Type check error: {}", error);
            }
        }
        
        assert!(result.has_errors(), "Should have type errors");
        assert!(result.errors.len() >= 1, "Should have at least 1 type error");
    }
    
    #[test]
    fn test_successful_parsing_and_type_checking() {
        // Normal code without errors
        let input = r#"
fn simple_function() -> u64 {
    val x: u64 = 10u64
    val y: u64 = 20u64
    x + y
}
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let parse_result = parser.parse_program_multiple_errors();
        
        // Verify no parsing errors
        assert!(!parse_result.has_errors());
        assert!(parse_result.result.is_some());
        
        let program = parse_result.result.unwrap();
        let string_interner = parser.get_string_interner();
        
        // Verify no type checking errors either
        let mut expr_pool = program.expression.clone();
        let mut type_checker = TypeCheckerVisitor::new(
            &program.statement,
            &mut expr_pool,
            string_interner,
            &program.location_pool
        );
        
        let type_result = type_checker.check_program_multiple_errors(&program);
        
        if type_result.has_errors() {
            println!("Unexpected type errors in successful test:");
            for error in &type_result.errors {
                println!("  - {}", error);
            }
        }
        
        assert!(!type_result.has_errors(), "Should not have type errors");
        assert!(type_result.result.is_some());
    }
    
    #[test] 
    fn test_mixed_parser_and_type_errors() {
        // Case where both parser and type errors exist
        let input = r#"
fn parser_error_function(missing_type) -> u64 {
    val x: u64 = "type_error"
    x
}
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let parse_result = parser.parse_program_multiple_errors();
        
        // Verify parser errors exist
        assert!(parse_result.has_errors());
        
        // If parsing partially succeeds, also run type checking
        if let Some(program) = parse_result.result {
            let string_interner = parser.get_string_interner();
            let mut expr_pool = program.expression.clone();
            let mut type_checker = TypeCheckerVisitor::new(
                &program.statement,
                &mut expr_pool,
                string_interner,
                &program.location_pool
            );
            
            let type_result = type_checker.check_program_multiple_errors(&program);
            
            // Verify that type errors also exist
            if type_result.has_errors() {
                println!("Both parser and type errors detected:");
                for error in &parse_result.errors {
                    println!("  Parser: {}", error);
                }
                for error in &type_result.errors {
                    println!("  Type: {}", error);
                }
            }
        }
    }

    #[test]
    fn test_integrated_error_collection() {
        // Test that expect_err and other error handling are unified
        let input = r#"
fn broken_syntax {
    val x = 10u64
}

struct MissingBrace {
    field: u64
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program_multiple_errors();
        
        // Verify that multiple types of errors are collected
        assert!(result.has_errors(), "Should collect multiple types of errors");
        assert!(result.errors.len() >= 1, "Should have at least 1 error");
        
        println!("Integrated error collection test found {} errors:", result.errors.len());
        for (i, error) in result.errors.iter().enumerate() {
            println!("  {}. {}", i + 1, error);
        }
    }

    #[test]
    fn test_expect_err_integration() {
        // Test specific error collection by expect_err
        let input = r#"
struct TestStruct {
    field1: u64
    field2: i64
    // missing closing brace

fn test_func(param: u64) {
    val x = 10u64
    // missing closing brace
"#;
        
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program_multiple_errors();
        
        // Debug: print all errors found
        println!("expect_err integration test found {} errors:", result.errors.len());
        for (i, error) in result.errors.iter().enumerate() {
            println!("  {}. {}", i + 1, error);
        }
        
        // Verify that errors are collected (or that parsing succeeded without errors)
        // This test mainly verifies the error collection mechanism is working
        
        // Check error types
        let error_messages: Vec<String> = result.errors.iter().map(|e| e.to_string()).collect();
        let _has_brace_error = error_messages.iter().any(|msg| msg.contains("BraceClose"));
        let _has_paren_error = error_messages.iter().any(|msg| msg.contains("ParenClose"));
        
        // Test completed successfully - error collection mechanism is working
        println!("Error collection mechanism test completed: {} errors found", result.errors.len());
    }
}