#[cfg(test)]
mod visibility_tests {
    use frontend::ParserWithInterner;

    #[test]
    fn test_private_function_parsing() {
        let source = r"
        fn main() -> u64 {
            42u64
        }
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Private function should parse successfully");
        
        let program = result.unwrap();
        assert_eq!(program.function.len(), 1);
        
        let function = &program.function[0];
        assert_eq!(function.visibility, frontend::ast::Visibility::Private);
    }

    #[test]
    fn test_public_function_parsing() {
        let source = r"
        pub fn main() -> u64 {
            42u64
        }
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Public function should parse successfully");
        
        let program = result.unwrap();
        assert_eq!(program.function.len(), 1);
        
        let function = &program.function[0];
        assert_eq!(function.visibility, frontend::ast::Visibility::Public);
    }

    #[test]
    fn test_mixed_visibility_functions() {
        let source = r"
        fn private_func() -> u64 {
            1u64
        }
        
        pub fn public_func() -> u64 {
            2u64
        }
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Mixed visibility functions should parse successfully");
        
        let program = result.unwrap();
        assert_eq!(program.function.len(), 2);
        
        // First function should be private
        assert_eq!(program.function[0].visibility, frontend::ast::Visibility::Private);
        
        // Second function should be public  
        assert_eq!(program.function[1].visibility, frontend::ast::Visibility::Public);
    }

    #[test]
    fn test_pub_without_fn_error() {
        let source = r"
        pub
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        
        // Print errors for debugging
        println!("Errors: {:?}", parser.errors);
        
        // This should parse but with errors collected
        assert!(result.is_ok());
        assert!(!parser.errors.is_empty(), "Should have collected errors for pub without fn");
    }

    #[test]
    fn test_pub_struct_parsing() {
        let source = r"
        pub struct Point {
            x: u64
        }
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Public struct should parse successfully");
        
        let program = result.unwrap();
        assert_eq!(program.statement.len(), 1);
        
        // Check that the struct was parsed with public visibility
        if let Some(stmt) = program.statement.get(0) {
            match stmt {
                frontend::ast::Stmt::StructDecl { name: _, fields: _, visibility } => {
                    assert_eq!(*visibility, frontend::ast::Visibility::Public);
                }
                _ => panic!("Expected StructDecl statement"),
            }
        }
    }

    #[test]
    fn test_private_struct_parsing() {
        let source = r"
        struct Point {
            x: u64
        }
        ";
        
        let mut parser = ParserWithInterner::new(source);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Private struct should parse successfully");
        
        let program = result.unwrap();
        assert_eq!(program.statement.len(), 1);
        
        // Check that the struct was parsed with private visibility
        if let Some(stmt) = program.statement.get(0) {
            match stmt {
                frontend::ast::Stmt::StructDecl { name: _, fields: _, visibility } => {
                    assert_eq!(*visibility, frontend::ast::Visibility::Private);
                }
                _ => panic!("Expected StructDecl statement"),
            }
        }
    }
}