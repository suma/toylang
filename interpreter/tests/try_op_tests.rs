// `?` postfix early-return operator tests.
//
// Pure parser-emitted `Expr::Try { inner, .. }` is rewritten by the
// type checker into a `match` over `Result<T, E>` or `Option<T>` whose
// error arm `return`s the propagating value. Backends (interpreter,
// AOT, JIT) therefore see only the desugared form — these tests
// exercise the construct via the interpreter end-to-end.


use crate::common::{assert_program_result_i64, assert_program_result_u64, test_program};

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

#[test]
fn try_op_cross_error_converts_via_from() {
    // From/Into cross-error conversion: `?` inside a function that
    // returns `Result<T, ErrWrap>` converts the inner `str` error
    // through `ErrWrap: From<str>` before re-returning.
    let src = r#"
        struct ErrWrap { code: u64 }

        impl From<str> for ErrWrap {
            fn from(value: str) -> ErrWrap {
                val r: ErrWrap = ErrWrap { code: 42u64 }
                r
            }
        }

        fn inner() -> Result<i64, str> {
            Result::Err("boom")
        }

        fn outer() -> Result<i64, ErrWrap> {
            val x = inner()?
            Result::Ok(x)
        }

        fn main() -> i64 {
            match outer() {
                Result::Err(w) => w.code as i64,
                Result::Ok(_) => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 42i64);
}

#[test]
fn try_op_cross_error_converts_enum_target() {
    // Cross-error conversion into an enum error type. FROM-INTO-ENUM-ERR
    // (2026-08-30): the let-rhs dispatch tells variant constructions
    // from associated functions, so `MyErr::from(...)` lowers on every
    // backend — this is also pinned 3-way by
    // `consistency::impls_refs::try_cross_error_into_enum_target`.
    let src = r#"
        enum MyErr {
            Fail(u64),
        }

        impl From<str> for MyErr {
            fn from(value: str) -> MyErr {
                val r: MyErr = MyErr::Fail(7u64)
                r
            }
        }

        fn inner() -> Result<i64, str> {
            Result::Err("boom")
        }

        fn outer() -> Result<i64, MyErr> {
            val x = inner()?
            Result::Ok(x)
        }

        fn main() -> i64 {
            match outer() {
                Result::Err(MyErr::Fail(v)) => v as i64,
                _ => -1i64,
            }
        }
    "#;
    assert_program_result_i64(src, 7i64);
}

#[test]
fn try_op_cross_error_no_from_is_a_type_error() {
    // No `From` impl for the mismatched error types: the `?` desugar
    // leaves the inner type alone and the enclosing return-type check
    // rejects the program.
    let src = r#"
        struct ErrWrap { code: u64 }

        fn inner() -> Result<i64, str> {
            Result::Err("boom")
        }

        fn outer() -> Result<i64, ErrWrap> {
            val x = inner()?
            Result::Ok(x)
        }

        fn main() -> i64 {
            match outer() {
                Result::Err(_) => -1i64,
                Result::Ok(v) => v,
            }
        }
    "#;
    assert!(test_program(src).is_err(), "mismatched error types without From must fail");
}

// ---------------------------------------------------------------------
// TRY-ERR-RETYPE: `?` across a *success*-type change. The error arm
// reconstructs the error variant against the enclosing function's
// declared return type instead of re-returning the scrutinee, so the
// program is sound end to end.
// ---------------------------------------------------------------------

#[test]
fn try_op_across_success_type_change_propagates_err() {
    // `read_line-ish` shape: inner `Result<str, str>`, outer
    // `Result<u64, str>`. The Err path must reach main with its
    // payload intact.
    let src = r#"
        fn first_byte(s: str) -> Result<u64, str> {
            val text: str = inner_text(s)?
            Result::Ok(text.len())
        }

        fn inner_text(s: str) -> Result<str, str> {
            if s == "" {
                Result::Err("empty")
            } else {
                Result::Ok(s)
            }
        }

        fn main() -> i64 {
            match first_byte("") {
                Result::Err(reason) => if reason == "empty" { -1i64 } else { -2i64 },
                Result::Ok(_) => -3i64,
            }
        }
    "#;
    assert_program_result_i64(src, -1i64);
}

#[test]
fn try_op_across_success_type_change_unwraps_ok() {
    let src = r#"
        fn text_len(s: str) -> Result<u64, str> {
            val text: str = inner_text(s)?
            Result::Ok(text.len())
        }

        fn inner_text(s: str) -> Result<str, str> {
            if s == "" {
                Result::Err("empty")
            } else {
                Result::Ok(s)
            }
        }

        fn main() -> u64 {
            match text_len("hello") {
                Result::Ok(n) => n,
                Result::Err(_) => 0u64,
            }
        }
    "#;
    assert_program_result_u64(src, 5u64);
}

#[test]
fn try_op_across_success_type_change_option() {
    // Same shape on `Option`: `Option<str>` feeding `Option<u64>`;
    // `None` reconstructs the unit variant against the outer type.
    let src = r#"
        fn wrapped(s: str) -> Option<u64> {
            val text: str = inner_opt(s)?
            Option::Some(text.len())
        }

        fn inner_opt(s: str) -> Option<str> {
            if s == "" {
                Option::None
            } else {
                Option::Some(s)
            }
        }

        fn main() -> u64 {
            match wrapped("abcd") {
                Option::Some(n) => n,
                Option::None => 0u64,
            }
        }
    "#;
    assert_program_result_u64(src, 4u64);
}

// ---------------------------------------------------------------------
// TRY-ERR-RETYPE: `return` is now checked against the enclosing
// function's declared return type.
// ---------------------------------------------------------------------

#[test]
fn return_type_mismatch_is_a_type_error() {
    let src = r#"
        fn bad() -> u64 {
            return "nope"
        }

        fn main() -> u64 { bad() }
    "#;
    assert!(test_program(src).is_err(), "return of the wrong type must fail type checking");
}

#[test]
fn return_in_unit_function_with_value_is_a_type_error() {
    let src = r#"
        fn side() {
            return 5u64
        }

        fn main() -> u64 { side() }
    "#;
    assert!(test_program(src).is_err(), "returning a value from a Unit fn must fail type checking");
}

#[test]
fn try_in_unit_function_is_a_type_error() {
    // `?` propagates by returning the inner Result — impossible from a
    // fn that returns nothing. Used to slip through the checker and
    // misbehave in the backends; now a clean type error (Rust agrees).
    let src = r#"
        fn probe(p: str) -> Result<str, str> {
            Result::Ok(p)
        }

        fn log_it(p: str) {
            val x: str = probe(p)?
            println(x)
        }

        fn main() -> u64 { 0u64 }
    "#;
    assert!(test_program(src).is_err(), "`?` inside a Unit fn must fail type checking");
}

// ---------------------------------------------------------------------
// `?` inside a stdlib body.
// ---------------------------------------------------------------------

#[test]
fn try_in_a_stdlib_body_survives_a_user_shadow_of_result() {
    // A stdlib module that uses `?` is integrated into the user's
    // program, and when the user has a `Result` of their own the
    // stdlib's is re-interned as `__std_Result`
    // (DICT-CROSS-MODULE-OPTION). The desugar classified the enum by
    // its interned spelling, so `json::parse` -- which propagates with
    // `?` -- failed with "`?` requires Result or Option, got enum
    // `__std_Result`", but only in programs that shadow. The shadow is
    // a struct here, so nothing about the user's type could stand in
    // for the stdlib enum.
    let src = r#"
        struct Result<T, E> { ok: bool, value: T, error: E }

        fn main() -> u64 {
            val mine: Result<u64, u64> = Result { ok: true, value: 7u64, error: 0u64 }
            val doc = json::parse("[1, 2, 3]")
            var n: u64 = 0u64
            match doc {
                __std_Result::Ok(d) => { n = d.size() }
                __std_Result::Err(e) => { n = 0u64 }
            }
            n + mine.value
        }
    "#;
    assert_program_result_u64(src, 11);
}
