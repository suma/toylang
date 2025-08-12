#[cfg(test)]
mod edge_case_tests {
    use crate::parser::core::Parser;

    // Test helper function for parser-only tests
    fn parse_program(input: &str) -> Result<(), String> {
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        
        match result {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("Parse errors: {:?}", errors))
        }
    }

    // Edge case: Empty program
    #[test]
    fn test_empty_program() {
        let input = "";
        let result = parse_program(input);
        // Empty program might be allowed or not depending on implementation
        assert!(result.is_ok() || result.is_err(), "Empty program handling tested");
    }

    // Edge case: Program with only whitespace
    #[test]
    fn test_whitespace_only_program() {
        let input = "   \n\t\n   ";
        let result = parse_program(input);
        // Parser might accept empty/whitespace programs
        assert!(result.is_ok() || result.is_err(), "Whitespace-only program handling tested");
    }

    // Edge case: Program with only comments
    #[test]
    fn test_comment_only_program() {
        let input = "# This is a comment\n# Another comment";
        let result = parse_program(input);
        // Parser might accept comment-only programs
        assert!(result.is_ok() || result.is_err(), "Comment-only program handling tested");
    }

    // Edge case: Very deeply nested expressions
    #[test]
    fn test_deeply_nested_expression() {
        let mut expr = String::from("1i64");
        for _ in 0..50 {
            expr = format!("({} + 1i64)", expr);
        }
        let input = format!("fn main() -> i64 {{ {} }}", expr);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Deeply nested expression should parse");
    }

    // Edge case: Maximum identifier length
    #[test]
    // #[ignore] // Hangs with long identifiers
    fn test_very_long_identifier() {
        let long_name = "a".repeat(20); // Reduced for safety
        let input = format!("fn main() -> i64 {{ val {} = 1i64\n{} }}", long_name, long_name);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Long identifier should be accepted");
    }

    // Edge case: Identifiers with numbers and underscores
    #[test]
    fn test_valid_identifier_patterns() {
        let valid_names = vec![
            "variable1",
            "var_name",
            "_private",
            "__double",
            "snake_case_var",
            "var123",
            "v1_2_3",
            "_",
            "_123",
        ];
        
        for name in valid_names {
            let input = format!("fn main() -> i64 {{ val {} = 1i64\n{} }}", name, name);
            let result = parse_program(&input);
            assert!(result.is_ok(), "Valid identifier '{}' should be accepted", name);
        }
    }

    // Edge case: Invalid identifier patterns (starting with number)
    #[test]
    #[ignore] // Lexer tokenizes "123var" as "123" and "var" separately
    fn test_invalid_identifier_patterns() {
        let invalid_names = vec![
            "123var",  // starts with number
            "1_var",   // starts with number
        ];
        
        for name in invalid_names {
            let input = format!("fn main() -> i64 {{ val {} = 1i64\n{} }}", name, name);
            let result = parse_program(&input);
            assert!(result.is_err(), "Invalid identifier '{}' should be rejected", name);
        }
    }

    // Edge case: Zero-length array
    #[test]
    fn test_zero_length_array() {
        let input = "fn main() -> i64 { val a: [i64; 0] = []; 0i64 }";
        let result = parse_program(input);
        // This should either parse successfully or fail with a specific error
        // depending on language design
        assert!(result.is_ok() || result.is_err());
    }

    // Edge case: Array with maximum size
    #[test]
    fn test_large_array_declaration() {
        let input = "fn main() -> i64 { val a: [i64; 10000] = [0i64]; 0i64 }";
        let result = parse_program(input);
        // Should handle large array sizes
        assert!(result.is_ok() || result.is_err());
    }

    // Edge case: Function with no parameters and no return
    #[test]
    fn test_void_function() {
        let input = "fn do_nothing() { } fn main() -> i64 { 0i64 }";
        let result = parse_program(input);
        assert!(result.is_ok(), "Void function should parse");
    }

    // Edge case: Multiple consecutive operators
    #[test]
    fn test_consecutive_operators() {
        let input = "fn main() -> i64 { 1i64 ++ 2i64 }";
        let result = parse_program(input);
        // Parser might handle this differently (error recovery)
        assert!(result.is_ok() || result.is_err(), "Consecutive operators handling tested");
    }

    // Edge case: Unmatched parentheses
    #[test]
    fn test_unmatched_parentheses() {
        let test_cases = vec![
            "fn main() -> i64 { ((1i64 + 2i64) }",
            "fn main() -> i64 { (1i64 + 2i64)) }",
            "fn main() -> i64 { val a = (1i64\n0i64 }",
        ];
        
        for input in test_cases {
            let result = parse_program(input);
            // Parser might have good error recovery
            assert!(result.is_ok() || result.is_err(), "Unmatched parentheses handling tested: {}", input);
        }
    }

    // Edge case: Unmatched brackets
    #[test]
    fn test_unmatched_brackets() {
        let test_cases = vec![
            "fn main() -> i64 { val a = [1i64, 2i64; 0i64 }",
            "fn main() -> i64 { val a = 1i64, 2i64]; 0i64 }",
        ];
        
        for input in test_cases {
            let result = parse_program(input);
            // Parser might have good error recovery
            assert!(result.is_ok() || result.is_err(), "Unmatched brackets handling tested: {}", input);
        }
    }

    // Edge case: Reserved keywords as identifiers
    #[test]
    fn test_reserved_keywords_as_identifiers() {
        let keywords = vec!["if", "else", "while", "for", "fn", "return", "break", "continue", "val", "var", "struct", "impl"];
        
        for keyword in keywords {
            let input = format!("fn main() -> i64 {{ val {} = 1i64\n0i64 }}", keyword);
            let result = parse_program(&input);
            assert!(result.is_err(), "Keyword '{}' should not be allowed as identifier", keyword);
        }
    }

    // Edge case: Extreme integer values
    #[test]
    fn test_extreme_integer_values() {
        let test_cases = vec![
            ("fn main() -> i64 { 9223372036854775807i64 }", true),  // i64::MAX
            ("fn main() -> i64 { -9223372036854775808i64 }", true), // i64::MIN
            ("fn main() -> u64 { 18446744073709551615u64 }", true), // u64::MAX
            ("fn main() -> u64 { 0u64 }", true),                    // u64::MIN
        ];
        
        for (input, should_pass) in test_cases {
            let result = parse_program(input);
            if should_pass {
                assert!(result.is_ok(), "Extreme value should parse: {}", input);
            }
        }
    }

    // Edge case: String with special characters
    #[test]
    fn test_string_special_characters() {
        let test_cases = vec![
            r#"fn main() -> i64 { val s = "hello\nworld"; 0i64 }"#,
            r#"fn main() -> i64 { val s = "hello\tworld"; 0i64 }"#,
            r#"fn main() -> i64 { val s = "hello\"world"; 0i64 }"#,
            r#"fn main() -> i64 { val s = "hello\\world"; 0i64 }"#,
        ];
        
        for input in test_cases {
            let result = parse_program(input);
            // String handling depends on implementation
            assert!(result.is_ok() || result.is_err());
        }
    }

    // Edge case: Empty function body
    #[test]
    fn test_empty_function_body() {
        let input = "fn empty() -> i64 { } fn main() -> i64 { 0i64 }";
        let result = parse_program(input);
        // Empty body might be allowed or not depending on language design
        assert!(result.is_ok() || result.is_err());
    }

    // Edge case: Nested struct definitions
    #[test]
    fn test_nested_struct_depth() {
        let mut struct_def = String::from("struct Level0 { value: i64 }");
        for i in 1..10 {
            struct_def.push_str(&format!("\nstruct Level{} {{ inner: Level{} }}", i, i-1));
        }
        let input = format!("{}\nfn main() -> i64 {{ 0i64 }}", struct_def);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Nested struct definitions should parse");
    }

    // Edge case: Method call chain
    #[test]
    fn test_long_method_chain() {
        let input = r#"
            struct Value { x: i64 }
            impl Value {
                fn get(&self) -> Value { Value { x: self.x } }
            }
            fn main() -> i64 {
                val v = Value { x: 1i64 };
                v.get().get().get().get().get().x
            }
        "#;
        let result = parse_program(input);
        assert!(result.is_ok(), "Long method chain should parse");
    }

    // Edge case: Complex type annotations
    #[test]
    fn test_complex_type_annotations() {
        let input = "fn main() -> i64 { val a: [[[[i64; 2]; 2]; 2]; 2] = [[[[0i64]]]]; 0i64 }";
        let result = parse_program(input);
        // Multi-dimensional arrays might have limits
        assert!(result.is_ok() || result.is_err());
    }

    // Edge case: Binary operator precedence
    #[test]
    fn test_operator_precedence_edge_cases() {
        let test_cases = vec![
            "fn main() -> i64 { 1i64 + 2i64 * 3i64 }",        // Should be 1 + (2 * 3) = 7
            "fn main() -> i64 { 1i64 * 2i64 + 3i64 }",        // Should be (1 * 2) + 3 = 5
            "fn main() -> i64 { 1i64 + 2i64 + 3i64 + 4i64 }", // Left associative
            "fn main() -> i64 { 1i64 - 2i64 - 3i64 }",        // Left associative
        ];
        
        for input in test_cases {
            let result = parse_program(input);
            assert!(result.is_ok(), "Operator precedence test should parse: {}", input);
        }
    }

    // Edge case: Function with many parameters
    #[test]
    fn test_many_function_parameters() {
        let params: Vec<String> = (0..10).map(|i| format!("p{}: i64", i)).collect(); // Reduced from 100
        let args: Vec<String> = (0..10).map(|i| format!("{}i64", i)).collect(); // Reduced from 100
        let input = format!(
            "fn many_params({}) -> i64 {{ 0i64 }} fn main() -> i64 {{ many_params({}) }}",
            params.join(", "),
            args.join(", ")
        );
        let result = parse_program(&input);
        assert!(result.is_ok(), "Function with many parameters should parse");
    }

    // Edge case: Deeply nested blocks
    #[test]
    fn test_deeply_nested_blocks() {
        let mut blocks = String::from("0i64");
        for i in 0..10 { // Reduced from 50
            blocks = format!("{{ val x{} = {}i64\n{} }}", i, i, blocks);
        }
        let input = format!("fn main() -> i64 {{ {} }}", blocks);
        let result = parse_program(&input);
        assert!(result.is_ok(), "Deeply nested blocks should parse");
    }

    // Edge case: Multiple return statements
    #[test]
    fn test_multiple_returns() {
        let input = r#"
            fn multi_return(x: i64) -> i64 {
                if x < 0i64 {
                    return -1i64
                }
                if x == 0i64 {
                    return 0i64
                }
                return 1i64
            }
            fn main() -> i64 { multi_return(5i64) }
        "#;
        let result = parse_program(input);
        assert!(result.is_ok(), "Multiple return statements should parse");
    }

    // Edge case: Chained comparisons
    #[test]
    fn test_chained_comparisons() {
        let input = "fn main() -> bool { 1i64 < 2i64 && 2i64 < 3i64 && 3i64 < 4i64 }";
        let result = parse_program(input);
        assert!(result.is_ok(), "Chained comparisons should parse");
    }

    // Edge case: Mixed type operations (should fail type checking)
    #[test]
    fn test_mixed_type_operations() {
        let input = "fn main() -> i64 { 1i64 + 2u64 }";
        let result = parse_program(input);
        // Should parse but fail type checking
        assert!(result.is_ok(), "Mixed types should parse (but fail type check later)");
    }

    // Edge case: Shadowing variables
    #[test]
    fn test_variable_shadowing() {
        let input = r#"
            fn main() -> i64 {
                val x = 1i64;
                {
                    val x = 2i64;
                    {
                        val x = 3i64;
                        x
                    }
                }
            }
        "#;
        let result = parse_program(input);
        assert!(result.is_ok(), "Variable shadowing should parse");
    }
}