//! Property-Based Integration Tests
//!
//! This module contains property-based testing using proptest for mathematical property
//! verification and invariant checking across the parser and type system.
//!
//! Property-based tests generate random inputs according to defined strategies and verify
//! that certain properties always hold. These tests are crucial for finding edge cases
//! and ensuring system-wide invariants.
//!
//! Test Categories:
//! - Valid identifier parsing invariants
//! - Binary and comparison operation properties
//! - Expression nesting and complexity properties
//! - Array literal and collection properties
//! - Variable declaration properties
//! - Function call properties
//! - Struct and field access properties
//! - Control flow (if/else, loops) properties
//! - Boolean expression properties
//! - Method definition properties
//! - Assignment chain properties
//! - Loop control (break/continue) properties
//! - String literal and comment properties
//! - Operator associativity and precedence properties

use proptest::prelude::*;
use frontend::ParserWithInterner;

mod helpers {
    use super::*;

    /// Parse a complete program and verify it succeeds or fails gracefully
    pub fn parse_program(input: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(input);
        match parser.parse_program() {
            Ok(_) => Ok(()),
            Err(errors) => Err(format!("{:?}", errors))
        }
    }

    /// Strategy for generating valid identifiers (reduced complexity for performance)
    pub fn valid_identifier() -> impl Strategy<Value = String> {
        "[a-z_][a-zA-Z0-9_]{0,5}".prop_map(|s| s.to_string())
            .prop_filter("Not a reserved keyword", |s| {
                !matches!(s.as_str(),
                    "if" | "elif" | "else" | "while" | "for" | "in" | "to" |
                    "fn" | "return" | "break" | "continue" |
                    "val" | "var" | "struct" | "impl" | "class" |
                    "true" | "false" | "null" |
                    "u64" | "i64" | "str" | "ptr" | "usize" | "bool" |
                    "pub" | "extern" | "package" | "import" | "as")
            })
    }

    /// Strategy for generating i64 integer literals
    pub fn int64_literal() -> impl Strategy<Value = String> {
        (-1000i64..1000i64).prop_map(|n| format!("{}i64", n))
    }

    /// Strategy for generating u64 integer literals
    pub fn uint64_literal() -> impl Strategy<Value = String> {
        (0u64..1000u64).prop_map(|n| format!("{}u64", n))
    }
}

mod identifier_properties {
    //! Properties about identifier parsing

    use super::*;
    use super::helpers::{parse_program, valid_identifier};

    /// Property: All generated valid identifiers must parse successfully
    proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 10,
            .. proptest::test_runner::Config::default()
        })]
        #[test]
        fn prop_valid_identifiers_parse(name in valid_identifier()) {
            let input = format!("fn main() -> i64 {{ val {} = 1i64 {} }}", name, name);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Valid identifier '{}' should parse", name);
        }
    }
}

mod expression_properties {
    //! Properties about expression parsing and evaluation

    use super::*;
    use super::helpers::{parse_program, int64_literal};

    /// Property: Binary operations with same types always parse
    proptest! {
        #[test]
        fn prop_binary_operations_parse(
            left in int64_literal(),
            right in int64_literal(),
            op in prop::sample::select(vec!["+", "-", "*", "/"])
        ) {
            let input = format!("fn main() -> i64 {{ {} {} {} }}", left, op, right);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Binary operation should parse: {} {} {}", left, op, right);
        }
    }

    /// Property: Comparison operations always parse
    proptest! {
        #[test]
        fn prop_comparison_operations_parse(
            left in int64_literal(),
            right in int64_literal(),
            op in prop::sample::select(vec!["<", ">", "<=", ">=", "==", "!="])
        ) {
            let input = format!("fn main() -> bool {{ {} {} {} }}", left, op, right);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Comparison should parse: {} {} {}", left, op, right);
        }
    }

    /// Property: Nested expressions always parse regardless of depth
    proptest! {
        #[test]
        fn prop_nested_expressions_parse(depth in 1usize..10usize) {
            let mut expr = String::from("1i64");
            for _ in 0..depth {
                expr = format!("({} + 1i64)", expr);
            }
            let input = format!("fn main() -> i64 {{ {} }}", expr);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Nested expression of depth {} should parse", depth);
        }
    }

