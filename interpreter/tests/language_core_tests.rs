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
}

