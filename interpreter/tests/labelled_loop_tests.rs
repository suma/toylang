// Labelled break / continue tests (LABEL feature).
//
// `@label: while/for ...` decorates a loop with a name; `break @label`
// / `continue @label` then targets that loop directly instead of the
// innermost one. Bare `break` / `continue` keep the original
// innermost-loop semantics. Validation lives in the type checker:
// undefined labels and `break` / `continue` outside of any loop both
// fail to type-check (see `typecheck_*_rejected` cases at the bottom).

mod common;

use common::{assert_program_result_i64, assert_program_result_u64, test_program};

// ---------------------------------------------------------------------
// Runtime: nested labelled break (interpreter / AOT / JIT all share
// the same observable behavior, exercised here through the interpreter
// path; cross-backend pinning lives in compiler/tests/consistency.rs).
// ---------------------------------------------------------------------

#[test]
fn labelled_break_exits_outer_for() {
    let src = r#"
        fn search() -> i64 {
            @outer: for i in 0i64 to 10i64 {
                for j in 0i64 to 10i64 {
                    if i * j == 42i64 {
                        return i * 100i64 + j
                    }
                    if j > 5i64 { break @outer }
                }
            }
            -1i64
        }

        fn main() -> i64 { search() }
    "#;
    // Inner loop breaks outer at i==0,j==6, so we never reach i*j==42.
    assert_program_result_i64(src, -1i64);
}

#[test]
fn labelled_continue_skips_outer_iteration() {
    let src = r#"
        fn count() -> u64 {
            var hits: u64 = 0u64
            @outer: for i in 0u64 to 5u64 {
                for j in 0u64 to 5u64 {
                    if j == 2u64 {
                        continue @outer
                    } else {
                        hits = hits + 1u64
                    }
                }
            }
            hits
        }

        fn main() -> u64 { count() }
    "#;
    // Each outer iter contributes exactly 2 hits (j=0,1) before the
    // labelled continue skips to the next i. 5 outer iters * 2 = 10.
    assert_program_result_u64(src, 10u64);
}

#[test]
fn labelled_break_in_while_inside_for() {
    let src = r#"
        fn run() -> i64 {
            var total: i64 = 0i64
            @grid: for i in 0i64 to 4i64 {
                var j: i64 = 0i64
                while j < 4i64 {
                    if i + j == 4i64 { break @grid }
                    total = total + 1i64
                    j = j + 1i64
                }
            }
            total
        }

        fn main() -> i64 { run() }
    "#;
    // Iteration order: i=0 j=0,1,2,3 (sums 0,1,2,3 — none==4) -> +4
    //                  i=1 j=0,1,2 (sums 1,2,3) -> +3, then j=3 sum==4 -> break @grid
    // Total = 7.
    assert_program_result_i64(src, 7i64);
}

#[test]
fn bare_break_inside_labelled_loop_targets_innermost() {
    let src = r#"
        fn count() -> u64 {
            var total: u64 = 0u64
            @outer: for i in 0u64 to 3u64 {
                for j in 0u64 to 5u64 {
                    if j == 2u64 {
                        break
                    } else {
                        total = total + 1u64
                    }
                }
                total = total + 100u64
            }
            total
        }

        fn main() -> u64 { count() }
    "#;
    // Each outer iter: inner contributes 2 hits (j=0,1), then +100. 3*(2+100)=306.
    assert_program_result_u64(src, 306u64);
}

#[test]
fn labelled_break_at_outermost_label() {
    let src = r#"
        fn run() -> u64 {
            @a: for i in 0u64 to 5u64 {
                @b: for j in 0u64 to 5u64 {
                    @c: for k in 0u64 to 5u64 {
                        if i == 1u64 && j == 2u64 && k == 3u64 {
                            break @a
                        }
                    }
                }
            }
            42u64
        }

        fn main() -> u64 { run() }
    "#;
    assert_program_result_u64(src, 42u64);
}

// ---------------------------------------------------------------------
// Validation: type-checker errors for unlabelled `break` outside any
// loop and labelled `break` / `continue` referencing an undefined name.
// ---------------------------------------------------------------------

#[test]
fn typecheck_break_outside_loop_rejected() {
    let src = r#"
        fn main() -> i64 {
            break
            0i64
        }
    "#;
    let err = test_program(src).expect_err("expected type-check failure for break outside loop");
    assert!(err.contains("outside") && err.contains("loop"), "actual: {err}");
}

#[test]
fn typecheck_undefined_label_rejected() {
    let src = r#"
        fn main() -> i64 {
            for i in 0i64 to 3i64 {
                break @missing
            }
            0i64
        }
    "#;
    let err = test_program(src).expect_err("expected type-check failure for undefined label");
    assert!(err.contains("undefined loop label") && err.contains("@missing"), "actual: {err}");
}
