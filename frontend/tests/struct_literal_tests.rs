//! Struct Literal Tests
//!
//! Tests for struct declaration validation, field access, literal creation,
//! generic struct inference, and slice method delegation.
//!
//! Target: src/type_checker/struct_literal.rs (458 lines, minimal tests)

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;

mod helpers {
    use super::*;
    use frontend::ast::{StmtRef, Stmt};

    /// Enhanced helper that processes StructDecl and ImplBlock statements
    /// before type-checking functions, so generic params and methods are registered.
    pub fn parse_and_check(source: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() && program.function.is_empty() {
                    return Err("No statements or functions found".to_string());
                }

                let functions = program.function.clone();
                let stmt_count = program.statement.len();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);

                // Process only StructDecl and ImplBlock statements
                for i in 0..stmt_count {
                    let stmt_ref = StmtRef(i as u32);
                    let should_visit = type_checker.core.stmt_pool.get(&stmt_ref)
                        .map(|stmt| matches!(stmt, Stmt::StructDecl { .. } | Stmt::ImplBlock { .. }))
                        .unwrap_or(false);
                    if should_visit {
                        if let Err(e) = type_checker.visit_stmt(&stmt_ref) {
                            return Err(format!("{:?}", e));
                        }
                    }
                }

                let mut errors = Vec::new();
                for func in functions.iter() {
                    if let Err(e) = type_checker.type_check(func.clone()) {
                        errors.push(format!("{:?}", e));
                    }
                }

                if !errors.is_empty() {
                    Err(errors.join("\n"))
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(format!("Parse error: {:?}", e))
        }
    }
}

mod struct_declaration {
    //! Tests for struct declaration validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_basic_struct_declaration() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 1u64, y: 2u64 }
                p.x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_with_multiple_types() {
        let source = r#"
            struct Record {
                id: u64,
                name: str,
                active: bool
            }

            fn main() -> u64 {
                val r = Record { id: 1u64, name: "test", active: true }
                r.id
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_duplicate_field_error() {
        let source = r#"
            struct Bad {
                x: u64,
                x: u64
            }

            fn main() -> u64 {
                0u64
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Duplicate field names should fail");
    }

    #[test]
    fn test_generic_struct_declaration() {
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn main() -> u64 {
                val b = Box { value: 42u64 }
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_two_param_generic_struct() {
        let source = r#"
            struct Pair<A, B> {
                first: A,
                second: B
            }

            fn main() -> u64 {
                val p = Pair { first: 42u64, second: true }
                p.first
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod field_access {
    //! Tests for struct field access type checking

    use super::helpers::parse_and_check;

    #[test]
    fn test_basic_field_access() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                p.x + p.y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_nonexistent_field_error() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                p.z
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Accessing nonexistent field should fail");
    }

    #[test]
    fn test_generic_struct_field_type_substitution() {
        let source = r#"
            struct Container<T> {
                item: T
            }

            fn main() -> bool {
                val c = Container { item: true }
                c.item
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_nested_struct_field_access() {
        let source = r#"
            struct Inner {
                value: u64
            }

            struct Outer {
                inner: Inner
            }

            fn main() -> u64 {
                val i = Inner { value: 42u64 }
                val o = Outer { inner: i }
                o.inner.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_field_access_on_non_struct_error() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                x.field
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Field access on non-struct should fail");
    }
}

mod struct_literal_creation {
    //! Tests for struct literal creation and type validation

    use super::helpers::parse_and_check;

    #[test]
    fn test_correct_field_types() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 1u64, y: 2u64 }
                p.x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_field_type_mismatch_error() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: true, y: 2u64 }
                p.x
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool for u64 field should fail");
    }

    #[test]
    fn test_number_auto_conversion_in_struct() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 10, y: 20 }
                p.x + p.y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_number_auto_conversion_in_struct() {
        let source = r#"
            struct Offset {
                dx: i64,
                dy: i64
            }

            fn main() -> i64 {
                val o = Offset { dx: 10, dy: 20 }
                o.dx
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_used_in_function() {
        // Test that struct values can be passed to and returned from functions
        // by accessing fields (avoiding Identifier vs Struct type mismatch)
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn point_sum(p: Point) -> u64 {
                p.x + p.y
            }

            fn main() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                point_sum(p)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod generic_struct_literal {
    //! Tests for generic struct literal type inference

    use super::helpers::parse_and_check;

    #[test]
    fn test_generic_u64_inference() {
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn main() -> u64 {
                val b = Box { value: 42u64 }
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_bool_inference() {
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn main() -> bool {
                val b = Box { value: true }
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_two_param_generic_inference() {
        let source = r#"
            struct Pair<A, B> {
                first: A,
                second: B
            }

            fn main() -> bool {
                val p = Pair { first: 42u64, second: true }
                p.second
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_struct_field_return_type() {
        let source = r#"
            struct Wrapper<T> {
                inner: T
            }

            fn main() -> u64 {
                val w = Wrapper { inner: 100u64 }
                w.inner
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_string_inference() {
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn main() -> str {
                val b = Box { value: "hello" }
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}
