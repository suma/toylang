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
        println!("parser_program failed {:?}", program.unwrap_err());
        return;
    }

    let program = program.unwrap();

    if let Err(errors) = interpreter::check_typing(&program) {
        for e in errors {
            eprintln!("{}", e);
        }
        return;
    }

    let res = interpreter::execute_program(&program);
    if res.is_ok() {
        println!("Result: {:?}", res.unwrap());
    } else {
        eprintln!("execute_program failed: {:?}", res.unwrap_err());
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
    use interpreter::object::{Object, RcObject};
    use interpreter::error::InterpreterError;
    use interpreter::evaluation::{EvaluationContext, convert_object, EvaluationResult};

    #[test]
    fn test_evaluate_integer() {
        let stmt_pool = StmtPool::new();
        let mut expr_pool = ExprPool::new();
        let expr_ref = expr_pool.add(Expr::Int64(42));
        let mut interner = DefaultStringInterner::new();

        let mut ctx = EvaluationContext::new(&stmt_pool, &expr_pool, &mut interner, HashMap::new());
        let result = match ctx.evaluate(&expr_ref) {
            Ok(EvaluationResult::Value(v)) => v,
            _ => panic!("evaluate should return int64 value"),
        };

        assert_eq!(result.borrow().unwrap_int64(), 42);
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

        let res = interpreter::execute_program(&program);
        assert!(res.is_ok());
        assert_eq!(res.unwrap().borrow().unwrap_uint64(), 3);
    }

    fn test_program(program: &str) -> Result<Rc<RefCell<Object>>, InterpreterError> {
        let mut parser = frontend::Parser::new(program);
        let program = parser.parse_program();
        assert!(program.is_ok());
        let res = interpreter::execute_program(&program.unwrap());
        assert!(res.is_ok());
        Ok(res.unwrap())
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
}