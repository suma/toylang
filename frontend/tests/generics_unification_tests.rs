//! Generics Unification Tests
//!
//! Tests for generic type inference, constraint solving, and unification.
//! Covers basic generic functions, associated functions, unification edge cases,
//! and solution application to nested structures.
//!
//! Target: src/type_checker/generics.rs + inference.rs

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

mod basic_generic_functions {
    //! Tests for basic generic function type inference

    use super::helpers::parse_and_check;

    #[test]
    fn test_identity_u64_inference() {
        let source = r#"
            fn identity<T>(x: T) -> T {
                x
            }

            fn main() -> u64 {
                identity(42u64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_identity_bool_inference() {
        let source = r#"
            fn identity<T>(x: T) -> T {
                x
            }

            fn main() -> bool {
                identity(true)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_identity_str_inference() {
        let source = r#"
            fn identity<T>(x: T) -> T {
                x
            }

            fn main() -> str {
                identity("hello")
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_two_param_generic_function() {
        let source = r#"
            fn first<A, B>(a: A, b: B) -> A {
                a
            }

            fn main() -> u64 {
                first(42u64, true)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_function_wrong_arg_count_error() {
        let source = r#"
            fn identity<T>(x: T) -> T {
                x
            }

            fn main() -> u64 {
                identity(1u64, 2u64)
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Wrong argument count should fail");
    }

    #[test]
    fn test_second_param_generic_function() {
        let source = r#"
            fn second<A, B>(a: A, b: B) -> B {
                b
            }

            fn main() -> bool {
                second(42u64, true)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod generic_struct_instantiation {
    //! Tests for generic struct type inference and instantiation

    use super::helpers::parse_and_check;

    #[test]
    fn test_generic_struct_via_factory_function() {
        // Test creating a generic struct through a standalone generic function
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn make_box<T>(v: T) -> Box<T> {
                Box { value: v }
            }

            fn main() -> u64 {
                val b = make_box(42u64)
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_struct_direct_field_access() {
        // Test that generic struct field access resolves T correctly
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
    fn test_generic_struct_with_bool() {
        let source = r#"
            struct Wrapper<T> {
                data: T
            }

            fn main() -> bool {
                val w = Wrapper { data: true }
                w.data
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod unification_edge_cases {
    //! Tests for type unification edge cases

    use super::helpers::parse_and_check;

    #[test]
    fn test_same_struct_type_unification() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn take_point(p: Point) -> u64 {
                p.x
            }

            fn main() -> u64 {
                val p = Point { x: 10u64, y: 20u64 }
                take_point(p)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_different_struct_type_error() {
        let source = r#"
            struct Foo {
                x: u64
            }

            struct Bar {
                x: u64
            }

            fn take_foo(f: Foo) -> u64 {
                f.x
            }

            fn main() -> u64 {
                val b = Bar { x: 42u64 }
                take_foo(b)
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Passing Bar where Foo is expected should fail");
    }

    #[test]
    fn test_array_type_unification() {
        let source = r#"
            fn take_array(arr: [u64; 3]) -> u64 {
                arr[0u64]
            }

            fn main() -> u64 {
                val a = [1u64, 2u64, 3u64]
                take_array(a)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_same_param_consistent() {
        let source = r#"
            fn equal<T>(a: T, b: T) -> T {
                a
            }

            fn main() -> u64 {
                equal(1u64, 2u64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_same_param_inconsistent_error() {
        let source = r#"
            fn equal<T>(a: T, b: T) -> T {
                a
            }

            fn main() -> u64 {
                equal(1u64, true)
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Inconsistent type for same parameter should fail");
    }
}

mod solution_application {
    //! Tests for applying generic solutions to complex types

    use super::helpers::parse_and_check;

    #[test]
    fn test_nested_generic_struct() {
        let source = r#"
            struct Inner<T> {
                value: T
            }

            struct Outer<T> {
                inner: Inner<T>
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
    fn test_generic_with_multiple_fields_same_param() {
        let source = r#"
            struct Pair<T> {
                first: T,
                second: T
            }

            fn main() -> u64 {
                val p = Pair { first: 10u64, second: 20u64 }
                p.first + p.second
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_generic_function_with_struct_return() {
        let source = r#"
            struct Box<T> {
                value: T
            }

            fn wrap<T>(x: T) -> Box<T> {
                Box { value: x }
            }

            fn main() -> u64 {
                val b = wrap(42u64)
                b.value
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_chained_generic_calls() {
        let source = r#"
            fn identity<T>(x: T) -> T {
                x
            }

            fn main() -> u64 {
                val a = identity(42u64)
                val b = identity(a)
                b
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}
