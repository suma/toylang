mod common;
use common::test_program;

use std::collections::HashMap;
use frontend::ast::*;
use string_interner::DefaultStringInterner;
use interpreter::evaluation::{EvaluationContext, EvaluationResult};

#[cfg(test)]
mod basic_tests {
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
        let mut parser = frontend::ParserWithInterner::new(r"
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
}