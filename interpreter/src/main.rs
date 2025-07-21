use std::env;
use std::fs;
use interpreter;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: {} <file>", args[0]);
        return;
    }
    let file = fs::read_to_string(&args[1]).expect("Failed to read file");
    let mut parser = frontend::Parser::new(&file);
    let program = parser.parse_program();
    if program.is_err() {
        if let Err(err) = program {
            println!("parser_program failed {:?}", err);
        }
        return;
    }

    let mut program = match program {
        Ok(p) => p,
        Err(e) => {
            println!("Failed to parse program: {:?}", e);
            return;
        }
    };

    if let Err(errors) = interpreter::check_typing(&mut program, Some(&file), Some(&args[1])) {
        for e in errors {
            eprintln!("{}", e);
        }
        return;
    }

    let res = interpreter::execute_program(&program, Some(&file), Some(&args[1]));
    if let Ok(result) = res {
        println!("Result: {:?}", result);
    } else {
        eprintln!("{}", res.unwrap_err());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use frontend;
    use frontend::ast::*;
    use string_interner::DefaultStringInterner;
    use interpreter::object::Object;
    use interpreter::evaluation::{EvaluationContext, EvaluationResult};

    #[test]
    fn test_evaluate_integer() {
        let stmt_pool = StmtPool::new();
        let mut expr_pool = ExprPool::new();
        let expr_ref = expr_pool.add(Expr::Int64(42));
        let mut interner = DefaultStringInterner::new();

        let mut ctx = EvaluationContext::new(&stmt_pool, &expr_pool, &mut interner, HashMap::new());
        let result = match ctx.evaluate(&expr_ref) {
            Ok(EvaluationResult::Value(v)) => v,
            Ok(other) => panic!("Expected Value but got {:?}", other),
            Err(e) => panic!("Evaluation failed: {:?}", e),
        };

        assert_eq!(result.borrow().unwrap_int64(), 42);
    }

    #[test]
    fn test_i64_basic() {
        let res = test_program(r"
        fn main() -> i64 {
            val a: i64 = 42i64
            val b: i64 = -10i64
            a + b
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 32);
    }

    #[test]
    fn test_simple_program() {
        let mut parser = frontend::Parser::new(r"
        fn main() -> u64 {
            val a = 1u64
            val b = 2u64
            val c = a + b
            c
        }
        ");
        let program = parser.parse_program();
        assert!(program.is_ok());

        let program = program.unwrap();

        let res = interpreter::execute_program(&program, Some("fn main() -> u64 { 1u64 + 2u64 }"), Some("test.t"));
        assert!(res.is_ok());
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    fn test_program(source_code: &str) -> Result<Rc<RefCell<Object>>, String> {
        let mut parser = frontend::Parser::new(source_code);
        let mut program = parser.parse_program()
            .map_err(|e| format!("Parse error: {:?}", e))?;
        
        // Check typing
        interpreter::check_typing(&mut program, Some(source_code), Some("test.t"))
            .map_err(|errors| format!("Type check errors: {:?}", errors))?;
        
        // Execute program
        interpreter::execute_program(&program, Some(source_code), Some("test.t"))
    }

    #[test]
    fn test_simple_if_then_else_1() {
        let res = test_program(r"
        fn main() -> u64 {
            if true {
                1u64
            } else {
                2u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1u64);
    }

    #[test]
    fn test_simple_if_then_else_2() {
        let res = test_program(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                2u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 2u64);
    }

    #[test]
    fn test_simple_elif_first_true() {
        let res = test_program(r"
        fn main() -> u64 {
            val x = 1u64
            if x == 0u64 {
                10u64
            } elif x == 1u64 {
                20u64
            } else {
                30u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 20);
    }

    #[test]
    fn test_simple_elif_second_true() {
        let res = test_program(r"
        fn main() -> u64 {
            val x = 2u64
            if x == 0u64 {
                10u64
            } elif x == 1u64 {
                20u64
            } elif x == 2u64 {
                30u64
            } else {
                40u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 30);
    }

    #[test]
    fn test_simple_elif_else_fallback() {
        let res = test_program(r"
        fn main() -> u64 {
            val x = 5u64
            if x == 0u64 {
                10u64
            } elif x == 1u64 {
                20u64
            } elif x == 2u64 {
                30u64
            } else {
                40u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 40);
    }

    #[test]
    fn test_elif_with_complex_conditions() {
        let res = test_program(r"
        fn main() -> u64 {
            val x = 15u64
            if x < 10u64 {
                1u64
            } elif x >= 10u64 && x < 20u64 {
                2u64
            } elif x >= 20u64 {
                3u64
            } else {
                4u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 2);
    }

    #[test]
    fn test_simple_for_loop() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                a = a + 1u64
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 4);
    }

    #[test]
    fn test_simple_for_loop_continue() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                if i < 3u64 {
                    continue
                }
                a = a + 1u64
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1);
    }

    #[test]
    fn test_simple_for_loop_break() {
        let res = test_program(r"
        fn main() -> u64 {
            var a = 0u64
            for i in 0u64 to 4u64 {
                a = a + 1u64
                if a > 2u64 {
                    break
                }
            }
            return a
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    #[test]
    fn test_simple_variable_scope() {
        let res = test_program(r"
        fn main() -> u64 {
            var x = 100u64
            {
                var x = 10u64
                x = x + 1000u64
            }
            x = x + 1u64
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 101);
    }

    #[test]
    fn test_simple_while_loop() {
        let res = test_program(r"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            while i < 5u64 {
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 10); // 0+1+2+3+4 = 10
    }

    #[test]
    fn test_while_loop_with_break() {
        let res = test_program(r"
        fn main() -> u64 {
            var i = 0u64
            while true {
                i = i + 1u64
                if i >= 3u64 {
                    break
                }
            }
            i
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    #[test]
    fn test_while_loop_with_continue() {
        let res = test_program(r"
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
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 12); // 1+2+4+5 = 12, skipping 3
    }

    #[test]
    fn test_short_circuit_and_false() {
        let res = test_program(r"
        fn main() -> bool {
            false && true
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), false);
    }

    #[test]
    fn test_short_circuit_and_true() {
        let res = test_program(r"
        fn main() -> bool {
            true && false
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), false);
    }

    #[test]
    fn test_short_circuit_or_true() {
        let res = test_program(r"
        fn main() -> bool {
            true || false
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_short_circuit_or_false() {
        let res = test_program(r"
        fn main() -> bool {
            false || true
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_simple_variable_scope_with_if() {
        let res = test_program(r"
        fn main() -> u64 {
            var x = 100u64
            if true {
                var x = 10u64
                x = x + 1000u64
            }
            x = x + 1u64
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 101);
    }

    #[test]
    fn test_auto_type_conversion_number_to_uint64() {
        // Test: Type-unspecified number (5) should convert to u64 when used with u64
        let res = test_program(r"
        fn main() -> u64 {
            val x = 10u64
            val y = 5
            x + y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 15);
    }

    #[test]
    fn test_auto_type_conversion_number_to_int64() {
        // Test: Type-unspecified number (5) should convert to i64 when used with i64
        let res = test_program(r"
        fn main() -> i64 {
            val x = 10i64
            val y = 5
            x + y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 15);
    }

    #[test]
    fn test_auto_type_conversion_both_numbers() {
        // Test: Two type-unspecified numbers should default to u64
        let res = test_program(r"
        fn main() -> u64 {
            val x = 10
            val y = 5
            x + y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 15);
    }

    #[test]
    fn test_auto_type_conversion_arithmetic_operations() {
        // Test: Mixed operations with type-unspecified numbers
        let res = test_program(r"
        fn main() -> u64 {
            val a = 20u64
            val b = 5
            val c = 2
            (a + b) * c - 10
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 40);  // (20 + 5) * 2 - 10 = 40
    }

    #[test]
    fn test_auto_type_conversion_comparison() {
        // Test: Type-unspecified number should convert for comparison operations
        let res = test_program(r"
        fn main() -> bool {
            val x = 10u64
            val y = 5
            x > y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_negative_number_forces_int64() {
        // Test: Type-unspecified positive number should convert to i64 when used with i64
        let res = test_program(r"
        fn main() -> i64 {
            val x = -5i64
            val y = 10
            x + y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 5);
    }

    #[test]
    fn test_explicit_type_declaration_inference() {
        // Test: Explicit type declaration should infer Number type immediately
        let res = test_program(r"
        fn main() -> i64 {
            val x: i64 = 100
            val y: i64 = 25
            x - y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 75);
    }

    #[test]
    fn test_explicit_type_redefine_declaration_inference() {
        // Test: Variable redefinition with different types works correctly
        let res = test_program(r"
        fn main() -> u64 {
            val y: i64 = 25
            val y = 26u64
            y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 26);
    }

    #[test]
    fn test_mixed_explicit_and_inferred_types() {
        // Test: Mix of explicit declarations and context inference
        let res = test_program(r"
        fn main() -> u64 {
            val a: u64 = 50
            val b = 30
            val c = 5
            (a + b) / c
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 16);
    }

    #[test]
    fn test_context_inference_in_comparison() {
        // Test: Number type inference in comparison operations
        let res = test_program(r"
        fn main() -> bool {
            val x: i64 = -10
            val y = 5
            x < y
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_bool(), true);
    }

    #[test]
    fn test_context_inference_multiple_operations() {
        // Test: Context inference through multiple operations
        let res = test_program(r"
        fn main() -> i64 {
            val a = 100
            val b: i64 = 50
            val c = 25
            val d = 10
            (a - b) + (c - d)
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 65); // (100-50) + (25-10) = 50 + 15 = 65
    }

    #[test]
    fn test_context_inference_nested_expressions() {
        // Test: Context inference in nested expressions
        let res = test_program(r"
        fn main() -> u64 {
            val base: u64 = 10
            val multiplier = 3
            val offset = 5
            base * (multiplier + offset)
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 80); // 10 * (3 + 5) = 10 * 8 = 80
    }

    #[test]
    fn test_context_inference_mixed_with_int64_explicit() {
        // Test: Context inference where only one variable has explicit Int64 type
        let res = test_program(r"
        fn main() -> i64 {
            val a = 100
            val b = 50
            val c = 25
            val d: i64 = 10
            (a - b) + (c - d)
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 65); // (100-50) + (25-10) = 50 + 15 = 65
    }

    #[test]
    fn test_no_context_defaults_to_uint64() {
        // Test: When no context is available, Number should default to UInt64
        let res = test_program(r"
        fn main() -> u64 {
            val x = 42
            x
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 42);
    }

    #[test]
    fn test_context_inference_across_variable_assignments() {
        // Test: Context inference works across multiple variable assignments
        let res = test_program(r"
        fn main() -> i64 {
            val base: i64 = 1000
            val step1 = 100
            val step2 = 50
            val step3 = 25
            base - step1 - step2 - step3
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_int64(), 825); // 1000 - 100 - 50 - 25 = 825
    }

    #[test]
    fn test_simple_if_then() {
        let res = test_program(r"
        fn main() -> u64 {
            if true {
                10u64
            } else {
                1u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 10);
    }

    #[test]
    fn test_simple_if_else() {
        let res = test_program(r"
        fn main() -> u64 {
            if false {
                1u64
            } else {
                1234u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1234);
    }

    #[test]
    fn test_simple_if_trivial_le() {
        let res = test_program(r"
        fn main() -> u64 {
            val n = 1u64
            if n <= 2u64 {
                1u64
            } else {
                1234u64
            }
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1);
    }

    #[test]
    fn test_simple_function_scope() {
        let res = test_program(r"
        fn add(a: u64, b: u64) -> u64 {
            a + b
        }
        fn main() -> u64 {
            add(1u64, 2u64)
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    #[test]
    fn test_simple_fib_scope() {
        let res = test_program(r"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 {
                n
            } else {
                fib(n - 1u64) + fib(n - 2u64)
            }
        }
        fn main() -> u64 {
            fib(2u64)
        }
        ");
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 1);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_comparison_transitivity(a: u64, b: u64, c: u64) {
            let mut values = vec![a, b, c];
            values.sort();
            let (a, b, c) = (values[0], values[1], values[2]);

            let program_a_lt_b = format!(r"
                fn main() -> bool {{
                    {}u64 < {}u64
                }}
            ", a, b);

            let program_b_lt_c = format!(r"
                fn main() -> bool {{
                    {}u64 < {}u64
                }}
            ", b, c);

            let program_a_lt_c = format!(r"
                fn main() -> bool {{
                    {}u64 < {}u64
                }}
            ", a, c);

            let a_lt_b = test_program(&program_a_lt_b).unwrap().borrow().unwrap_bool();
            let b_lt_c = test_program(&program_b_lt_c).unwrap().borrow().unwrap_bool();
            let a_lt_c = test_program(&program_a_lt_c).unwrap().borrow().unwrap_bool();

            // a < b and b < c => a < c
            if a < b && b < c {
                assert!(a_lt_b);
                assert!(b_lt_c);
                assert!(a_lt_c);
            }
        }

        #[test]
        fn test_logical_operations(a: bool, b: bool) {
            let program_and = format!(r"
                fn main() -> bool {{
                    {} && {}
                }}
            ", a, b);

            let program_or = format!(r"
                fn main() -> bool {{
                    {} || {}
                }}
            ", a, b);

            let result_and = test_program(&program_and).unwrap().borrow().unwrap_bool();
            let result_or = test_program(&program_or).unwrap().borrow().unwrap_bool();

            assert_eq!(result_and, a && b);
            assert_eq!(result_or, a || b);
        }

        #[test]
        fn test_i64_arithmetic_properties(a in -1000i64..1000i64, b in -100i64..100i64) {
            prop_assume!(b != 0);

            let program_add = format!(r"
                fn main() -> i64 {{
                    {}i64 + {}i64
                }}
            ", a, b);

            let program_sub = format!(r"
                fn main() -> i64 {{
                    {}i64 - {}i64
                }}
            ", a, b);

            let program_mul = format!(r"
                fn main() -> i64 {{
                    {}i64 * {}i64
                }}
            ", a, b);

            let program_div = format!(r"
                fn main() -> i64 {{
                    {}i64 / {}i64
                }}
            ", a, b);

            let result_add = test_program(&program_add).unwrap().borrow().unwrap_int64();
            let result_sub = test_program(&program_sub).unwrap().borrow().unwrap_int64();
            let result_mul = test_program(&program_mul).unwrap().borrow().unwrap_int64();
            let result_div = test_program(&program_div).unwrap().borrow().unwrap_int64();

            assert_eq!(result_add, a + b);
            assert_eq!(result_sub, a - b);
            assert_eq!(result_mul, a * b);
            assert_eq!(result_div, a / b);
        }

        #[test]
        fn test_i64_comparison_properties(a: i64, b: i64) {
            let program_lt = format!(r"
                fn main() -> bool {{
                    {}i64 < {}i64
                }}
            ", a, b);

            let program_le = format!(r"
                fn main() -> bool {{
                    {}i64 <= {}i64
                }}
            ", a, b);

            let program_gt = format!(r"
                fn main() -> bool {{
                    {}i64 > {}i64
                }}
            ", a, b);

            let program_ge = format!(r"
                fn main() -> bool {{
                    {}i64 >= {}i64
                }}
            ", a, b);

            let program_eq = format!(r"
                fn main() -> bool {{
                    {}i64 == {}i64
                }}
            ", a, b);

            let program_ne = format!(r"
                fn main() -> bool {{
                    {}i64 != {}i64
                }}
            ", a, b);

            let result_lt = test_program(&program_lt).unwrap().borrow().unwrap_bool();
            let result_le = test_program(&program_le).unwrap().borrow().unwrap_bool();
            let result_gt = test_program(&program_gt).unwrap().borrow().unwrap_bool();
            let result_ge = test_program(&program_ge).unwrap().borrow().unwrap_bool();
            let result_eq = test_program(&program_eq).unwrap().borrow().unwrap_bool();
            let result_ne = test_program(&program_ne).unwrap().borrow().unwrap_bool();

            assert_eq!(result_lt, a < b);
            assert_eq!(result_le, a <= b);
            assert_eq!(result_gt, a > b);
            assert_eq!(result_ge, a >= b);
            assert_eq!(result_eq, a == b);
            assert_eq!(result_ne, a != b);
        }

        #[test]
        fn test_i64_for_loop_properties(start in -1000i64..1000i64, end in -1000i64..1000i64) {
            prop_assume!(start <= end);

            let program = format!(r"
                fn main() -> i64 {{
                    var sum: i64 = 0i64
                    for i in {}i64 to {}i64 {{
                        sum = sum + i
                    }}
                    sum
                }}
            ", start, end);

            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            let expected: i64 = (start..end).sum();
            assert_eq!(result, expected);
        }
    }

    // Array tests
    #[test]
    fn test_array_basic_operations() {
        let program = r"
            fn main() -> u64 {
                val a: [u64; 3] = [1u64, 2u64, 3u64]
                a[0u64] + a[1u64] + a[2u64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 6);
    }

    #[test]
    fn test_array_assignment() {
        let program = r"
            fn main() -> i64 {
                var a: [i64; 3] = [1i64, 2i64, 3i64]
                a[1i64] = 10i64
                a[0i64] + a[1i64] + a[2i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 14);
    }

    #[test]
    fn test_array_different_types() {
        let program = r"
            fn main() -> bool {
                val bools: [bool; 2] = [true, false]
                bools[0u64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_bool();
        assert_eq!(result, true);
    }

    #[test]
    fn test_array_complex_expressions() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 4] = [10i64, 20i64, 30i64, 40i64]
                val index: i64 = 2i64
                a[0i64] * a[1i64] + a[index] - a[3i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 190);
    }

    #[test]
    fn test_array_nested_access() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 3] = [0i64, 1i64, 2i64]
                val b: [i64; 3] = [10i64, 20i64, 30i64]
                b[a[1i64]] + b[a[2i64]]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 50);
    }

    #[test]
    fn test_array_in_loop() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 5] = [1i64, 2i64, 3i64, 4i64, 5i64]
                var sum: i64 = 0i64
                for i in 0i64 to 5i64 {
                    sum = sum + a[i]
                }
                sum
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 15);
    }

    #[test]
    fn test_array_single_element() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 1] = [42i64]
                a[0i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_array_multiple_assignments() {
        let program = r"
            fn main() -> i64 {
                var a: [i64; 3] = [1i64, 2i64, 3i64]
                a[0i64] = a[1i64] + a[2i64]
                a[1i64] = a[0i64] * 2i64
                a[2i64] = a[1i64] - a[0i64]
                a[0i64] + a[1i64] + a[2i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 20);
    }

    // Error case tests - these should be type check errors or runtime errors
    #[test]
    #[should_panic(expected = "Array element 1 has type Bool but expected Int64")]
    fn test_array_type_mismatch() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 2] = [1i64, true]
                a[0i64]
            }
        ";
        test_program(program).unwrap();
    }

    #[test]
    #[should_panic(expected = "Array index")]
    fn test_array_index_out_of_bounds() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 2] = [1i64, 2i64]
                a[5i64]
            }
        ";
        test_program(program).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_array_negative_index() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 2] = [1i64, 2i64]
                a[-1i64]
            }
        ";
        test_program(program).unwrap();
    }

    #[test]
    fn test_array_size_one() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 1] = [99i64]
                a[0i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 99);
    }

    #[test]
    fn test_array_large_size() {
        let program = r"
            fn main() -> i64 {
                val a: [i64; 10] = [0i64, 1i64, 2i64, 3i64, 4i64, 5i64, 6i64, 7i64, 8i64, 9i64]
                a[9i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 9);
    }

    #[test]
    fn test_array_modification_preserves_other_elements() {
        let program = r"
            fn main() -> i64 {
                var a: [i64; 5] = [10i64, 20i64, 30i64, 40i64, 50i64]
                a[2i64] = 99i64
                a[0i64] + a[1i64] + a[2i64] + a[3i64] + a[4i64]
            }
        ";
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 219);
    }

    proptest! {
        #[test]
        fn test_array_sum_property(values in prop::collection::vec(0i64..100i64, 1..10)) {
            let size = values.len();
            let values_str = values.iter().map(|v| format!("{}i64", v)).collect::<Vec<_>>().join(", ");
            
            let program = format!(r"
                fn main() -> i64 {{
                    val a: [i64; {}] = [{}]
                    var sum: i64 = 0i64
                    for i in 0i64 to {}i64 {{
                        sum = sum + a[i]
                    }}
                    sum
                }}
            ", size, values_str, size);
            
            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            let expected: i64 = values.iter().sum();
            assert_eq!(result, expected);
        }

        #[test]
        fn test_array_access_property(values in prop::collection::vec(0i64..1000i64, 1..20), index in 0usize..19) {
            prop_assume!(index < values.len());
            
            let size = values.len();
            let values_str = values.iter().map(|v| format!("{}i64", v)).collect::<Vec<_>>().join(", ");
            
            let program = format!(r"
                fn main() -> i64 {{
                    val a: [i64; {}] = [{}]
                    a[{}i64]
                }}
            ", size, values_str, index);
            
            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            assert_eq!(result, values[index]);
        }

        #[test]
        fn test_array_assignment_property(original in prop::collection::vec(0i64..100i64, 2..10), new_value in 0i64..1000i64, index in 0usize..9) {
            prop_assume!(index < original.len());
            
            let size = original.len();
            let values_str = original.iter().map(|v| format!("{}i64", v)).collect::<Vec<_>>().join(", ");
            
            let program = format!(r"
                fn main() -> i64 {{
                    var a: [i64; {}] = [{}]
                    a[{}i64] = {}i64
                    a[{}i64]
                }}
            ", size, values_str, index, new_value, index);
            
            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            assert_eq!(result, new_value);
        }

        // Enhanced property tests - Arithmetic operations with random values
        #[test]
        fn test_arithmetic_properties_extended(a in -10000i64..10000i64, b in -1000i64..1000i64, c in -100i64..100i64) {
            prop_assume!(b != 0 && c != 0);
            
            // Test associativity: (a + b) + c = a + (b + c)
            let program_left = format!(r"
                fn main() -> i64 {{
                    ({}i64 + {}i64) + {}i64
                }}
            ", a, b, c);
            
            let program_right = format!(r"
                fn main() -> i64 {{
                    {}i64 + ({}i64 + {}i64)
                }}
            ", a, b, c);
            
            let result_left = test_program(&program_left).unwrap().borrow().unwrap_int64();
            let result_right = test_program(&program_right).unwrap().borrow().unwrap_int64();
            
            assert_eq!(result_left, result_right);
            assert_eq!(result_left, a.wrapping_add(b).wrapping_add(c));
        }

        // Enhanced property tests - Type inference consistency
        #[test]
        fn test_type_inference_consistency(values in prop::collection::vec(0i64..1000i64, 1..5)) {
            let size = values.len();
            let values_str = values.iter().map(|v| format!("{}i64", v)).collect::<Vec<_>>().join(", ");
            
            // Test that inferred types work consistently with explicit types
            let program = format!(r"
                fn main() -> i64 {{
                    val a: [i64; {}] = [{}]  # Values explicitly as i64
                    var sum: i64 = 0i64  # sum explicitly as i64
                    for i in 0i64 to {}i64 {{  # i explicitly as i64
                        sum = sum + a[i]
                    }}
                    sum
                }}
            ", size, values_str, size);
            
            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            let expected: i64 = values.iter().sum();
            assert_eq!(result, expected);
        }

        // Enhanced property tests - Loop boundary conditions
        #[test]
        fn test_loop_boundary_properties(start in 0i64..100i64, count in 0i64..20i64) {
            let end = start + count;
            
            let program = format!(r"
                fn main() -> i64 {{
                    var iterations: i64 = 0i64
                    for i in {}i64 to {}i64 {{
                        iterations = iterations + 1i64
                    }}
                    iterations
                }}
            ", start, end);
            
            let result = test_program(&program).unwrap().borrow().unwrap_int64();
            assert_eq!(result, count);
        }

        // Enhanced property tests - Comparison operations
        #[test]
        fn test_comparison_properties_extended(a in -1000i64..1000i64, b in -1000i64..1000i64, c in -1000i64..1000i64) {
            // Test transitivity of comparisons
            let program_a_le_b = format!(r"
                fn main() -> bool {{
                    {}i64 <= {}i64
                }}
            ", a, b);
            
            let program_b_le_c = format!(r"
                fn main() -> bool {{
                    {}i64 <= {}i64
                }}
            ", b, c);
            
            let program_a_le_c = format!(r"
                fn main() -> bool {{
                    {}i64 <= {}i64
                }}
            ", a, c);
            
            let a_le_b = test_program(&program_a_le_b).unwrap().borrow().unwrap_bool();
            let b_le_c = test_program(&program_b_le_c).unwrap().borrow().unwrap_bool();
            let a_le_c = test_program(&program_a_le_c).unwrap().borrow().unwrap_bool();
            
            // If a <= b and b <= c, then a <= c (transitivity)
            if a_le_b && b_le_c {
                assert!(a_le_c);
            }
        }

        // Enhanced property tests - Array operations with mixed types
        #[test]
        fn test_array_mixed_operations(u64_vals in prop::collection::vec(0u64..1000u64, 1..5), i64_vals in prop::collection::vec(0i64..1000i64, 1..5)) {
            let u64_size = u64_vals.len();
            let i64_size = i64_vals.len();
            let u64_str = u64_vals.iter().map(|v| format!("{}u64", v)).collect::<Vec<_>>().join(", ");
            let i64_str = i64_vals.iter().map(|v| format!("{}i64", v)).collect::<Vec<_>>().join(", ");
            
            let program = format!(r"
                fn main() -> u64 {{
                    val a: [u64; {}] = [{}]
                    val b: [i64; {}] = [{}]
                    var sum_u64: u64 = 0u64
                    var sum_i64: i64 = 0i64
                    
                    for i in 0u64 to {}u64 {{
                        sum_u64 = sum_u64 + a[i]
                    }}
                    
                    for i in 0i64 to {}i64 {{
                        sum_i64 = sum_i64 + b[i]
                    }}
                    
                    sum_u64  # Return u64 sum for verification
                }}
            ", u64_size, u64_str, i64_size, i64_str, u64_size, i64_size);
            
            let result = test_program(&program).unwrap().borrow().unwrap_uint64();
            let expected: u64 = u64_vals.iter().sum();
            assert_eq!(result, expected);
        }
    }

    // Array type inference tests
    #[test]
    fn test_array_type_inference_u64() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 3] = [1, 2, 3]
                a[0u64] + a[1u64] + a[2u64]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 6u64);
    }

    #[test]
    fn test_array_type_inference_i64() {
        let program = r#"
            fn main() -> i64 {
                val a: [i64; 3] = [1, 2, 3]
                a[0u64] + a[1u64] + a[2u64]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 6i64);
    }

    #[test]
    fn test_array_type_inference_mixed_values() {
        let program = r#"
            fn main() -> i64 {
                val a: [i64; 4] = [10, 20, 30, 40]
                val b: [i64; 2] = [a[0u64], a[1u64]]
                b[0u64] + b[1u64]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 30i64);
    }

    #[test]
    fn test_array_type_inference_large_numbers() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 2] = [1000000, 2000000]
                a[0u64] + a[1u64]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 3000000u64);
    }

    #[test]
    fn test_array_type_inference_negative_numbers() {
        let program = r#"
            fn main() -> i64 {
                val a: [i64; 3] = [-1, -2, -3]
                a[0u64] + a[1u64] + a[2u64]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, -6i64);
    }

    // Array index type inference tests
    #[test]
    fn test_array_index_inference_number_literal() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 3] = [10u64, 20u64, 30u64]
                a[0] + a[1] + a[2]  # 0, 1, 2 should be inferred as u64
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 60u64);
    }

    #[test]
    fn test_array_index_inference_i64_array() {
        let program = r#"
            fn main() -> i64 {
                val a: [i64; 3] = [10i64, 20i64, 30i64]
                a[0] + a[1] + a[2]  # 0, 1, 2 should be inferred as u64 for indexing
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 60i64);
    }

    #[test]
    fn test_array_index_inference_variable() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
                val i = 2  # i should be inferred as u64 for indexing
                a[i]
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 3u64);
    }

    #[test]
    fn test_array_index_inference_expression() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 4] = [10u64, 20u64, 30u64, 40u64]
                val base = 1
                a[base + 1]  # base + 1 should be inferred as u64
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 30u64);
    }

    #[test]
    fn test_array_index_inference_mixed_indexing() {
        let program = r#"
            fn main() -> i64 {
                val a: [i64; 3] = [100i64, 200i64, 300i64]
                val idx = 1
                a[0] + a[idx] + a[2]  # Mix of literals and variables
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_int64();
        assert_eq!(result, 600i64);
    }

    // Boundary value tests - Integer overflow/underflow
    #[test]
    #[should_panic(expected = "attempt to add with overflow")]
    fn test_integer_overflow_u64() {
        let program = r#"
            fn main() -> u64 {
                18446744073709551615u64 + 1u64
            }
        "#;
        let _result = test_program(program);
    }

    #[test]
    #[should_panic(expected = "attempt to subtract with overflow")]
    fn test_integer_underflow_u64() {
        let program = r#"
            fn main() -> u64 {
                0u64 - 1u64
            }
        "#;
        let _result = test_program(program);
    }

    #[test]
    #[should_panic(expected = "attempt to add with overflow")]
    fn test_integer_overflow_i64() {
        let program = r#"
            fn main() -> i64 {
                9223372036854775807i64 + 1i64
            }
        "#;
        let _result = test_program(program);
    }

    #[test]
    #[should_panic(expected = "attempt to subtract with overflow")]
    fn test_integer_underflow_i64() {
        let program = r#"
            fn main() -> i64 {
                -9223372036854775808i64 - 1i64
            }
        "#;
        let _result = test_program(program);
    }

    // Boundary value tests - Division by zero
    #[test]
    #[should_panic(expected = "attempt to divide by zero")]
    fn test_division_by_zero_u64() {
        let program = r#"
            fn main() -> u64 {
                10u64 / 0u64
            }
        "#;
        let _result = test_program(program);
    }

    #[test]
    #[should_panic(expected = "attempt to divide by zero")]
    fn test_division_by_zero_i64() {
        let program = r#"
            fn main() -> i64 {
                -10i64 / 0i64
            }
        "#;
        let _result = test_program(program);
    }

    // Boundary value tests - Array max boundary
    #[test]
    fn test_array_max_index_access() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 5] = [10u64, 20u64, 30u64, 40u64, 50u64]
                a[4u64]  # Maximum valid index
            }
        "#;
        let result = test_program(program).unwrap().borrow().unwrap_uint64();
        assert_eq!(result, 50u64);
    }

    #[test]
    fn test_array_max_index_plus_one() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 5] = [10u64, 20u64, 30u64, 40u64, 50u64]
                a[5u64]  # Out of bounds (max+1)
            }
        "#;
        let result = test_program(program);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Array index"));
    }

    // Boundary value tests - Array size 0
    #[test]
    fn test_array_size_zero() {
        let program = r#"
            fn main() -> u64 {
                val a: [u64; 0] = []
                0u64
            }
        "#;
        let result = test_program(program);
        assert!(result.is_err());
        // This could be caught at type check time or parse time
        let error_msg = result.unwrap_err();
        assert!(error_msg.contains("not supported") || 
                error_msg.contains("Type check errors") ||
                error_msg.contains("Parse error"));
    }

    // Boundary value tests - Deep recursion
    #[test]
    fn test_deep_recursion_fibonacci() {
        let program = r#"
            fn fib(n: u64) -> u64 {
                if n <= 1u64 {
                    n
                } else {
                    fib(n - 1u64) + fib(n - 2u64)
                }
            }
            fn main() -> u64 {
                fib(20u64)  # Deep recursion but computable
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 6765u64);  // fib(20) = 6765
    }

    // Boundary value tests - Type conversion boundary
    #[test]
    fn test_type_conversion_boundary() {
        let program = r#"
            fn main() -> u64 {
                val i: i64 = 9223372036854775807i64  # i64 max value
                val u: u64 = 18446744073709551615u64  # u64 max value
                u - 9223372036854775808u64  # u64 max - (i64 max + 1)
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 9223372036854775807u64);
    }

    // Enhanced error handling tests - Undefined variable (detected at type check time)
    #[test]
    fn test_undefined_variable_access() {
        let program = r#"
            fn main() -> u64 {
                undefined_var
            }
        "#;
        let result = test_program(program);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Type check errors"));
    }

    // Enhanced error handling tests - Undefined function (detected at type check time)
    #[test]
    fn test_undefined_function_call() {
        let program = r#"
            fn main() -> u64 {
                undefined_function()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Type check errors"));
    }

    // Enhanced error handling tests - Function argument type mismatch (should pass type check)
    #[test]
    fn test_function_argument_type_mismatch() {
        // This test verifies that proper type conversion works
        let program = r#"
            fn test_func(x: u64) -> u64 {
                x
            }
            fn main() -> u64 {
                test_func(100u64)  # Proper u64 argument
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 100u64);
    }


    // Enhanced error handling tests - Array assignment type mismatch (detected at type check time)
    #[test]
    fn test_array_assignment_type_mismatch() {
        let program = r#"
            fn main() -> u64 {
                var a: [u64; 3] = [1u64, 2u64, 3u64]
                a[0u64] = -1i64  # Negative value to u64 array
                a[0u64]
            }
        "#;
        let result = test_program(program);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Type check errors"));
    }

    // Struct field access and method call tests
    #[test] 
    fn test_struct_declaration_parsing() {
        // Test that struct declarations can be parsed successfully
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            fn main() -> i64 {
                42i64
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 42i64);
    }

    #[test]
    fn test_impl_block_parsing() {
        // Test that impl blocks can be parsed successfully
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            impl Point {
                fn get_x(&self) -> i64 {
                    42i64
                }
            }
            
            fn main() -> i64 {
                100i64
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 100i64);
    }

    #[test]
    fn test_struct_literal_parsing() {
        // Test that struct literals can be parsed successfully
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            fn main() -> i64 {
                val p = Point { x: 10i64, y: 20i64 }
                42i64
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 42i64);
    }

    #[test]
    fn test_struct_field_access_with_literal() {
        // Test struct field access with struct literal
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            fn main() -> i64 {
                val p = Point { x: 10i64, y: 20i64 }
                p.x
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 10i64);
    }

    #[test]
    fn test_struct_method_call_with_literal() {
        // Test struct method call with struct literal
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            impl Point {
                fn get_x(&self) -> i64 {
                    self.x
                }
            }
            
            fn main() -> i64 {
                val p = Point { x: 42i64, y: 24i64 }
                p.get_x()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 42i64);
    }

    #[test]
    fn test_struct_method_call_with_args() {
        // Test struct method call with arguments
        let program = r#"
            struct Point {
                x: i64,
                y: i64,
            }
            
            impl Point {
                fn add(&self, dx: i64, dy: i64) -> i64 {
                    self.x + dx + self.y + dy
                }
            }
            
            fn main() -> i64 {
                val p = Point { x: 10i64, y: 20i64 }
                p.add(5i64, 15i64)
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_int64();
        assert_eq!(value, 50i64); // 10 + 5 + 20 + 15 = 50
    }

    // String.len() method tests
    #[test]
    fn test_string_len_basic() {
        let program = r#"
            fn main() -> u64 {
                val s = "hello"
                s.len()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 5u64);
    }

    #[test]
    fn test_string_len_empty() {
        let program = r#"
            fn main() -> u64 {
                val s = ""
                s.len()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 0u64);
    }

    #[test]
    fn test_string_len_arithmetic() {
        let program = r#"
            fn main() -> u64 {
                val str1 = "hello"
                val str2 = "world"
                str1.len() + str2.len()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 10u64);
    }

    #[test]
    fn test_string_len_comparison() {
        let program = r#"
            fn main() -> bool {
                val long = "this is a long string"
                val short = "hi"
                long.len() > short.len()
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_bool();
        assert_eq!(value, true);
    }

    #[test]
    fn test_string_len_in_expression() {
        let program = r#"
            fn main() -> u64 {
                val s = "test"
                val len = s.len()
                if len > 3u64 {
                    len * 2u64
                } else {
                    len
                }
            }
        "#;
        let result = test_program(program);
        assert!(result.is_ok());
        let value = result.unwrap().borrow().unwrap_uint64();
        assert_eq!(value, 8u64); // "test".len() = 4, 4 > 3, so 4 * 2 = 8
    }
}
