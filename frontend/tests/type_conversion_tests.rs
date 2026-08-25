//! Type Conversion Tests
//!
//! Tests for the type conversion and numeric type resolution subsystem.
//! Covers numeric literal conversion, type resolution between Number/u64/i64,
//! type mismatch detection, implicit conversions, and error cases.
//!
//! Target: src/type_checker/type_conversion.rs (479 lines, 0 existing tests)

use frontend::ParserWithInterner;
use frontend::type_checker::TypeCheckerVisitor;

mod helpers {
    use super::*;

    /// Helper function to parse and type-check source code
    pub fn parse_and_check(source: &str) -> Result<(), String> {
        let mut parser = ParserWithInterner::new(source);
        match parser.parse_program() {
            Ok(mut program) => {
                if program.statement.is_empty() && program.function.is_empty() {
                    return Err("No statements or functions found".to_string());
                }

                let functions = program.function.clone();
                let string_interner = parser.get_string_interner();
                let mut type_checker = TypeCheckerVisitor::with_program(&mut program, string_interner);
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

mod numeric_literal_conversion {
    //! Tests for bare number literal conversion to concrete types

    use super::helpers::parse_and_check;

    #[test]
    fn test_bare_number_with_u64_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_resolved_by_arithmetic_context() {
        let source = r#"
            fn main() -> u64 {
                val x = 42
                val y = 1u64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_resolved_by_return_type() {
        // NUMBER-HINT: the tail expression *is* the return value, so
        // the declared return type claims an unsuffixed literal that
        // reached it through a binding. This used to be an error
        // ("expected u64, but got Number") because no position ever
        // told the literal what to be.
        let source = r#"
            fn main() -> u64 {
                val x = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_resolved_by_signed_return_type() {
        // Same position, opposite signedness: the return type decides,
        // not the u64 default.
        let source = r#"
            fn main() -> i64 {
                val x = 0i64
                val y = 42
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_argument_takes_parameter_type() {
        // NUMBER-HINT: a literal argument becomes the parameter's
        // type. `f(21)` used to be rejected as `u64` against `i64`
        // because a *previously checked* function's finalization pass
        // had already frozen the literal.
        let source = r#"
            fn f(x: i64) -> i64 { x * 2i64 }
            fn main() -> u64 {
                println(f(21))
                0
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_argument_takes_narrow_parameter_type() {
        let source = r#"
            fn f(x: i8) -> i8 { x }
            fn main() -> u64 {
                println(f(3))
                0
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_argument_out_of_narrow_range_is_rejected() {
        // The coercion range-checks: 300 does not fit a u8, and that
        // is a conversion error rather than a silent wrap.
        let source = r#"
            fn f(x: u8) -> u8 { x }
            fn main() -> u64 {
                println(f(300))
                0
            }
        "#;
        assert!(parse_and_check(source).is_err());
    }

    #[test]
    fn test_annotated_sibling_does_not_retype_an_unannotated_literal() {
        // NUMBER-HINT: a pre-scan used to walk the body for the first
        // `val x: i64` / `val x: u64` and make that annotation the
        // numeric hint for the *whole function*, so an unrelated
        // sibling binding decided the type of every unsuffixed literal
        // after it. Here `b`'s `i64` made `a + 1` signed, and the u64
        // return type then rejected the body.
        let source = r#"
            fn main() -> u64 {
                val a = 42
                val b: i64 = 10
                val d = a + 1
                println(b)
                d
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_annotated_sibling_before_does_not_retype_either() {
        // Same check with the annotated binding first — the old
        // pre-scan was order-sensitive, so both orders are pinned.
        let source = r#"
            fn main() -> u64 {
                val b: i64 = 10
                val a = 42
                println(b)
                a + 1
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_every_position_that_names_a_type_claims_its_literals() {
        // NUMBER-HINT: the set of positions is the whole point — one
        // that knows what it expects but does not claim the literal
        // forces a suffix, and leaks the internal `Number` placeholder
        // into its diagnostic when it mismatches. Each line here was a
        // separate hole. (The impl-block positions — method and
        // associated-function arguments — need the interpreter's
        // driver, so they are pinned in
        // `interpreter/tests/language_core_tests.rs` instead, which
        // also *runs* the program.)
        let source = r#"
            struct P { x: i64 }
            struct B<T> { v: T }
            enum E { V(i64) }

            fn take(n: i64) -> i64 { n }

            fn main() -> u64 {
                val p = P { x: 5 }                      # struct field
                val b: B<i64> = B { v: 5 }              # generic struct field
                val arr: [i64; 3] = [1, 2, 3]           # annotated array element
                val sib = [1i64, 2, 3]                  # sibling-typed array element
                val tup: (i64, i64) = (1, 2)            # tuple element
                val dic: dict[str, i64] = dict{"a": 1}  # dict value
                val e = E::V(5)                         # enum payload
                val clo = fn(x: i64) -> i64 { x }       # closure parameter
                val tail = fn() -> i64 { 5 }            # closure body tail
                val called = take(5)                    # function argument

                var m: i64 = 0i64
                m = 5                                   # assignment
                m += 5                                  # compound assignment

                var q = P { x: 0i64 }
                q.x = 5                                 # field assignment

                # `b.v` is read in the interpreter-side test instead: this
                # helper never visits struct declarations, so a generic
                # field access still reports `Generic(T)` here.
                println(p.x + arr[0] + sib[0] + tup.0 + m + q.x + tail() + clo(5) + called)
                println(dic)
                println(e)
                0
            }
        "#;
        assert_eq!(parse_and_check(source), Ok(()));
    }

    #[test]
    fn test_a_claimed_literal_is_rewritten_not_just_retyped() {
        // A position that reports the resolved type but leaves an
        // `Expr::Number` in the pool type-checks a program no backend
        // can run. The branch tails of an `if` / `match` and a closure
        // body are the shapes where the literal sits several levels
        // below the expression whose type was claimed.
        let source = r#"
            fn branch(n: i64) -> i64 { if n > 0i64 { 1 } elif n > 5i64 { 2 } else { 3 } }
            fn arm(n: i64) -> i64 { match n { 0i64 => 1, _ => 2 } }
            fn main() -> u64 {
                val c = fn(x: i64) -> i64 { if x > 0i64 { 1 } else { 2 } }
                println(branch(1i64) + arm(0i64) + c(1i64))
                0
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_in_explicit_return() {
        // `return 0` names the same position as the tail expression.
        let source = r#"
            fn f(n: i64) -> i64 {
                if n > 0i64 {
                    return 1
                }
                0
            }
            fn main() -> u64 {
                println(f(5i64))
                0
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_bare_number_with_i64_hint() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_u64_suffix_literal() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_suffix_literal() {
        let source = r#"
            fn main() -> i64 {
                val x = 42i64
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_hex_literal_with_u64_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 0xFF
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_zero_literal_with_annotation() {
        let source = r#"
            fn main() -> u64 {
                val x: u64 = 0
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod numeric_type_resolution {
    //! Tests for resolve_numeric_types: Number+concrete type interactions

    use super::helpers::parse_and_check;

    #[test]
    fn test_number_plus_u64_resolves_to_u64() {
        let source = r#"
            fn main() -> u64 {
                val x = 10
                val y = 20u64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_i64_resolves_to_i64() {
        let source = r#"
            fn main() -> i64 {
                val x = 10
                val y = 20i64
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_number_defaults_to_u64() {
        let source = r#"
            fn main() -> u64 {
                val x = 10
                val y = 20
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_plus_number_with_i64_return_hint() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 10
                val y: i64 = 20
                x + y
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_number_in_arithmetic_expression() {
        let source = r#"
            fn main() -> u64 {
                val a = 5
                val b = 10u64
                val c = a * b + 3
                c
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod type_mismatch_errors {
    //! Tests for type mismatch detection in numeric operations

    use super::helpers::parse_and_check;

    #[test]
    fn test_u64_plus_i64_mixed_error() {
        let source = r#"
            fn main() -> u64 {
                val x = 10u64
                val y = 20i64
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Mixing u64 and i64 in arithmetic should fail");
    }

    #[test]
    fn test_bool_arithmetic_error() {
        let source = r#"
            fn main() -> bool {
                val x = true
                val y = false
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool arithmetic should fail");
    }

    #[test]
    fn test_string_plus_u64_error() {
        let source = r#"
            fn main() -> u64 {
                val x = "hello"
                val y = 10u64
                x + y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String + u64 should fail");
    }
}

mod implicit_conversion {
    //! Tests for implicit type conversion in various contexts

    use super::helpers::parse_and_check;

    #[test]
    fn test_val_declaration_implicit_conversion() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 42
                x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_function_argument_conversion() {
        let source = r#"
            fn take_i64(x: i64) -> i64 {
                x
            }

            fn main() -> i64 {
                take_i64(42i64)
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_field_number_conversion() {
        let source = r#"
            struct Point {
                x: u64,
                y: u64
            }

            fn main() -> u64 {
                val p = Point { x: 10, y: 20 }
                p.x
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_struct_field_i64_number_conversion() {
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
    fn test_array_element_implicit_conversion() {
        let source = r#"
            fn main() -> u64 {
                val arr: [u64; 3] = [1, 2, 3]
                arr[0u64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }

    #[test]
    fn test_i64_array_implicit_conversion() {
        let source = r#"
            fn main() -> i64 {
                val arr: [i64; 3] = [1, 2, 3]
                arr[0i64]
            }
        "#;
        assert!(parse_and_check(source).is_ok());
    }
}

mod type_conversion_errors {
    //! Tests for conversion error cases

    use super::helpers::parse_and_check;

    #[test]
    fn test_bool_to_u64_assignment_error() {
        let source = r#"
            fn main() -> u64 {
                val x = true
                val y: u64 = x
                y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool to u64 assignment should fail");
    }

    #[test]
    fn test_string_to_i64_assignment_error() {
        let source = r#"
            fn main() -> i64 {
                val x = "hello"
                val y: i64 = x
                y
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String to i64 assignment should fail");
    }

    #[test]
    fn test_bool_to_function_param_error() {
        let source = r#"
            fn take_u64(x: u64) -> u64 {
                x
            }

            fn main() -> u64 {
                take_u64(true)
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "Bool passed as u64 param should fail");
    }

    #[test]
    fn test_wrong_return_type_error() {
        let source = r#"
            fn main() -> u64 {
                val x = "hello"
                x
            }
        "#;
        let result = parse_and_check(source);
        assert!(result.is_err(), "String returned as u64 should fail");
    }
}
