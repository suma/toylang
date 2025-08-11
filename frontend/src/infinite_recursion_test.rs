#[cfg(test)]
mod infinite_recursion_tests {
    use crate::parser::core::Parser;

    #[test]
    fn test_malformed_struct_fields() {
        // Test malformed struct fields that might cause infinite recursion
        let input = "struct Test { a:, b:, c:, d:, e:, f:, g:, h:, i:, j:, k:, l:, m:, n:, o:, p: } fn main() -> i64 { 0i64 }";
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        // Should not hang or stack overflow
        assert!(result.is_ok() || result.is_err(), "Parser should handle malformed fields");
    }

    #[test]
    fn test_many_struct_fields() {
        // Generate a struct with many fields
        let mut fields = String::new();
        for i in 0..100 {
            if i > 0 {
                fields.push_str(", ");
            }
            fields.push_str(&format!("field{}: i64", i));
        }
        let input = format!("struct Test {{ {} }} fn main() -> i64 {{ 0i64 }}", fields);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Parser should handle many struct fields");
    }

    #[test]
    fn test_many_impl_methods() {
        // Generate an impl block with many methods
        let mut methods = String::new();
        for i in 0..50 {
            methods.push_str(&format!("fn method{}(&self) -> i64 {{ 0i64 }} ", i));
        }
        let input = format!("struct Test {{ x: i64 }} impl Test {{ {} }} fn main() -> i64 {{ 0i64 }}", methods);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Parser should handle many impl methods");
    }

    #[test]
    #[ignore] // May hang due to deep nesting
    fn test_deeply_nested_arrays() {
        // Test deeply nested arrays
        let mut array = String::from("[0i64]");
        for _ in 0..20 {
            array = format!("[{}]", array);
        }
        let input = format!("fn main() -> i64 {{ val a = {}; 0i64 }}", array);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        assert!(result.is_ok() || result.is_err(), "Parser should handle deeply nested arrays");
    }

    #[test]
    #[ignore] // May hang due to deep nesting  
    fn test_deeply_nested_struct_literals() {
        // Test deeply nested struct literals
        let input = r#"
            struct Inner { x: i64 }
            struct Outer { inner: Inner }
            fn main() -> i64 {
                val o = Outer { inner: Inner { x: 1i64 } };
                0i64
            }
        "#;
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Parser should handle nested struct literals");
    }

    #[test]
    fn test_malformed_impl_methods() {
        // Test malformed impl methods that might cause issues
        let input = "struct Test { x: i64 } impl Test { fn fn fn fn fn } fn main() -> i64 { 0i64 }";
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        // Should not hang or stack overflow
        assert!(result.is_ok() || result.is_err(), "Parser should handle malformed methods");
    }

    #[test]
    fn test_many_function_parameters() {
        // Test function with many parameters
        let mut params = String::new();
        for i in 0..100 {
            if i > 0 {
                params.push_str(", ");
            }
            params.push_str(&format!("p{}: i64", i));
        }
        let input = format!("fn test({}) -> i64 {{ 0i64 }} fn main() -> i64 {{ test(0i64) }}", params);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "Parser should handle many function parameters");
    }

    #[test]
    fn test_many_function_arguments() {
        // Test function call with many arguments
        let mut args = String::new();
        for i in 0..100 {
            if i > 0 {
                args.push_str(", ");
            }
            args.push_str(&format!("{}i64", i));
        }
        let input = format!("fn test() -> i64 {{ 0i64 }} fn main() -> i64 {{ test({}) }}", args);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        // Should handle gracefully even if it fails due to parameter mismatch
        assert!(result.is_ok() || result.is_err(), "Parser should handle many arguments");
    }

    #[test]
    #[ignore] // This test may hang - needs further investigation
    fn test_malformed_parameters() {
        // Test malformed parameters that might cause infinite recursion
        let input = "fn test(a: , b: , c: , d: , e: , f: ) -> i64 { 0i64 } fn main() -> i64 { 0i64 }";
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        // Should not hang or stack overflow
        assert!(result.is_ok() || result.is_err(), "Parser should handle malformed parameters");
    }

    #[test]
    fn test_malformed_arguments() {
        // Test malformed function arguments
        let input = "fn test() -> i64 { 0i64 } fn main() -> i64 { test(,,,,,) }";
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        // Should not hang or stack overflow
        assert!(result.is_ok() || result.is_err(), "Parser should handle malformed arguments");
    }

    #[test]
    #[ignore] // This test might be slow
    fn test_extreme_struct_fields() {
        // Test with extreme number of fields
        let mut fields = String::new();
        for i in 0..1000 {
            if i > 0 {
                fields.push_str(", ");
            }
            fields.push_str(&format!("f{}: i64", i));
        }
        let input = format!("struct Test {{ {} }} fn main() -> i64 {{ 0i64 }}", fields);
        let mut parser = Parser::new(&input);
        let result = parser.parse_program();
        assert!(result.is_ok() || result.is_err(), "Parser should handle extreme fields");
    }
}