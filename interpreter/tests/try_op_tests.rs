// `?` postfix early-return operator tests.
//
// Pure parser-emitted `Expr::Try { inner, .. }` is rewritten by the
// type checker into a `match` over `Result<T, E>` or `Option<T>` whose
// error arm `return`s the propagating value. Backends (interpreter,
// AOT, JIT) therefore see only the desugared form — these tests
// exercise the construct via the interpreter end-to-end.

mod common;

use common::{assert_program_result_i64, assert_program_result_u64};

// ---------------------------------------------------------------------
// `?` on Result — happy and error paths.
// ---------------------------------------------------------------------

#[test]
fn try_op_result_unwraps_ok() {
    // `divide(100, 5)? + divide(20, 2)?` should evaluate to 20 + 10.
    let src = r#"
        fn divide(a: i64, b: i64) -> Result<i64, str> {
            if b == 0i64 {
                Result::Err("division by zero")
            } else {
                Result::Ok(a / b)
            }
        }

        fn compute() -> Result<i64, str> {
            val x = divide(100i64, 5i64)?
            val y = divide(20i64, 2i64)?
            Result::Ok(x + y)
        }

        fn main() -> i64 {
            match compute() {
                Result::Ok(v) => v,
                Result::Err(_) => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 30i64);
}

#[test]
fn try_op_result_propagates_err() {
    // `divide(100, 0)?` should short-circuit and propagate `Err` out of
    // `compute`. The caller's match arm produces -1.
    let src = r#"
        fn divide(a: i64, b: i64) -> Result<i64, str> {
            if b == 0i64 {
                Result::Err("division by zero")
            } else {
                Result::Ok(a / b)
            }
        }

        fn compute(d: i64) -> Result<i64, str> {
            val x = divide(100i64, d)?
            Result::Ok(x + 1i64)
        }

        fn main() -> i64 {
            match compute(0i64) {
                Result::Ok(v) => v,
                Result::Err(_) => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, -1i64);
}

#[test]
fn try_op_result_propagated_err_carries_payload() {
    // The propagated `Err` retains its payload, observable via the
    // caller's match arm binding. Use an i64 payload so the test
    // can read it back through `assert_program_result_i64` without
    // string interning gymnastics.
    let src = r#"
        fn fail() -> Result<i64, i64> {
            Result::Err(777i64)
        }

        fn wrapper() -> Result<i64, i64> {
            val x = fail()?
            Result::Ok(x)
        }

        fn main() -> i64 {
            match wrapper() {
                Result::Ok(_) => -1i64,
                Result::Err(e) => e,
            }
        }
    "#;
    assert_program_result_i64(src, 777i64);
}

// ---------------------------------------------------------------------
// `?` on Option — happy and `None` paths.
// ---------------------------------------------------------------------

#[test]
fn try_op_option_unwraps_some() {
    let src = r#"
        fn first_positive(a: i64, b: i64) -> Option<i64> {
            if a > 0i64 {
                Option::Some(a)
            } elif b > 0i64 {
                Option::Some(b)
            } else {
                Option::None
            }
        }

        fn chain(a: i64, b: i64, c: i64) -> Option<i64> {
            val x = first_positive(a, b)?
            val y = first_positive(x, c)?
            Option::Some(y + 1i64)
        }

        fn main() -> i64 {
            match chain(3i64, -1i64, 7i64) {
                Option::Some(v) => v,
                Option::None => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 4i64);
}

#[test]
fn try_op_option_propagates_none() {
    let src = r#"
        fn first_positive(a: i64, b: i64) -> Option<i64> {
            if a > 0i64 {
                Option::Some(a)
            } elif b > 0i64 {
                Option::Some(b)
            } else {
                Option::None
            }
        }

        fn chain(a: i64, b: i64) -> Option<i64> {
            val x = first_positive(a, b)?
            Option::Some(x + 1i64)
        }

        fn main() -> u64 {
            match chain(-3i64, -1i64) {
                Option::Some(v) => v as u64,
                Option::None => 99u64,
            }
        }
    "#;
    assert_program_result_u64(src, 99u64);
}

// ---------------------------------------------------------------------
// `?` chained on the same line and inside expression contexts.
// ---------------------------------------------------------------------

#[test]
fn try_op_chained_two_in_one_function() {
    // Two `?`s in the same function body, second one fires only if the
    // first one's success was extracted.
    let src = r#"
        fn step(n: i64, lim: i64) -> Result<i64, str> {
            if n < lim {
                Result::Ok(n + 1i64)
            } else {
                Result::Err("limit")
            }
        }

        fn pipeline(n: i64) -> Result<i64, str> {
            val a = step(n, 10i64)?
            val b = step(a, 10i64)?
            Result::Ok(b)
        }

        fn main() -> i64 {
            match pipeline(5i64) {
                Result::Ok(v) => v,
                Result::Err(_) => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 7i64);
}

#[test]
fn try_op_used_as_expression_argument() {
    // `?` is a postfix expression operator — must compose with normal
    // expression positions like function arguments.
    let src = r#"
        fn lookup(k: i64) -> Result<i64, str> {
            if k > 0i64 {
                Result::Ok(k * 10i64)
            } else {
                Result::Err("bad key")
            }
        }

        fn add_two(a: i64, b: i64) -> i64 {
            a + b
        }

        fn use_both(k1: i64, k2: i64) -> Result<i64, str> {
            Result::Ok(add_two(lookup(k1)?, lookup(k2)?))
        }

        fn main() -> i64 {
            match use_both(2i64, 3i64) {
                Result::Ok(v) => v,
                Result::Err(_) => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 50i64);
}
