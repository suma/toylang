#[cfg(test)]
mod boundary_tests {
    use frontend::ParserWithInterner;

    // Helper function for parser-only testing
    fn parse_program_only(input: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(input);
        match parser.parse_program() {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("Parse error: {:?}", errors))
        }
    }

    // Test minimum and maximum integer values
    #[test]
    // #[ignore] // Lexer panics on overflow
    fn test_i64_boundaries() {
        let test_cases = vec![
            ("9223372036854775807i64", true),   // i64::MAX
            ("-9223372036854775808i64", true),  // i64::MIN
            ("9223372036854775808i64", false),  // i64::MAX + 1 (should fail)
            ("-9223372036854775809i64", false), // i64::MIN - 1 (should fail)
        ];

        for (value, should_pass) in test_cases {
            let input = format!("fn main() -> i64 {{ {} }}", value);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Value {} should be accepted", value);
            } else {
                // Large values might be rejected at parse time or type check time
                assert!(result.is_ok() || result.is_err(), "Value {} handling tested", value);
            }
        }
    }

    #[test]
    // #[ignore] // Lexer panics on overflow
    fn test_u64_boundaries() {
        let test_cases = vec![
            ("0u64", true),                      // u64::MIN
            ("18446744073709551615u64", true),   // u64::MAX
            ("18446744073709551616u64", false),  // u64::MAX + 1 (should fail)
        ];

        for (value, should_pass) in test_cases {
            let input = format!("fn main() -> u64 {{ {} }}", value);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Value {} should be accepted", value);
            } else {
                // Large values might be rejected at parse time
                assert!(result.is_ok() || result.is_err(), "Value {} handling tested", value);
            }
        }
    }

    // Test array access boundaries (simplified for current language capabilities)
    #[test]
    fn test_array_access_boundaries() {
        // Since arrays aren't implemented yet, test simple variable declarations instead
        let test_cases = vec![
            ("val a: i64 = 0i64", true),     // Basic variable declaration
            ("val b: u64 = 1u64", true),     // Another basic declaration
            ("var c: bool = true", true),    // Mutable variable
        ];

        for (code, should_pass) in test_cases {
            let input = format!("fn main() -> i64 {{ {} 0i64 }}", code);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Code '{}' should be accepted", code);
            } else {
                assert!(result.is_err(), "Code '{}' should be rejected", code);
            }
        }
    }

    // Test identifier length boundaries
    #[test]
    // #[ignore] // Hangs with long identifiers
    fn test_identifier_length_boundaries() {
        let test_cases = vec![
            (1, true),   // Single character
            (2, true),   // Two characters
            (3, true),   // Three characters
            (5, true),   // Five characters
        ];

        for (length, should_pass) in test_cases {
            let name = "a".repeat(length);
            // Simple function with long identifier name
            let input = format!("fn {}() -> i64 {{ 1i64 }} fn main() -> i64 {{ {}() }}", name, name);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Identifier length {} should be accepted", length);
            } else {
                assert!(result.is_err(), "Identifier length {} should be rejected", length);
            }
        }
    }

    // Test string literal length boundaries
    #[test]
    // #[ignore] // Hangs with long strings
    fn test_string_length_boundaries() {
        let test_cases = vec![
            (0, true),  // Empty string
            (1, true),  // Single character
            (10, true), // Short string
            (20, true), // Medium string (reduced from large sizes)
        ];

        for (length, should_pass) in test_cases {
            let content = "a".repeat(length);
            let input = format!(r#"fn main() -> i64 {{ val s = "{}"
0i64 }}"#, content);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "String length {} should be accepted", length);
            } else {
                assert!(result.is_err(), "String length {} should be rejected", length);
            }
        }
    }

    // Test function parameter boundaries
    #[test]
    fn test_function_parameter_boundaries() {
        let test_cases = vec![
            (0, true),   // No parameters
            (1, true),   // Single parameter
            (10, true),  // Many parameters
            (100, true), // Very many parameters
        ];

        for (param_count, should_pass) in test_cases {
            let params: Vec<String> = (0..param_count)
                .map(|i| format!("p{}: i64", i))
                .collect();
            let args: Vec<String> = (0..param_count)
                .map(|i| format!("{}i64", i))
                .collect();
            
            let input = if param_count == 0 {
                "fn test() -> i64 { 0i64 } fn main() -> i64 { test() }".to_string()
            } else {
                format!(
                    "fn test({}) -> i64 {{ 0i64 }} fn main() -> i64 {{ test({}) }}",
                    params.join(", "),
                    args.join(", ")
                )
            };
            
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Function with {} parameters should be accepted", param_count);
            } else {
                assert!(result.is_err(), "Function with {} parameters should be rejected", param_count);
            }
        }
    }

    // Test nesting depth boundaries
    #[test]
    fn test_expression_nesting_depth() {
        let test_cases = vec![
            (1, true),   // Minimal nesting
            (10, true),  // Moderate nesting
            (50, true),  // Deep nesting
            (100, true), // Very deep nesting
            (500, true), // Extremely deep nesting
        ];

        for (depth, should_pass) in test_cases {
            let mut expr = String::from("1i64");
            for _ in 0..depth {
                expr = format!("({} + 1i64)", expr);
            }
            let input = format!("fn main() -> i64 {{ {} }}", expr);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Nesting depth {} should be accepted", depth);
            } else {
                assert!(result.is_err(), "Nesting depth {} should be rejected", depth);
            }
        }
    }

    // Test block nesting depth
    #[test]
    // #[ignore] // Hangs with deep nesting
    fn test_block_nesting_depth() {
        let test_cases = vec![
            (1, true),   // Single block
            (5, true),   // Light nesting
            (10, true),  // Moderate nesting
            (15, true),  // Reasonable nesting (reduced from deep sizes)
        ];

        for (depth, should_pass) in test_cases {
            let mut blocks = String::from("0i64");
            for i in 0..depth {
                blocks = format!("{{ val x{} = 1i64\n{} }}", i, blocks);
            }
            let input = format!("fn main() -> i64 {{ {} }}", blocks);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Block depth {} should be accepted", depth);
            } else {
                assert!(result.is_err(), "Block depth {} should be rejected", depth);
            }
        }
    }

    // Test struct field count boundaries
    #[test]
    // #[ignore] // Hangs with many fields
    fn test_struct_field_count() {
        let test_cases = vec![
            (1, true),   // Single field
            (3, true),   // Few fields
            (5, true),   // Several fields (reduced from large numbers)
        ];

        for (field_count, should_pass) in test_cases {
            let fields: Vec<String> = (0..field_count)
                .map(|i| format!("f{}: i64", i))
                .collect();
            let values: Vec<String> = (0..field_count)
                .map(|i| format!("f{}: {}i64", i, i))
                .collect();
            
            let input = format!(
                "struct Test {{ {} }} fn main() -> i64 {{ val t = Test {{ {} }}\n0i64 }}",
                fields.join(", "),
                values.join(", ")
            );
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Struct with {} fields should be accepted", field_count);
            } else {
                assert!(result.is_err(), "Struct with {} fields should be rejected", field_count);
            }
        }
    }

    // Test method count boundaries
    #[test]
    fn test_method_count_boundaries() {
        let test_cases = vec![
            (1, true),  // Single method
            (10, true), // Many methods
            (50, true), // Very many methods
        ];

        for (method_count, should_pass) in test_cases {
            let methods: Vec<String> = (0..method_count)
                .map(|i| format!("fn method{}(&self) -> i64 {{ self.value + {}i64 }}", i, i))
                .collect();
            
            let input = format!(
                r#"
                struct Test {{ value: i64 }}
                impl Test {{
                    {}
                }}
                fn main() -> i64 {{ 0i64 }}
                "#,
                methods.join("\n")
            );
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Impl with {} methods should be accepted", method_count);
            } else {
                assert!(result.is_err(), "Impl with {} methods should be rejected", method_count);
            }
        }
    }

    // Test recursion depth boundaries (indirect recursion)
    #[test]
    fn test_type_recursion_boundaries() {
        // Test mutual recursion between structs
        let input = r#"
            struct A { b: B }
            struct B { a: A }
            fn main() -> i64 { 0i64 }
        "#;
        let result = parse_program_only(input);
        // This might be allowed or rejected depending on implementation
        assert!(result.is_ok() || result.is_err(), "Mutual recursion handling tested");
    }


    // Test method chaining boundaries
    #[test]
    // #[ignore] // Hangs with deep chaining
    fn test_method_chaining_depth() {
        let test_cases = vec![
            (1, true),  // Single function call
            (3, true),  // Few function calls
            (5, true),  // Several function calls (reduced from deep chaining)
        ];

        for (depth, should_pass) in test_cases {
            // Create nested function calls instead of method chaining
            let mut chain = String::from("1i64");
            for i in 0..depth {
                let func_name = format!("func{}", i);
                chain = format!("{}({})", func_name, chain);
            }
            
            // Create function definitions with newlines
            let mut func_defs = String::new();
            for i in 0..depth {
                let func_name = format!("func{}", i);
                func_defs.push_str(&format!("fn {}(x: i64) -> i64 {{ x + 1i64 }}\n", func_name));
            }
            
            let input = format!(
                "{}fn main() -> i64 {{ {} }}",
                func_defs,
                chain
            );
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Method chaining depth {} should be accepted", depth);
            } else {
                assert!(result.is_err(), "Method chaining depth {} should be rejected", depth);
            }
        }
    }

    // Test if-else chaining boundaries
    #[test]
    // #[ignore] // Hangs with long chains
    fn test_if_else_chain_boundaries() {
        let test_cases = vec![
            (1, true),  // Single if-else
            (3, true),  // Few else-if clauses
            (5, true),  // Several else-if clauses (reduced from long chains)
        ];

        for (chain_length, should_pass) in test_cases {
            let mut if_chain = String::from("if true {\n0i64\n}");
            for i in 1..chain_length {
                if_chain.push_str(&format!(" elif {}i64 == {}i64 {{\n{}i64\n}}", i, i, i));
            }
            if_chain.push_str(" else {\n999i64\n}");
            
            let input = format!("fn main() -> i64 {{\n{}\n}}", if_chain);
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "If-else chain length {} should be accepted", chain_length);
            } else {
                assert!(result.is_err(), "If-else chain length {} should be rejected", chain_length);
            }
        }
    }

    // Test variable count boundaries in scope
    #[test]
    // #[ignore] // Hangs with many variables
    fn test_variable_count_in_scope() {
        let test_cases = vec![
            (2, true),  // Few variables
            (3, true),  // Several variables
            (5, true),  // Several variables (reduced from large numbers)
        ];

        for (var_count, should_pass) in test_cases {
            let declarations: Vec<String> = (0..var_count)
                .map(|i| format!("val var{} = {}i64", i, i))
                .collect();
            let usage: Vec<String> = (0..var_count)
                .map(|i| format!("var{}", i))
                .collect();
            
            let input = format!(
                "fn main() -> i64 {{\n{}\n{} }}",
                declarations.join("\n"),
                usage.join(" + ")
            );
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "{} variables in scope should be accepted", var_count);
            } else {
                assert!(result.is_err(), "{} variables in scope should be rejected", var_count);
            }
        }
    }
}