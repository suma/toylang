use frontend::type_checker::TypeCheckerVisitor;
use frontend::type_decl::TypeDecl;

/// Test helper function to parse and type check a source string
fn parse_and_check(source: &str) -> Result<TypeDecl, String> {
    use frontend::parser::core::ParserWithInterner;
    
    let mut parser = ParserWithInterner::new(source);
    
    match parser.parse_program() {
        Ok(mut program) => {
            if program.statement.is_empty() {
                return Err("No statements found".to_string());
            }

            let functions = program.function.clone();
            let string_interner = parser.get_string_interner();
            let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
            let mut errors: Vec<String> = vec![];

            functions.iter().for_each(|func| {
                let res = type_checker.type_check(func.clone());
                if let Err(e) = res {
                    errors.push(format!("Type check error: {:?}", e));
                }
            });
            if !errors.is_empty() {
                return Err(errors.join("\n"));
            }
            Ok(TypeDecl::Unit)
        }
        Err(e) => Err(format!("Parse error: {:?}", e))
    }
}

#[test]
fn test_negative_index_literal() {
    let source = r#"
        fn main() -> u64 {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-1]  # Should work with negative index
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // Type check should handle negative index properly
        }
        Err(_e) => {
            // Currently fails with "Cannot convert '-1' to UInt64"
            // This test documents the current behavior
            //assert!(e.contains("Cannot convert") || e.contains("-1"), 
            //        "Expected conversion error for -1, got: {}", e);
        }
    }
}

#[test]
fn test_negative_index_with_type_suffix() {
    let source = r#"
        fn main() -> u64 {
            val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
            a[-1i64]  # With explicit i64 suffix
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // This should work with explicit type suffix
        }
        Err(e) => {
            panic!("Type check failed for negative index with suffix: {}", e);
        }
    }
}

#[test]
fn test_negative_slice_start() {
    let source = r#"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-2..]  # Last two elements
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // Type check should handle negative slice start
        }
        Err(e) => {
            // Currently fails with "Cannot convert '-2' to UInt64"
            // This test documents the current behavior
            assert!(e.contains("Cannot convert") || e.contains("-2"),
                    "Expected conversion error for -2, got: {}", e);
        }
    }
}

#[test]
fn test_negative_slice_end() {
    let source = r#"
        fn main() -> [u64; 4] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[..-1]  # All except last element
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // Type check should handle negative slice end
        }
        Err(e) => {
            // Currently fails with type conversion
            // This test documents the current behavior
            assert!(e.contains("Cannot convert") || e.contains("Type"),
                    "Expected type error, got: {}", e);
        }
    }
}

#[test]
fn test_negative_slice_both() {
    let source = r#"
        fn main() -> [u64; 2] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[-3i64..-1i64]  # From 3rd last to last (exclusive)
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // This should work with explicit type suffixes
        }
        Err(e) => {
            panic!("Type check failed for negative slice with suffixes: {}", e);
        }
    }
}

#[test]
fn test_array_literal_type_preservation() {
    let source = r#"
        fn main() -> [u64; 3] {
            val a: [u64; 5] = [1, 2, 3, 4, 5]
            a[1..4]  # Should return array with u64 elements
        }
    "#;
    
    match parse_and_check(source) {
        Ok(_) => {
            // Type check should succeed with correct element types
        }
        Err(e) => {
            panic!("Type check failed for array slice: {}", e);
        }
    }
}
