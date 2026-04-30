//! Language Core Tests
//!
//! This module contains integration tests for core language features.
//! It validates basic program execution, variable declarations, control flow,
//! and simple function calls across the interpreter.
//!
//! Test Categories:
//! - Basic execution and evaluation
//! - Variable declarations (val/var) with type inference
//! - Control flow (if/else, for, while loops)
//! - Loop control (break, continue)
//! - Function calls and return types
//! - Basic arithmetic and comparisons
//! - Heap memory operations (val with builtins)
//! - Error handling (immutable variable reassignment)

mod common;

use std::collections::HashMap;
use frontend::ast::*;
use string_interner::DefaultStringInterner;
use interpreter::evaluation::{EvaluationContext, EvaluationResult};

mod helpers {
    use compiler_core::CompilerSession;

    /// Execute a test program and return the result
    pub fn execute_test_program(source: &str) -> Result<String, String> {
        let mut session = CompilerSession::new();

        // Parse the program
        let mut program = session.parse_program(source)
            .map_err(|e| format!("Parse error: {:?}", e))?;

        // Type check
        interpreter::check_typing(&mut program, session.string_interner_mut(), Some(source), Some("test"))
            .map_err(|e| format!("Type check error: {:?}", e))?;

        // Execute
        let result = interpreter::execute_program(&program, session.string_interner(), Some(source), Some("test"))
            .map_err(|e| format!("Runtime error: {}", e))?;

        Ok(format!("{:?}", result.borrow()))
    }
}

mod basic_execution {
    //! Basic program execution and evaluation tests

    use super::*;

    #[test]
    fn test_evaluate_integer() {
        let stmt_pool = StmtPool::new();
        let mut expr_pool = ExprPool::new();
        let expr_ref = expr_pool.add(Expr::Int64(42));
        let mut interner = DefaultStringInterner::new();

        let mut ctx = EvaluationContext::new(&stmt_pool, &expr_pool, &mut interner, HashMap::new());
        let result = match ctx.evaluate(&expr_ref) {
            Ok(EvaluationResult::Value(v)) => v,
            Ok(other) => panic!("Expected Value but got {other:?}"),
            Err(e) => panic!("Evaluation failed: {e:?}"),
        };

        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn test_simple_program() {
        let mut parser = frontend::ParserWithInterner::new(r"
        fn main() -> u64 {
            val a = 1u64
            val b = 2u64
            val c = a + b
            c
        }
        ");
        let program = parser.parse_program();
        assert!(program.is_ok(), "Program should parse successfully");

        let program = program.unwrap();
        let string_interner = parser.get_string_interner();

        let res = interpreter::execute_program(&program, string_interner, Some("test"), Some("test.t"));
        assert!(res.is_ok(), "Program should execute successfully");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3, "Expected 1+2=3");
    }

    #[test]
    fn test_i64_basic() {
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val a: i64 = 42i64
            val b: i64 = -10i64
            a + b
        }
        ", 32);
    }

    // Statement-level disambiguation: when `-` appears at the start of a new
    // source line, the parser treats it as unary negation of a new expression
    // rather than binary subtraction continuing the previous statement.

