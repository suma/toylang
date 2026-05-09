// `if val` / `while val` (IF-VAL feature) tests.
//
// Toylang's pattern-binding-with-conditional construct. `if val PAT = EXPR`
// desugars to a two-arm `match` (PAT => then, _ => else). `while val PAT =
// EXPR` desugars to `while true { match EXPR { PAT => body, _ => break } }`.
// Toylang uses `val` (not `let`) as its immutable-binding keyword; the
// pattern-binding construct follows the same convention for consistency.
//
// Both forms are pure parser desugar — type checker / interpreter / AOT /
// JIT all see plain `match` and `while` and need no further work. These
// tests exercise the desugar via end-to-end interpreter runs.

mod common;

use common::{assert_program_result_i64, assert_program_result_u64, test_program};

// ---------------------------------------------------------------------
// `if val` — happy paths.
// ---------------------------------------------------------------------

#[test]
fn if_val_some_binds_inner_value() {
    let src = r#"
        fn main() -> i64 {
            val opt: Option<i64> = Option::Some(42i64)
            if val Option::Some(x) = opt {
                x
            } else {
                -1i64
            }
        }
    "#;
    assert_program_result_i64(src, 42i64);
}

#[test]
fn if_val_none_falls_through_to_else() {
    let src = r#"
        fn main() -> i64 {
            val opt: Option<i64> = Option::None
            if val Option::Some(x) = opt {
                x
            } else {
                -1i64
            }
        }
    "#;
    assert_program_result_i64(src, -1i64);
}

#[test]
fn if_val_no_else_branch_is_unit() {
    // Without `else`, the catch-all arm yields a Unit block. Used for
    // side-effecting forms where you only care about the matching case.
    let src = r#"
        fn main() -> u64 {
            var sum: u64 = 0u64
            val opt: Option<u64> = Option::Some(7u64)
            if val Option::Some(x) = opt {
                sum = sum + x
            }
            val miss: Option<u64> = Option::None
            if val Option::Some(y) = miss {
                sum = sum + y
            }
            sum
        }
    "#;
    assert_program_result_u64(src, 7u64);
}

#[test]
fn if_val_result_ok_path() {
    let src = r#"
        fn main() -> i64 {
            val r: Result<i64, str> = Result::Ok(99i64)
            if val Result::Ok(v) = r {
                v
            } else {
                -1i64
            }
        }
    "#;
    assert_program_result_i64(src, 99i64);
}

#[test]
fn if_val_user_enum_variant() {
    let src = r#"
        enum Shape {
            Circle(i64),
            Square(i64),
            Point,
        }

        fn main() -> i64 {
            val s: Shape = Shape::Circle(5i64)
            if val Shape::Circle(r) = s {
                r * r
            } else {
                0i64
            }
        }
    "#;
    assert_program_result_i64(src, 25i64);
}

// ---------------------------------------------------------------------
// `while val` — drains an iterator-style enum until None.
// ---------------------------------------------------------------------

#[test]
fn while_val_drains_counter() {
    // Pull values out of a hand-rolled "iterator" struct. We simulate the
    // typical `while let Some(x) = iter.next()` pattern by maintaining a
    // var counter that flips to None when exhausted. (We can't write a
    // full iterator here without redefining `next()`'s state across
    // calls, which exposes a mutability gap unrelated to IF-VAL — so use
    // the simpler scalar form.)
    let src = r#"
        fn next(n: i64) -> Option<i64> {
            if n > 0i64 {
                Option::Some(n)
            } else {
                Option::None
            }
        }

        fn main() -> i64 {
            var sum: i64 = 0i64
            var i: i64 = 5i64
            while val Option::Some(x) = next(i) {
                sum = sum + x
                i = i - 1i64
            }
            sum
        }
    "#;
    // 5+4+3+2+1 = 15
    assert_program_result_i64(src, 15i64);
}

#[test]
fn while_val_immediately_breaks_on_none() {
    let src = r#"
        fn main() -> u64 {
            var hits: u64 = 0u64
            while val Option::Some(x) = Option::None {
                hits = hits + 1u64
            }
            hits
        }
    "#;
    assert_program_result_u64(src, 0u64);
}

// ---------------------------------------------------------------------
// Composition: labelled `while val` — outer label propagates to the
// synthetic `while true` so user `break @outer` still works.
// ---------------------------------------------------------------------

#[test]
fn labelled_while_val_outer_break() {
    let src = r#"
        fn next(n: i64) -> Option<i64> {
            if n > 0i64 { Option::Some(n) } else { Option::None }
        }

        fn main() -> i64 {
            var found: i64 = -1i64
            @outer: while val Option::Some(x) = next(10i64) {
                if x == 10i64 {
                    found = x
                    break @outer
                }
            }
            found
        }
    "#;
    assert_program_result_i64(src, 10i64);
}

// ---------------------------------------------------------------------
// Negative: `if val` without `=` should be a parse error.
// (Validates the grammar didn't get over-permissive.)
// ---------------------------------------------------------------------

#[test]
fn if_val_missing_eq_rejected() {
    let src = r#"
        fn main() -> i64 {
            val opt: Option<i64> = Option::Some(1i64)
            if val Option::Some(x) opt {
                x
            } else {
                0i64
            }
        }
    "#;
    let err = test_program(src).expect_err("expected parse / type-check failure");
    assert!(err.contains("Parse") || err.contains("expected") || err.contains("Type check"), "actual: {err}");
}