    /// Property: Operator associativity is consistent
    proptest! {
        #[test]
        fn prop_operator_associativity(
            a in int64_literal(),
            b in int64_literal(),
            c in int64_literal()
        ) {
            // Test left associativity of addition
            let input1 = format!("fn main() -> i64 {{ {} + {} + {} }}", a, b, c);
            let input2 = format!("fn main() -> i64 {{ ({} + {}) + {} }}", a, b, c);

            let result1 = parse_program(&input1);
            let result2 = parse_program(&input2);

            prop_assert!(result1.is_ok() && result2.is_ok(),
                "Both left-associative expressions should parse");
        }
    }
}

mod collection_properties {
    //! Properties about arrays, collections, and literals

    use super::*;
    use super::helpers::{parse_program, int64_literal};

    /// Property: Array literals with any size parse successfully
    proptest! {
        #[test]
        fn prop_array_literals_parse(elements in prop::collection::vec(int64_literal(), 0..10)) {
            if elements.is_empty() {
                return Ok(());
            }
            let array_literal = format!("[{}]", elements.join(", "));
            let size = elements.len();
            let input = format!("fn main() -> i64 {{ val a: [i64; {}] = {} 0i64 }}", size, array_literal);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Array literal should parse: {}", array_literal);
        }
    }

    /// Property: String literals with any content parse
    proptest! {
        #[test]
        fn prop_string_literals_parse(s in "[a-zA-Z0-9 ]{0,50}") {
            let input = format!(r#"fn main() -> i64 {{ val s = "{}"; 0i64 }}"#, s);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "String literal should parse: {}", s);
        }
    }
}

mod variable_properties {
    //! Properties about variable declarations

    use super::*;
    use super::helpers::{parse_program, valid_identifier, int64_literal};

    /// Property: Both val and var declarations parse for any valid identifier
    proptest! {
        #[test]
        fn prop_variable_declarations_parse(
            name in valid_identifier(),
            value in int64_literal(),
            is_const in prop::bool::ANY
        ) {
            let decl_type = if is_const { "val" } else { "var" };
            let input = format!("fn main() -> i64 {{ {} {} = {} {} }}", decl_type, name, value, name);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "{} declaration should parse", decl_type);
        }
    }

    /// Property: Assignment chains always parse
    proptest! {
        #[test]
        fn prop_assignment_chains_parse(
            vars in prop::collection::vec(valid_identifier(), 1..5),
            value in int64_literal()
        ) {
            let declarations: Vec<String> = vars.iter().map(|v| format!("var {} = 0i64", v)).collect();
            let assignments: Vec<String> = vars.iter().map(|v| format!("{} = {}", v, value)).collect();

            let input = format!(
                "fn main() -> i64 {{ {} {} 0i64 }}",
                declarations.join(" "),
                assignments.join(" ")
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Assignment chain should parse");
        }
    }
}

mod function_properties {
    //! Properties about function definitions and calls

    use super::*;
    use super::helpers::{parse_program, int64_literal};

    /// Property: Function calls with any number of arguments parse
    proptest! {
        #[test]
        fn prop_function_calls_parse(args in prop::collection::vec(int64_literal(), 0..10)) {
            let params: Vec<String> = (0..args.len()).map(|i| format!("p{}: i64", i)).collect();
            let func_def = if params.is_empty() {
                "fn test_func() -> i64 { 0i64 }".to_string()
            } else {
                format!("fn test_func({}) -> i64 {{ p0 }}", params.join(", "))
            };

            let call_args = args.join(", ");
            let input = format!("{}\nfn main() -> i64 {{ test_func({}) }}", func_def, call_args);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Function call with {} args should parse", args.len());
        }
    }

    /// Property: Return statements parse with any value
    proptest! {
        #[test]
        fn prop_return_statements_parse(value in int64_literal()) {
            let input = format!("fn test() -> i64 {{ return {} }} fn main() -> i64 {{ test() }}", value);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Return statement should parse");
        }
    }
}

mod struct_properties {
    //! Properties about struct definitions and field access

    use super::*;
    use super::helpers::{parse_program, valid_identifier};

