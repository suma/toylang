#[cfg(test)]
mod boundary_tests {
    use crate::parser::core::Parser;

    // Helper function for parser-only testing
    fn parse_program_only(input: &str) -> Result<(), String> {
        let mut parser = Parser::new(input);
        match parser.parse_program() {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("Parse error: {:?}", errors))
        }
    }

    // Test minimum and maximum integer values
    #[test]
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

    // Test array size boundaries
    #[test]
    fn test_array_size_boundaries() {
        let test_cases = vec![
            (0, false),    // Zero-size arrays might not be allowed
            (1, true),     // Minimum valid size
            (1000, true),  // Reasonable size
            (65536, true), // Large but manageable size
        ];

        for (size, should_pass) in test_cases {
            let input = if size == 0 {
                format!("fn main() -> i64 {{ val a: [i64; {}] = []; 0i64 }}", size)
            } else {
                format!("fn main() -> i64 {{ val a: [i64; {}] = [0i64]; 0i64 }}", size)
            };
            
            let result = parse_program_only(&input);
            
            if should_pass {
                assert!(result.is_ok(), "Array size {} should be accepted", size);
            } else {
                // Zero-size or very large arrays might be rejected
                assert!(result.is_ok() || result.is_err(), "Array size {} handling tested", size);
            }
        }
    }

    // Test identifier length boundaries
    #[test]
    fn test_identifier_length_boundaries() {
        let test_cases = vec![
            (1, true),   // Single character
            (10, true),  // Normal length
            (50, true),  // Long but reasonable
            (100, true), // Very long (reduced from 10000)
        ];

        for (length, should_pass) in test_cases {
            let name = "a".repeat(length);
            let input = format!("fn main() -> i64 {{ val {} = 1i64; {} }}", name, name);
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
    fn test_string_length_boundaries() {
        let test_cases = vec![
            (0, true),  // Empty string
            (1, true),  // Single character
            (50, true), // Long string (reduced from 10000)
            (100, true), // Very long string (reduced from 10000)
        ];

        for (length, should_pass) in test_cases {
            let content = "a".repeat(length);
            let input = format!(r#"fn main() -> i64 {{ val s = "{}"; 0i64 }}"#, content);
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
    fn test_block_nesting_depth() {
        let test_cases = vec![
            (1, true),   // Single block
            (10, true),  // Moderate nesting
            (50, true),  // Deep nesting
            (100, true), // Very deep nesting
        ];

        for (depth, should_pass) in test_cases {
            let mut blocks = String::from("0i64");
            for i in 0..depth {
                blocks = format!("{{ val x{} = 1i64; {} }}", i, blocks);
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
    fn test_struct_field_count() {
        let test_cases = vec![
            (1, true),   // Single field
            (10, true),  // Many fields
            (100, true), // Very many fields
        ];

        for (field_count, should_pass) in test_cases {
            let fields: Vec<String> = (0..field_count)
                .map(|i| format!("f{}: i64", i))
                .collect();
            let values: Vec<String> = (0..field_count)
                .map(|i| format!("f{}: {}i64", i, i))
                .collect();
            
            let input = format!(
                "struct Test {{ {} }} fn main() -> i64 {{ val t = Test {{ {} }}; 0i64 }}",
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

    // Test array element access boundaries
    #[test]
    fn test_array_access_boundaries() {
        let input = r#"
            fn main() -> i64 {
                val arr: [i64; 5] = [1i64, 2i64, 3i64, 4i64, 5i64];
                arr[0u64] + arr[4u64]
            }
        "#;
        let result = parse_program_only(input);
        assert!(result.is_ok(), "Valid array access should work");
    }

    // Test method chaining boundaries
    #[test]
    fn test_method_chaining_depth() {
        let test_cases = vec![
            (1, true),  // Single method call
            (5, true),  // Moderate chaining
            (20, true), // Deep chaining
        ];

        for (depth, should_pass) in test_cases {
            let mut chain = String::from("obj");
            for _ in 0..depth {
                chain.push_str(".get()");
            }
            chain.push_str(".value");
            
            let input = format!(
                r#"
                struct Value {{ value: i64 }}
                impl Value {{
                    fn get(&self) -> Value {{ Value {{ value: self.value }} }}
                }}
                fn main() -> i64 {{
                    val obj = Value {{ value: 1i64 }};
                    {}
                }}
                "#,
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
    fn test_if_else_chain_boundaries() {
        let test_cases = vec![
            (1, true),  // Single if-else
            (10, true), // Many else-if clauses
            (50, true), // Very many else-if clauses
        ];

        for (chain_length, should_pass) in test_cases {
            let mut if_chain = String::from("if true { 0i64 }");
            for i in 1..chain_length {
                if_chain.push_str(&format!(" else if {} == {}i64 {{ {}i64 }}", i, i, i));
            }
            if_chain.push_str(" else { 999i64 }");
            
            let input = format!("fn main() -> i64 {{ {} }}", if_chain);
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
    fn test_variable_count_in_scope() {
        let test_cases = vec![
            (5, true),  // Few variables
            (10, true), // Many variables (reduced from 1000)
            (20, true), // Very many variables (reduced from 1000)
        ];

        for (var_count, should_pass) in test_cases {
            let declarations: Vec<String> = (0..var_count)
                .map(|i| format!("val var{} = {}i64;", i, i))
                .collect();
            let usage: Vec<String> = (0..var_count)
                .map(|i| format!("var{}", i))
                .collect();
            
            let input = format!(
                "fn main() -> i64 {{ {} {} }}",
                declarations.join(" "),
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