    #[test]
    fn test_modulo_i64() {
        // Truncated remainder, matching Rust's `%`.
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            (17i64 % 5i64) + (-7i64 % 3i64)
        }
        ",
            1,
        );
    }

    #[test]
    fn test_modulo_u64() {
        common::assert_program_result_u64(
            r"
        fn main() -> u64 {
            (100u64 % 7u64) + (255u64 % 16u64)
        }
        ",
            17,
        );
    }

    #[test]
    fn test_compound_assign_arithmetic() {
        // 10 +5 -2 *3 /2 %7 = 5
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            var x: i64 = 10i64
            x += 5i64
            x -= 2i64
            x *= 3i64
            x /= 2i64
            x %= 7i64
            x
        }
        ",
            5,
        );
    }

    #[test]
    fn test_compound_assign_field() {
        common::assert_program_result_i64(
            r"
        struct Point { x: i64, y: i64 }
        fn main() -> i64 {
            var p = Point { x: 10i64, y: 5i64 }
            p.x += 3i64
            p.y *= 4i64
            p.x + p.y
        }
        ",
            33,
        );
    }

    #[test]
    fn test_compound_assign_self_reference() {
        // `x += x` exercises both copies of the duplicated lhs against
        // the same SSA / runtime variable, so the result is doubled.
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            var x: i64 = 7i64
            x += x
            x *= x
            x
        }
        ",
            196,
        );
    }

    // ----- f64 (Float64) tests -----

    #[test]
    fn test_f64_basic_arithmetic() {
        common::assert_program_result_f64(
            r"
        fn main() -> f64 {
            val a: f64 = 3.0f64
            val b: f64 = 2.0f64
            (a + b) * 2.0f64 - 1.0f64
        }
        ",
            9.0,
        );
    }

    #[test]
    fn test_f64_division_and_modulo() {
        common::assert_program_result_f64(
            r"
        fn main() -> f64 {
            7.0f64 % 3.0f64
        }
        ",
            1.0,
        );
    }

    #[test]
    fn test_f64_comparison_returns_bool() {
        // The branch is selected when 1.5 < 2.0 holds, then we read
        // back through an i64 channel so we exercise the full path.
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            val a: f64 = 1.5f64
            val b: f64 = 2.0f64
            if a < b { 1i64 } else { 0i64 }
        }
        ",
            1,
        );
    }

    #[test]
    fn test_f64_unary_minus() {
        common::assert_program_result_f64(
            r"
        fn main() -> f64 {
            val x: f64 = 2.5f64
            -x
        }
        ",
            -2.5,
        );
    }

    #[test]
    fn test_f64_cast_round_trip() {
        // 5 → 5.0 → 5 — exercises both conversion directions.
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            val i: i64 = 5i64
            val f: f64 = i as f64
            (f * 2.0f64) as i64
        }
        ",
            10,
        );
    }

    #[test]
    fn test_f64_cast_truncation() {
        // f64 → i64 truncates toward zero (matching Rust's `as`).
        common::assert_program_result_i64(
            r"
        fn main() -> i64 {
            val pi: f64 = 3.7f64
            pi as i64
        }
        ",
            3,
        );
    }

    #[test]
    fn test_f64_integer_suffix_literal() {
        // `42f64` is shorthand for `42.0f64`; both produce the same value.
        common::assert_program_result_f64(
            r"
        fn main() -> f64 {
            42f64 + 0.5f64
        }
        ",
            42.5,
        );
    }

    // ----- Design-by-Contract tests -----

    #[test]
    fn test_contract_requires_and_ensures_pass() {
        // Both predicates hold; the call returns 5.
        common::assert_program_result_i64(
            r"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures  result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }
        ",
            5,
        );
    }

    #[test]
    fn test_contract_requires_violation_at_runtime() {
        // Calling with b == 0 trips `requires b != 0i64`.
        let source = r"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }
        fn main() -> i64 { divide(1i64, 0i64) }
        ";
        let result = common::test_program(source);
        let err = result.expect_err("expected requires violation");
        let msg = err.to_string();
        assert!(
            msg.contains("Contract violation") && msg.contains("requires") && msg.contains("divide"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_contract_ensures_violation_at_runtime() {
        // Implementation lies: returns -x for any x, violating ensures.
        let source = r"
        fn buggy_abs(x: i64) -> i64
            ensures result >= 0i64
        {
            -x
        }
        fn main() -> i64 { buggy_abs(5i64) }
        ";
        let result = common::test_program(source);
        let err = result.expect_err("expected ensures violation");
        let msg = err.to_string();
        assert!(
            msg.contains("Contract violation") && msg.contains("ensures") && msg.contains("buggy_abs"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_contract_multiple_requires_clauses_index() {
        // Three `requires`; only the third (`x <= hi`) fails. The error
        // message must report the third clause specifically (clause #3).
        let source = r"
        fn between(x: i64, lo: i64, hi: i64) -> i64
            requires lo <= hi
            requires lo <= x
            requires x <= hi
        {
            x
        }
        fn main() -> i64 { between(20i64, 0i64, 10i64) }
        ";
        let result = common::test_program(source);
        let err = result.expect_err("expected requires violation");
        let msg = err.to_string();
        assert!(
            msg.contains("requires") && msg.contains("#3") && msg.contains("between"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_contract_method_with_self_and_result() {
        common::assert_program_result_i64(
            r"
        struct Counter { n: i64 }
        impl Counter {
            fn inc(self: Self) -> Self
                requires self.n >= 0i64
                ensures  result.n == self.n + 1i64
            {
                Counter { n: self.n + 1i64 }
            }
        }
        fn main() -> i64 {
            val c = Counter { n: 41i64 }
            val c2 = c.inc()
            c2.n
        }
        ",
            42,
        );
    }

    #[test]
    fn test_contract_non_bool_clause_is_type_error() {
        // `requires b` is i64-typed, not bool — must be rejected at type check.
        let source = r"
        fn divide(a: i64, b: i64) -> i64
            requires b
        {
            a / b
        }
        fn main() -> i64 { divide(1i64, 1i64) }
        ";
        let result = common::test_program(source);
        let err = result.expect_err("expected type error on non-bool requires clause");
        let msg = err.to_string();
        assert!(
            msg.contains("requires") && msg.contains("bool"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_unary_minus_on_variable() {
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val x: i64 = 7i64
            val y: i64 = -x
            y
        }
        ", -7);
    }

    #[test]
    fn test_unary_minus_on_expression() {
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val a: i64 = 10i64
            val b: i64 = 3i64
            val y: i64 = -(a - b)
            y
        }
        ", -7);
    }

    #[test]
    fn test_unary_minus_double_negation_identity() {
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val x: i64 = 42i64
            val y: i64 = -(-x)
            y
        }
        ", 42);
    }

    #[test]
    fn test_unary_minus_on_function_call() {
        common::assert_program_result_i64(r"
        fn seven() -> i64 {
            7i64
        }

        fn main() -> i64 {
            val y: i64 = -seven()
            y
        }
        ", -7);
    }

    #[test]
    fn test_unary_minus_on_untyped_number_coerces_to_i64() {
        // A bare `-5` has operand type Number; the negate operator resolves
        // it to Int64 so the program returns a signed value.
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val y: i64 = -5
            y
        }
        ", -5);
    }

    // ----- top-level `const` declarations -----

    #[test]
    fn test_const_basic_usage() {
        common::assert_program_result_u64(
            r"
        const ANSWER: u64 = 42u64
        fn main() -> u64 { ANSWER }
        ",
            42,
        );
    }

    #[test]
    fn test_const_multiple_referenced_in_function() {
        // Two consts in declaration order, both referenced from a non-main
        // function — verifies consts persist across function-body scopes.
        common::assert_program_result_i64(
            r"
        const TWO: i64 = 2i64
        const THREE: i64 = 3i64
        fn add(a: i64) -> i64 { a + TWO + THREE }
        fn main() -> i64 { add(5i64) }
        ",
            10,
        );
    }

    #[test]
    fn test_const_f64_value() {
        common::assert_program_result_f64(
            r"
        const PI: f64 = 3.14f64
        fn main() -> f64 { PI * 2.0f64 }
        ",
            6.28,
        );
    }

    #[test]
    fn test_const_initializer_can_reference_earlier_const() {
        // `B` references `A` already in scope — declaration order is
        // significant; forward references are not allowed.
        common::assert_program_result_u64(
            r"
        const A: u64 = 5u64
        const B: u64 = A + 10u64
        fn main() -> u64 { B }
        ",
            15,
        );
    }

    #[test]
    fn test_const_type_mismatch_is_type_error() {
        let source = r"
        const X: u64 = true
        fn main() -> u64 { X }
        ";
        let result = common::test_program(source);
        let err = result.expect_err("expected type-check failure");
        let msg = err.to_string();
        assert!(
            msg.contains("Const") && msg.contains("X"),
            "unexpected error: {msg}"
        );
    }

    // ----- panic builtin -----

    #[test]
    fn test_panic_aborts_with_message() {
        let source = r#"
        fn main() -> i64 {
            panic("not implemented")
            0i64
        }
        "#;
        let result = common::test_program(source);
        let err = result.expect_err("expected panic");
        let msg = err.to_string();
        assert!(
            msg.contains("panic:") && msg.contains("not implemented"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_panic_in_expression_position_unifies_with_branch_type() {
        // The `then` branch panics; the `else` branch returns i64.
        // Without the type-unifier change, the if-expression would
        // collapse to Unit and the function would fail to typecheck.
        common::assert_program_result_i64(
            r#"
        fn divide(a: i64, b: i64) -> i64 {
            if b == 0i64 {
                panic("division by zero")
            } else {
                a / b
            }
        }
        fn main() -> i64 { divide(20i64, 4i64) }
        "#,
            5,
        );
    }

    #[test]
    fn test_panic_message_can_be_a_const() {
        // Panic accepts any `str` expression, including a const.
        let source = r#"
        const ERR: str = "fatal"
        fn main() -> i64 { panic(ERR) }
        "#;
        let result = common::test_program(source);
        let err = result.expect_err("expected panic");
        assert!(err.to_string().contains("panic: fatal"));
    }

    // ----- assert builtin -----

    #[test]
    fn test_assert_passes_when_condition_true() {
        common::assert_program_result_i64(
            r#"
        fn main() -> i64 {
            assert(1i64 + 1i64 == 2i64, "math broken")
            assert(2i64 > 1i64, "ordering broken")
            42i64
        }
        "#,
            42,
        );
    }

    #[test]
    fn test_assert_panics_when_condition_false() {
        let source = r#"
        fn main() -> i64 {
            assert(1i64 == 2i64, "values differ")
            0i64
        }
        "#;
        let result = common::test_program(source);
        let err = result.expect_err("expected assert failure");
        let msg = err.to_string();
        assert!(
            msg.contains("panic:") && msg.contains("values differ"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_assert_does_not_evaluate_message_on_success() {
        // The message is only used when the condition fails; this test
        // doesn't directly exercise that, but documents the expected
        // shape — the success path returns Unit cleanly.
        common::assert_program_result_u64(
            r#"
        fn main() -> u64 {
            assert(true, "ok")
            7u64
        }
        "#,
            7,
        );
    }

    /// Regression: a `return` in a value-producing position used to escape
    /// the function as a "Propagate flow:" runtime error because the helper
    /// type `InterpreterError::PropagateFlow` was created at every
    /// `extract_value` boundary and never caught. The replacement
    /// `try_value!` macro propagates the control-flow signal as
    /// `Ok(EvaluationResult::Return(...))` so the enclosing function
    /// sees it and returns normally.
    #[test]
    fn test_return_inside_value_position_propagates() {
        common::assert_program_result_i64(
            r"
        fn foo(x: i64) -> i64 {
            val y: i64 = if x > 0i64 { return 100i64 } else { 5i64 }
            y + 1i64
        }
        fn main() -> i64 { foo(1i64) }
        ",
            100,
        );
    }

    /// Same regression in the else branch — the false condition path also
    /// has to propagate flow correctly.
    #[test]
    fn test_return_inside_value_position_else_branch() {
        common::assert_program_result_i64(
            r"
        fn foo(x: i64) -> i64 {
            val y: i64 = if x > 0i64 { 5i64 } else { return 100i64 }
            y + 1i64
        }
        fn main() -> i64 { foo(0i64) }
        ",
            100,
        );
    }

    #[test]
    fn test_unary_minus_at_statement_start() {
        // `-x` on its own line must not be absorbed as `7i64 - x`.
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            var x: i64 = 7i64
            x = 0i64 - x
            -x
        }
        ", 7);
    }

    #[test]
    fn test_unary_minus_after_binding_line() {
        // Regression: without the newline-aware fix, `val a = 10\n-b` parses
        // as `val a = 10 - b`, masking the following `-b` expression statement.
        common::assert_program_result_i64(r"
        fn main() -> i64 {
            val a: i64 = 10i64
            val b: i64 = 3i64
            val c: i64 = a
            -c + b
        }
        ", -7);
    }

    #[test]
    fn test_unary_minus_on_u64_is_type_error() {
        let source = r"
        fn main() -> u64 {
            val x: u64 = 7u64
            val y: u64 = -x
            y
        }
        ";
        let result = common::test_program(source);
        assert!(result.is_err(), "negating a u64 should fail type checking");
    }
}

mod variables {
    //! Variable declaration and type inference tests

    use super::helpers::execute_test_program;

    #[test]
    fn test_val_boolean_basic() {
        let source = r#"
            fn main() -> u64 {
                val x = true
                if x {
                    1u64
                } else {
                    0u64
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(1)"), "Expected UInt64(1), got: {}", result);
    }

    #[test]
    fn test_val_integer_basic() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                x
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(42)"), "Expected UInt64(42), got: {}", result);
    }

    #[test]
    fn test_val_multiple_variables() {
        let source = r#"
            fn main() -> u64 {
                val x = true
                val y = 10u64
                val z = 20u64
                if x {
                    y + z
                } else {
                    0u64
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
    }

    #[test]
    fn test_val_nested_scopes() {
        let source = r#"
            fn test_func() -> u64 {
                val x = true
                if x {
                    val y = 5u64
                    y
                } else {
                    0u64
                }
            }

            fn main() -> u64 {
                test_func()
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(5)"), "Expected UInt64(5), got: {}", result);
    }

    #[test]
    fn test_val_with_arithmetic() {
        let source = r#"
            fn main() -> u64 {
                val x = 10u64
                val y = 20u64
                val result = x + y
                result
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
    }

    #[test]
    fn test_val_type_annotation() {
        let source = r#"
            fn main() -> i64 {
                val x: i64 = 100i64
                val y: i64 = 50i64
                x - y
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("Int64(50)"), "Expected Int64(50), got: {}", result);
    }

    #[test]
    fn test_var_mutable_assignment() {
        let source = r#"
            fn main() -> u64 {
                var x = 10u64
                x = x + 5u64
                x
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(15)"), "Expected UInt64(15), got: {}", result);
    }

    #[test]
    fn test_var_multiple_assignments() {
        let source = r#"
            fn main() -> u64 {
                var x = 1u64
                var y = 2u64
                x = x + y
                y = y * 3u64
                x + y
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(9)"), "Expected UInt64(9), got: {}", result);
    }

    #[test]
    fn test_variable_scoping() {
        let source = r#"
            fn inner_func() -> u64 {
                val local_var = 42u64
                local_var
            }

            fn main() -> u64 {
                val outer_var = 10u64
                val result = inner_func()
                outer_var + result
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(52)"), "Expected UInt64(52), got: {}", result);
    }

    #[test]
    fn test_val_shadowing() {
        let source = r#"
            fn main() -> u64 {
                val x = 5u64
                val x = 10u64
                x
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(10)"), "Expected UInt64(10) after shadowing, got: {}", result);
    }

    #[test]
    fn test_mixed_val_var() {
        let source = r#"
            fn main() -> u64 {
                val immut = 5u64
                var mut_var = 10u64
                mut_var = mut_var + immut
                mut_var
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(15)"), "Expected UInt64(15), got: {}", result);
    }

    #[test]
    fn test_type_inference_from_context() {
        let source = r#"
            fn main() -> u64 {
                val x = 42u64
                val y = 8u64
                x - y
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(34)"), "Expected UInt64(34), got: {}", result);
    }

    #[test]
    fn test_boolean_variable() {
        let source = r#"
            fn main() -> u64 {
                val is_true = true
                val is_false = false
                if is_true && !is_false {
                    100u64
                } else {
                    0u64
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(100)"), "Expected UInt64(100), got: {}", result);
    }

    #[test]
    fn test_val_mixed_with_var() {
        let source = r#"
            fn main() -> u64 {
                val x = 10u64
                var y = 20u64
                val z = x + y
                y = 30u64
                val result = z + y
                result
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(60)"), "Expected UInt64(60), got: {}", result);
    }

    #[test]
    fn test_val_function_parameters_vs_locals() {
        let source = r#"
            fn test_func(param: u64) -> u64 {
                val local = param * 2u64
                local
            }

            fn main() -> u64 {
                val input = 15u64
                test_func(input)
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(30)"), "Expected UInt64(30), got: {}", result);
    }

    #[test]
    fn test_val_error_handling_immutable() {
        // This test verifies that val variables cannot be reassigned
        let source = r#"
            fn main() -> u64 {
                val x = 10u64
                x = 20u64  # This should cause a compile error
                x
            }
        "#;

        // This should fail at type checking stage
        let result = execute_test_program(source);
        assert!(result.is_err(), "Assignment to val variable should fail");

        let error = result.unwrap_err();
        assert!(error.contains("error") || error.contains("Error"),
                "Error message should contain 'error': {}", error);
    }
}

mod control_flow {
    //! Control flow (if/else, for, while) tests

    use super::common;
    use super::helpers::execute_test_program;

    #[test]
    fn test_simple_for_loop() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 5u64 {
                sum = sum + i
            }
            sum
        }
        ", 10);
    }

    #[test]
    fn test_simple_for_loop_break() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 10u64 {
                if i > 3u64 {
                    break
                }
                sum = sum + i
            }
            sum
        }
        ", 6);
    }

    #[test]
    fn test_simple_for_loop_continue() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var sum = 0u64
            for i in 1u64 to 5u64 {
                if i == 3u64 {
                    continue
                }
                sum = sum + i
            }
            sum
        }
        ", 7);
    }

    #[test]
    fn test_while_loop_basic() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ", 10);
    }

    #[test]
    fn test_while_loop_break() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 100u64 {
                if i > 5u64 {
                    break
                }
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ", 15);
    }

    #[test]
    fn test_if_else_basic() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val x = 10u64
            if x > 5u64 {
                100u64
            } else {
                50u64
            }
        }
        ", 100);
    }

    #[test]
    fn test_nested_if_else() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            val x = 15u64
            if x > 10u64 {
                if x > 20u64 {
                    300u64
                } else {
                    200u64
                }
            } else {
                100u64
            }
        }
        ", 200);
    }

    #[test]
    fn test_simple_if_then_else_true() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            if true {
                1u64
            } else {
                2u64
            }
        }
        ", 1);
    }

    #[test]
    fn test_simple_if_then_else_false() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                2u64
            }
        }
        ", 2);
    }

    #[test]
    fn test_while_loop_with_break() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while true {
                if i >= 3u64 {
                    break
                }
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ", 3);
    }

    #[test]
    fn test_while_loop_with_continue() {
        common::assert_program_result_u64(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                i = i + 1u64
                if i == 3u64 {
                    continue
                }
                sum = sum + i
            }
            sum
        }
        ", 12);
    }

    #[test]
    fn test_val_conditional_chains() {
        use super::helpers::execute_test_program;

        let source = r#"
            fn main() -> u64 {
                val a = true
                val b = false
                val c = true

                if a {
                    if b {
                        1u64
                    } else {
                        if c {
                            2u64
                        } else {
                            3u64
                        }
                    }
                } else {
                    4u64
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(2)"), "Expected UInt64(2), got: {}", result);
    }

    #[test]
    fn test_range_in_for_loop() {
        let source = r#"
            fn main() -> u64 {
                var sum: u64 = 0u64
                for i in 0u64..5u64 {
                    sum = sum + i
                }
                sum
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(10)"), "got: {}", result);
    }

    #[test]
    fn test_range_and_to_produce_same_iteration() {
        let source = r#"
            fn sum_with_to() -> u64 {
                var s: u64 = 0u64
                for i in 0u64 to 5u64 {
                    s = s + i
                }
                s
            }

            fn sum_with_range() -> u64 {
                var s: u64 = 0u64
                for i in 0u64..5u64 {
                    s = s + i
                }
                s
            }

            fn main() -> u64 {
                sum_with_to() + sum_with_range()
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(20)"), "got: {}", result);
    }

    #[test]
    fn test_range_literal_as_value() {
        // Range can be stored in a val and printed deterministically as
        // `start..end`. Iteration over a bound range is not supported yet.
        let source = r#"
            fn main() -> u64 {
                val r = 3u64..7u64
                println(r)
                0u64
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(0)"), "got: {}", result);
    }

    #[test]
    fn test_range_endpoint_type_mismatch_rejected() {
        let source = r#"
            fn main() -> u64 {
                val r = 0u64..10i64
                0u64
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected type error for mixed-signed range");
    }
}

mod function_calls {
    //! Function definition and call tests

    use super::common;

    #[test]
    fn test_simple_function_call() {
        common::assert_program_result_u64(r"
        fn add(a: u64, b: u64) -> u64 {
            a + b
        }

        fn main() -> u64 {
            add(5u64, 3u64)
        }
        ", 8);
    }

    #[test]
    fn test_multiple_function_calls() {
        common::assert_program_result_u64(r"
        fn double(x: u64) -> u64 {
            x * 2u64
        }

        fn add(a: u64, b: u64) -> u64 {
            a + b
        }

        fn main() -> u64 {
            val x = double(5u64)
            val y = double(3u64)
            add(x, y)
        }
        ", 16);
    }

    #[test]
    fn test_recursive_function() {
        common::assert_program_result_u64(r"
        fn factorial(n: u64) -> u64 {
            if n <= 1u64 {
                1u64
            } else {
                n * factorial(n - 1u64)
            }
        }

        fn main() -> u64 {
            factorial(5u64)
        }
        ", 120);
    }

    #[test]
    fn test_function_with_multiple_statements() {
        common::assert_program_result_u64(r"
        fn calculate(x: u64, y: u64) -> u64 {
            val sum = x + y
            val product = x * y
            sum + product
        }

        fn main() -> u64 {
            calculate(3u64, 4u64)
        }
        ", 19);
    }
}

mod heap_operations {
    //! Heap memory operation tests with val/var variables

    use super::helpers::execute_test_program;

    #[test]
    fn test_val_heap_integration() {
        let source = r#"
            fn main() -> u64 {
                val heap_ptr = __builtin_heap_alloc(8u64)
                val is_null = __builtin_ptr_is_null(heap_ptr)
                if is_null {
                    0u64
                } else {
                    __builtin_ptr_write(heap_ptr, 0u64, 100u64)
                    val value = __builtin_ptr_read(heap_ptr, 0u64)
                    __builtin_heap_free(heap_ptr)
                    value
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(100)"), "Expected UInt64(100), got: {}", result);
    }

    #[test]
    fn test_val_heap_complex_operations() {
        let source = r#"
            fn main() -> u64 {
                val src = __builtin_heap_alloc(16u64)
                val dst = __builtin_heap_alloc(16u64)

                __builtin_ptr_write(src, 0u64, 123u64)
                __builtin_ptr_write(src, 8u64, 456u64)

                __builtin_mem_copy(src, dst, 16u64)

                val result1 = __builtin_ptr_read(dst, 0u64)
                val result2 = __builtin_ptr_read(dst, 8u64)

                __builtin_heap_free(src)
                __builtin_heap_free(dst)

                result1 + result2
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(579)"), "Expected UInt64(579), got: {}", result);
    }

    #[test]
    fn test_val_heap_realloc() {
        let source = r#"
            fn main() -> u64 {
                val heap_ptr1 = __builtin_heap_alloc(8u64)
                __builtin_ptr_write(heap_ptr1, 0u64, 200u64)

                val heap_ptr2 = __builtin_heap_realloc(heap_ptr1, 16u64)
                val value = __builtin_ptr_read(heap_ptr2, 0u64)

                __builtin_heap_free(heap_ptr2)
                value
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(200)"), "Expected UInt64(200), got: {}", result);
    }

    #[test]
    fn test_val_heap_memory_operations() {
        let source = r#"
            fn main() -> u64 {
                val heap_ptr = __builtin_heap_alloc(16u64)

                # Set memory to a specific value
                val fill_value = 255u64
                __builtin_mem_set(heap_ptr, fill_value, 8u64)

                # Read back as u64 (should be all 0xFF bytes)
                val result = __builtin_ptr_read(heap_ptr, 0u64)

                __builtin_heap_free(heap_ptr)

                # 0xFFFFFFFFFFFFFFFF = 18446744073709551615
                if result == 18446744073709551615u64 {
                    1u64
                } else {
                    0u64
                }
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(1)"), "Expected UInt64(1), got: {}", result);
    }

    #[test]
    fn test_val_complex_heap_scenario() {
        let source = r#"
            fn allocate_and_fill(size: u64, value: u64) -> u64 {
                val heap_ptr = __builtin_heap_alloc(size)
                val is_null = __builtin_ptr_is_null(heap_ptr)

                if is_null {
                    0u64
                } else {
                    __builtin_ptr_write(heap_ptr, 0u64, value)
                    val stored = __builtin_ptr_read(heap_ptr, 0u64)
                    __builtin_heap_free(heap_ptr)
                    stored
                }
            }

            fn main() -> u64 {
                val test1 = allocate_and_fill(8u64, 111u64)
                val test2 = allocate_and_fill(8u64, 222u64)
                val test3 = allocate_and_fill(8u64, 333u64)

                test1 + test2 + test3
            }
        "#;

        let result = execute_test_program(source).expect("Program should execute successfully");
        assert!(result.contains("UInt64(666)"), "Expected UInt64(666), got: {}", result);
    }

    #[test]
    fn test_generic_list_with_allocator_type_param() {
        // struct List<T, A: Allocator> with push/get working under an arena.
        let source = r#"
            struct List<T, A: Allocator> {
                data: ptr,
                len: u64,
                cap: u64,
                alloc: A,
            }

            impl List {
                fn push(self: Self, value: u64) -> Self {
                    val elem_size: u64 = __builtin_sizeof(value)
                    var new_cap: u64 = self.cap
                    if self.cap == 0u64 {
                        new_cap = 8u64
                    } elif self.len >= self.cap {
                        new_cap = self.cap * 2u64
                    }
                    var new_data: ptr = self.data
                    if new_cap != self.cap {
                        new_data = __builtin_heap_realloc(self.data, new_cap * elem_size)
                    }
                    __builtin_ptr_write(new_data, self.len * elem_size, value)
                    List { data: new_data, len: self.len + 1u64, cap: new_cap, alloc: self.alloc }
                }

                fn get(self: Self, index: u64) -> u64 {
                    __builtin_ptr_read(self.data, index * 8u64)
                }
            }

            fn main() -> u64 {
                val arena = __builtin_arena_allocator()
                with allocator = arena {
                    val empty: List<u64, Allocator> = List {
                        data: __builtin_heap_alloc(0u64),
                        len: 0u64,
                        cap: 0u64,
                        alloc: ambient,
                    }
                    val list = empty.push(10u64).push(20u64).push(30u64)
                    list.get(0u64) + list.get(1u64) + list.get(2u64)
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(60)"), "got: {}", result);
    }

    #[test]
    fn test_generic_list_i64() {
        // Same shape as List<u64,_> but stores i64 — the typed-slot
        // path preserves the signed value across realloc and read-back.
        let source = r#"
            struct List<T, A: Allocator> {
                data: ptr,
                len: u64,
                cap: u64,
                alloc: A,
            }

            impl List {
                fn push(self: Self, value: i64) -> Self {
                    val elem_size: u64 = __builtin_sizeof(value)
                    var new_cap: u64 = self.cap
                    if self.cap == 0u64 {
                        new_cap = 8u64
                    } elif self.len >= self.cap {
                        new_cap = self.cap * 2u64
                    }
                    var new_data: ptr = self.data
                    if new_cap != self.cap {
                        new_data = __builtin_heap_realloc(self.data, new_cap * elem_size)
                    }
                    __builtin_ptr_write(new_data, self.len * elem_size, value)
                    List { data: new_data, len: self.len + 1u64, cap: new_cap, alloc: self.alloc }
                }

                fn get(self: Self, index: u64) -> i64 {
                    val v: i64 = __builtin_ptr_read(self.data, index * 8u64)
                    v
                }
            }

            fn main() -> i64 {
                val arena = __builtin_arena_allocator()
                with allocator = arena {
                    val empty: List<i64, Allocator> = List {
                        data: __builtin_heap_alloc(0u64),
                        len: 0u64,
                        cap: 0u64,
                        alloc: ambient,
                    }
                    val list = empty.push(-5i64).push(10i64).push(-20i64)
                    list.get(0u64) + list.get(1u64) + list.get(2u64)
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(-15)"), "got: {}", result);
    }

    #[test]
    fn test_generic_list_bool() {
        let source = r#"
            struct List<T, A: Allocator> {
                data: ptr,
                len: u64,
                cap: u64,
                alloc: A,
            }

            impl List {
                fn push(self: Self, value: bool) -> Self {
                    val elem_size: u64 = __builtin_sizeof(value)
                    var new_cap: u64 = self.cap
                    if self.cap == 0u64 {
                        new_cap = 8u64
                    } elif self.len >= self.cap {
                        new_cap = self.cap * 2u64
                    }
                    var new_data: ptr = self.data
                    if new_cap != self.cap {
                        new_data = __builtin_heap_realloc(self.data, new_cap * elem_size)
                    }
                    __builtin_ptr_write(new_data, self.len * elem_size, value)
                    List { data: new_data, len: self.len + 1u64, cap: new_cap, alloc: self.alloc }
                }

                fn get(self: Self, index: u64) -> bool {
                    val v: bool = __builtin_ptr_read(self.data, index)
                    v
                }
            }

            fn main() -> u64 {
                val arena = __builtin_arena_allocator()
                with allocator = arena {
                    val empty: List<bool, Allocator> = List {
                        data: __builtin_heap_alloc(0u64),
                        len: 0u64,
                        cap: 0u64,
                        alloc: ambient,
                    }
                    val list = empty.push(true).push(false).push(true)
                    var cnt: u64 = 0u64
                    for i in 0u64..3u64 {
                        if list.get(i) {
                            cnt = cnt + 1u64
                        }
                    }
                    cnt
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(2)"), "got: {}", result);
    }

    #[test]
    fn test_sizeof_primitives() {
        let source = r#"
            fn main() -> u64 {
                val a: u64 = __builtin_sizeof(0u64)
                val b: u64 = __builtin_sizeof(0i64)
                val c: u64 = __builtin_sizeof(true)
                a + b + c
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(17)"), "got: {}", result);
    }

    #[test]
    fn test_sizeof_through_generic_parameter() {
        // Generic functions propagate concrete types at call site, so
        // `__builtin_sizeof(probe)` reports the real element size.
        let source = r#"
            fn elem_size<T>(probe: T) -> u64 {
                __builtin_sizeof(probe)
            }

            fn main() -> u64 {
                elem_size(0u64) + elem_size(true)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(9)"), "got: {}", result);
    }

    #[test]
    fn test_sizeof_struct_sums_field_widths() {
        let source = r#"
            struct Point {
                x: u64,
                y: i64,
                active: bool,
            }

            fn main() -> u64 {
                val p: Point = Point { x: 0u64, y: 0i64, active: false }
                __builtin_sizeof(p)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(17)"), "got: {}", result);
    }

    #[test]
    fn test_sizeof_enum_adds_tag_and_payload() {
        // Unit variants take 1 byte (tag only). Tuple variants add their
        // payload sizes on top of the 1-byte tag.
        let source = r#"
            enum Option<T> { None, Some(T) }
            enum Color { Red, Green, Blue }

            fn main() -> u64 {
                val n: Option<i64> = Option::None
                val s: Option<i64> = Option::Some(42i64)
                val c: Color = Color::Red
                __builtin_sizeof(n) + __builtin_sizeof(s) + __builtin_sizeof(c)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(11)"), "got: {}", result);
    }

    #[test]
    fn test_sizeof_with_heap_alloc_sizing() {
        // Realistic usage: allocate space for one element using sizeof.
        let source = r#"
            fn main() -> u64 {
                val arena = __builtin_arena_allocator()
                with allocator = arena {
                    val p = __builtin_heap_alloc(__builtin_sizeof(0u64))
                    __builtin_ptr_write(p, 0u64, 42u64)
                    __builtin_ptr_read(p, 0u64)
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("UInt64(42)"), "got: {}", result);
    }
}

mod enum_and_match {
    //! Phase 1 enum support: unit variants, variant construction via
    //! `Enum::Variant`, and match with unit patterns + wildcard.

    use super::helpers::execute_test_program;

    #[test]
    fn test_match_all_variants() {
        let source = r#"
            enum Color {
                Red,
                Green,
                Blue,
            }

            fn main() -> i64 {
                val c: Color = Color::Green
                match c {
                    Color::Red => 1i64,
                    Color::Green => 2i64,
                    Color::Blue => 3i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(2)"), "got: {}", result);
    }

    #[test]
    fn test_match_wildcard_fallback() {
        let source = r#"
            enum Status {
                Ok,
                NotFound,
                Error,
            }

            fn describe(s: Status) -> i64 {
                match s {
                    Status::Ok => 0i64,
                    _ => -1i64,
                }
            }

            fn main() -> i64 {
                describe(Status::Ok) + describe(Status::NotFound) + describe(Status::Error)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(-2)"), "got: {}", result);
    }

    #[test]
    fn test_enum_variant_as_function_argument() {
        let source = r#"
            enum Day {
                Mon,
                Tue,
                Wed,
            }

            fn is_midweek(d: Day) -> bool {
                match d {
                    Day::Wed => true,
                    _ => false,
                }
            }

            fn main() -> bool {
                is_midweek(Day::Wed)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Bool(true)"), "got: {}", result);
    }

    #[test]
    fn test_match_unknown_variant_fails_type_check() {
        let source = r#"
            enum Color {
                Red,
            }

            fn main() -> i64 {
                val c: Color = Color::Red
                match c {
                    Color::Blue => 1i64,
                    _ => 0i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected type error for unknown variant");
    }

    #[test]
    fn test_match_wrong_enum_in_pattern_fails() {
        let source = r#"
            enum A { X }
            enum B { Y }

            fn main() -> i64 {
                val a: A = A::X
                match a {
                    B::Y => 1i64,
                    _ => 0i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected type error for cross-enum pattern");
    }

    #[test]
    fn test_tuple_variant_construction_and_match() {
        let source = r#"
            enum Shape {
                Circle(i64),
                Rect(i64, i64),
                Point,
            }

            fn area(s: Shape) -> i64 {
                match s {
                    Shape::Circle(r) => r * r * 3i64,
                    Shape::Rect(w, h) => w * h,
                    Shape::Point => 0i64,
                }
            }

            fn main() -> i64 {
                area(Shape::Circle(5i64)) + area(Shape::Rect(3i64, 4i64)) + area(Shape::Point)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(87)"), "got: {}", result);
    }

    #[test]
    fn test_tuple_variant_wildcard_slot_ignores_value() {
        let source = r#"
            enum Pair { Both(i64, i64) }

            fn take_second(p: Pair) -> i64 {
                match p {
                    Pair::Both(_, y) => y,
                }
            }

            fn main() -> i64 {
                take_second(Pair::Both(1i64, 42i64))
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(42)"), "got: {}", result);
    }

    #[test]
    fn test_payload_arity_mismatch_rejected() {
        let source = r#"
            enum E { V(i64, i64) }

            fn main() -> i64 {
                val e: E = E::V(1i64, 2i64)
                match e {
                    E::V(x) => x,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected pattern-arity mismatch error");
    }

    #[test]
    fn test_payload_type_mismatch_at_construction_rejected() {
        let source = r#"
            enum E { V(i64) }

            fn main() -> i64 {
                val e: E = E::V(1u64)
                0i64
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected payload type mismatch error");
    }

    #[test]
    fn test_unit_variant_called_with_args_rejected() {
        let source = r#"
            enum E { X }

            fn main() -> i64 {
                val e: E = E::X(1i64)
                0i64
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected arity mismatch on unit variant");
    }

    #[test]
    fn test_tuple_variant_unit_reference_rejected() {
        let source = r#"
            enum E { V(i64) }

            fn main() -> i64 {
                val e: E = E::V
                0i64
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected error: tuple variant referenced without arguments");
    }

    #[test]
    fn test_non_exhaustive_match_rejected() {
        let source = r#"
            enum Color { Red, Green, Blue }

            fn main() -> i64 {
                val c: Color = Color::Red
                match c {
                    Color::Red => 1i64,
                    Color::Green => 2i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected non-exhaustive match error");
        let err = result.unwrap_err();
        assert!(err.contains("Blue"), "error should mention missing variant Blue: {}", err);
    }

    #[test]
    fn test_exhaustive_match_without_wildcard_accepted() {
        let source = r#"
            enum Bit { Zero, One }

            fn main() -> i64 {
                val b: Bit = Bit::One
                match b {
                    Bit::Zero => 0i64,
                    Bit::One => 1i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(1)"), "got: {}", result);
    }

    #[test]
    fn test_wildcard_makes_match_exhaustive() {
        let source = r#"
            enum Color { Red, Green, Blue }

            fn main() -> i64 {
                val c: Color = Color::Blue
                match c {
                    Color::Red => 1i64,
                    _ => 99i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(99)"), "got: {}", result);
    }

    #[test]
    fn test_non_exhaustive_tuple_variant_match_rejected() {
        let source = r#"
            enum Shape {
                Circle(i64),
                Point,
            }

            fn main() -> i64 {
                val s: Shape = Shape::Point
                match s {
                    Shape::Circle(r) => r,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected non-exhaustive match error");
        let err = result.unwrap_err();
        assert!(err.contains("Point"), "error should mention missing Point: {}", err);
    }

    #[test]
    fn test_generic_enum_tuple_variant_infers_type_params() {
        let source = r#"
            enum Option<T> {
                None,
                Some(T),
            }

            fn main() -> i64 {
                val x: Option<i64> = Option::Some(42i64)
                match x {
                    Option::Some(v) => v,
                    Option::None => 0i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(42)"), "got: {}", result);
    }

    #[test]
    fn test_generic_enum_unit_variant_with_type_hint() {
        let source = r#"
            enum Option<T> {
                None,
                Some(T),
            }

            fn unwrap_or(o: Option<i64>, default: i64) -> i64 {
                match o {
                    Option::Some(v) => v,
                    Option::None => default,
                }
            }

            fn main() -> i64 {
                val a: Option<i64> = Option::Some(100i64)
                val b: Option<i64> = Option::None
                unwrap_or(a, 1i64) + unwrap_or(b, 2i64)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(102)"), "got: {}", result);
    }

    #[test]
    fn test_generic_enum_binding_uses_substituted_payload_type() {
        // `Some(v)` binds v with the substituted payload type (i64 here),
        // so the arm body can treat it as an i64 without further casts.
        let source = r#"
            enum Box<T> { Put(T) }

            fn main() -> i64 {
                val b = Box::Put(7i64)
                match b {
                    Box::Put(v) => v + 1i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(8)"), "got: {}", result);
    }

    #[test]
    fn test_unreachable_arm_after_wildcard_rejected() {
        let source = r#"
            enum Color { Red, Green }

            fn main() -> i64 {
                val c: Color = Color::Red
                match c {
                    _ => 0i64,
                    Color::Red => 1i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected unreachable arm error");
        let err = result.unwrap_err();
        assert!(err.contains("wildcard"), "error should mention wildcard: {}", err);
    }

    #[test]
    fn test_duplicate_variant_arm_rejected() {
        let source = r#"
            enum Color { Red, Green }

            fn main() -> i64 {
                val c: Color = Color::Red
                match c {
                    Color::Red => 1i64,
                    Color::Red => 2i64,
                    Color::Green => 3i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected duplicate variant arm error");
        let err = result.unwrap_err();
        assert!(err.contains("already fully covered"), "error should mention repeated arm: {}", err);
    }

    #[test]
    fn test_literal_pattern_on_int64() {
        let source = r#"
            fn describe(n: i64) -> i64 {
                match n {
                    0i64 => 100i64,
                    1i64 => 200i64,
                    _ => 999i64,
                }
            }

            fn main() -> i64 {
                describe(0i64) + describe(1i64) + describe(5i64)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(1299)"), "got: {}", result);
    }

    #[test]
    fn test_literal_pattern_on_bool_is_exhaustive() {
        let source = r#"
            fn f(b: bool) -> i64 {
                match b {
                    true => 1i64,
                    false => 0i64,
                }
            }

            fn main() -> i64 {
                f(true) + f(false)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(1)"), "got: {}", result);
    }

    #[test]
    fn test_non_exhaustive_bool_match_rejected() {
        let source = r#"
            fn main() -> i64 {
                val b: bool = true
                match b {
                    true => 1i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected non-exhaustive bool error");
    }

    #[test]
    fn test_int_match_without_wildcard_rejected() {
        let source = r#"
            fn main() -> i64 {
                val n: i64 = 3i64
                match n {
                    0i64 => 1i64,
                    1i64 => 2i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected non-exhaustive int error");
    }

    #[test]
    fn test_duplicate_literal_pattern_rejected() {
        let source = r#"
            fn main() -> i64 {
                val n: i64 = 0i64
                match n {
                    0i64 => 1i64,
                    0i64 => 2i64,
                    _ => 3i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected duplicate literal error");
    }

    #[test]
    fn test_literal_pattern_type_mismatch_rejected() {
        let source = r#"
            fn main() -> i64 {
                val n: i64 = 0i64
                match n {
                    0u64 => 1i64,
                    _ => 2i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected literal type mismatch");
    }

    #[test]
    fn test_nested_enum_pattern_match() {
        let source = r#"
            enum Option<T> {
                None,
                Some(T),
            }

            fn main() -> i64 {
                val b: Option<Option<i64>> = Option::Some(Option::Some(5i64))
                match b {
                    Option::Some(Option::Some(v)) => v + 10i64,
                    Option::Some(Option::None) => -1i64,
                    Option::None => -2i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(15)"), "got: {}", result);
    }

    #[test]
    fn test_nested_pattern_partial_variant_allows_followup_arm() {
        // Having `Some(Some(v))` and `Some(None)` as separate arms is fine;
        // the first only covers part of the `Some` value space, so the
        // second is still reachable.
        let source = r#"
            enum Option<T> { None, Some(T) }

            fn classify(o: Option<Option<i64>>) -> i64 {
                match o {
                    Option::Some(Option::Some(v)) => v,
                    Option::Some(Option::None) => 999i64,
                    Option::None => 0i64,
                }
            }

            fn main() -> i64 {
                classify(Option::Some(Option::Some(5i64)))
                    + classify(Option::Some(Option::None))
                    + classify(Option::None)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(1004)"), "got: {}", result);
    }

    #[test]
    fn test_nested_enum_pattern_unit_inside_tuple() {
        let source = r#"
            enum Color { Red, Green, Blue }
            enum Box { Put(Color) }

            fn main() -> i64 {
                val b = Box::Put(Color::Green)
                match b {
                    Box::Put(Color::Red) => 1i64,
                    Box::Put(Color::Green) => 2i64,
                    Box::Put(Color::Blue) => 3i64,
                }
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(2)"), "got: {}", result);
    }

    #[test]
    fn test_string_literal_pattern() {
        let source = r#"
            fn classify(s: str) -> i64 {
                match s {
                    "zero" => 0i64,
                    "one" => 1i64,
                    "two" => 2i64,
                    _ => -1i64,
                }
            }

            fn main() -> i64 {
                classify("one") + classify("two") + classify("unknown")
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(2)"), "got: {}", result);
    }

    #[test]
    fn test_string_pattern_without_wildcard_rejected() {
        let source = r#"
            fn main() -> i64 {
                val s: str = "x"
                match s {
                    "a" => 1i64,
                    "b" => 2i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected non-exhaustive str error");
    }

    #[test]
    fn test_duplicate_string_literal_pattern_rejected() {
        let source = r#"
            fn main() -> i64 {
                val s: str = "x"
                match s {
                    "a" => 1i64,
                    "a" => 2i64,
                    _ => 3i64,
                }
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected duplicate string literal error");
    }

    #[test]
    fn test_name_pattern_at_top_level_binds_scrutinee() {
        let source = r#"
            enum Color { Red, Green, Blue }

            fn color_code(c: Color) -> i64 {
                match c {
                    Color::Red => 1i64,
                    other => 99i64,
                }
            }

            fn main() -> i64 {
                color_code(Color::Red) + color_code(Color::Blue)
            }
        "#;
        let result = execute_test_program(source).expect("should execute");
        assert!(result.contains("Int64(100)"), "got: {}", result);
    }

    #[test]
    fn test_duplicate_variant_rejected() {
        let source = r#"
            enum Color {
                Red,
                Red,
            }

            fn main() -> i64 {
                0i64
            }
        "#;
        let result = execute_test_program(source);
        assert!(result.is_err(), "expected duplicate variant rejection");
    }
}