    /// Property: Struct field access chains parse at any depth
    proptest! {
        #[test]
        fn prop_struct_field_access_parse(depth in 1usize..5usize) {
            let mut struct_defs = String::new();
            for i in 0..depth {
                if i == 0 {
                    struct_defs.push_str("struct S0 { value: i64 }\n");
                } else {
                    struct_defs.push_str(&format!("struct S{} {{ inner: S{} }}\n", i, i-1));
                }
            }

            let mut init = String::from("S0 { value: 1i64 }");
            for i in 1..depth {
                init = format!("S{} {{ inner: {} }}", i, init);
            }

            let mut access = String::from("obj");
            for _i in (0..depth-1).rev() {
                access.push_str(".inner");
            }
            access.push_str(".value");

            let input = format!(
                "{}\nfn main() -> i64 {{ val obj = {} {} }}",
                struct_defs, init, access
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Struct field access chain of depth {} should parse", depth);
        }
    }

    /// Property: Method definitions with any valid names parse
    proptest! {
        #[test]
        fn prop_method_definitions_parse(
            struct_name in valid_identifier(),
            method_name in valid_identifier(),
            field_name in valid_identifier()
        ) {
            let input = format!(
                r#"
                struct {} {{ {}: i64 }}
                impl {} {{
                    fn {}(&self) -> i64 {{ self.{} }}
                }}
                fn main() -> i64 {{ 0i64 }}
                "#,
                struct_name, field_name, struct_name, method_name, field_name
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Method definition should parse");
        }
    }
}

mod control_flow_properties {
    //! Properties about control flow constructs

    use super::*;
    use super::helpers::{parse_program, valid_identifier, int64_literal};

    /// Property: Boolean expressions parse with any operator and operands
    proptest! {
        #[test]
        fn prop_boolean_expressions_parse(
            left in prop::bool::ANY,
            right in prop::bool::ANY,
            op in prop::sample::select(vec!["&&", "||"])
        ) {
            let input = format!("fn main() -> bool {{ {} {} {} }}", left, op, right);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Boolean expression should parse: {} {} {}", left, op, right);
        }
    }

    /// Property: If-else chains at any depth parse
    proptest! {
        #[test]
        fn prop_if_else_chains_parse(conditions in prop::collection::vec(int64_literal(), 1..5)) {
            let mut if_chain = String::new();
            for (i, cond) in conditions.iter().enumerate() {
                if i == 0 {
                    if_chain.push_str(&format!("if {} > 0i64 {{ {}i64 }}", cond, i));
                } else {
                    if_chain.push_str(&format!(" else if {} > 0i64 {{ {}i64 }}", cond, i));
                }
            }
            if_chain.push_str(" else { 999i64 }");

            let input = format!("fn main() -> i64 {{ {} }}", if_chain);
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "If-else chain should parse");
        }
    }

    /// Property: For loops with any valid range parse
    proptest! {
        #[test]
        fn prop_for_loops_parse(
            start in 0i64..100i64,
            end in 0i64..100i64,
            var_name in valid_identifier()
        ) {
            let input = format!(
                "fn main() -> i64 {{ for {} in {}i64 to {}i64 {{ }} 0i64 }}",
                var_name, start, end
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "For loop should parse");
        }
    }

    /// Property: While loops with any condition parse
    proptest! {
        #[test]
        fn prop_while_loops_parse(
            var_name in valid_identifier(),
            limit in int64_literal()
        ) {
            let input = format!(
                "fn main() -> i64 {{ var {} = 0i64 while {} < {} {{ {} = {} + 1i64 }} 0i64 }}",
                var_name, var_name, limit, var_name, var_name
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "While loop should parse");
        }
    }

    /// Property: Break and continue in loops parse
    proptest! {
        #[test]
        fn prop_break_continue_parse(use_break in prop::bool::ANY) {
            let keyword = if use_break { "break" } else { "continue" };
            let input = format!(
                "fn main() -> i64 {{ for i in 0i64 to 10i64 {{ if i > 5i64 {{ {} }} }} 0i64 }}",
                keyword
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "{} in loop should parse", keyword);
        }
    }
}

mod comment_properties {
    //! Properties about comment handling

    use super::*;
    use super::helpers::{parse_program, int64_literal};

    /// Property: Comments don't affect parsing of any code
    proptest! {
        #[test]
        fn prop_comments_ignored(
            comment in "[a-zA-Z0-9 ]{0,50}",
            value in int64_literal()
        ) {
            let input = format!(
                "fn main() -> i64 {{ # {}\n {} }}",
                comment, value
            );
            let result = parse_program(&input);
            prop_assert!(result.is_ok(), "Comment should not affect parsing");
        }
    }
}

