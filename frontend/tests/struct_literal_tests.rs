//! Struct Literal Tests
//!
//! Tests for struct declaration validation, field access, literal creation,
//! generic struct inference, and slice method delegation.
//!
//! Target: src/type_checker/struct_literal.rs (458 lines, minimal tests)


use crate::common::type_check_with_declarations as parse_and_check;

mod struct_declaration {
    //! Tests for struct declaration validation

    use super::parse_and_check;

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

    use super::parse_and_check;

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

    use super::parse_and_check;

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

    use super::parse_and_check;

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
