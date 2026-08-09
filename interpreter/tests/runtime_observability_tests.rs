// LLM-LOOP P6 — a runtime failure says where it happened and why.
//
// Before this, `panic("boom")` reported exactly `panic: boom`: no line,
// no call path. A contract violation named the clause by index but not
// the values that made it false. Both left one option — add prints and
// run again — which is the round trip the whole LLM-LOOP effort exists
// to remove.

mod common;

use common::test_program;

/// Run a program expected to fail at runtime and return the diagnostic.
fn runtime_failure(source: &str) -> String {
    match test_program(source) {
        Ok(v) => panic!("expected a runtime failure, got {v:?}"),
        Err(e) => e,
    }
}

// --- panic ------------------------------------------------------------

#[test]
fn panic_reports_the_line_it_fired_on() {
    let diags = runtime_failure(
        "fn boom() -> u64 {
            panic(\"kaboom\")
        }
        fn main() -> u64 { boom() }",
    );
    assert!(diags.contains("kaboom"), "{diags}");
    assert!(diags.contains("test.t:2:"), "expected the panic's own line:\n{diags}");
}

#[test]
fn panic_reports_the_call_path_that_reached_it() {
    // The question a bare message cannot answer: *which* path got here.
    let diags = runtime_failure(
        "fn inner(n: u64) -> u64 { panic(\"deep\") }
        fn middle(n: u64) -> u64 { inner(n) }
        fn main() -> u64 { middle(3u64) }",
    );
    assert!(diags.contains("backtrace"), "{diags}");
    let inner_at = diags
        .find("inner")
        .unwrap_or_else(|| panic!("no `inner` frame:\n{diags}"));
    let middle_at = diags
        .find("middle")
        .unwrap_or_else(|| panic!("no `middle` frame:\n{diags}"));
    assert!(
        inner_at < middle_at,
        "backtrace should be innermost first:\n{diags}"
    );
}

#[test]
fn a_failed_assert_reports_like_a_panic() {
    let diags = runtime_failure(
        "fn check(n: u64) -> u64 {
            assert(n > 10u64, \"n too small\")
            n
        }
        fn main() -> u64 { check(3u64) }",
    );
    assert!(diags.contains("n too small"), "{diags}");
    assert!(diags.contains("test.t:2:"), "{diags}");
    assert!(diags.contains("check"), "{diags}");
}

#[test]
fn a_passing_assert_costs_nothing_observable() {
    common::assert_program_result_u64(
        "fn check(n: u64) -> u64 {
            assert(n > 1u64, \"n too small\")
            n
        }
        fn main() -> u64 { check(42u64) }",
        42,
    );
}

// --- contracts --------------------------------------------------------

#[test]
fn a_requires_violation_reports_the_argument_values() {
    // `clause #1 evaluated to false` says which predicate failed; the
    // values say what to fix.
    let diags = runtime_failure(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 0i64) }",
    );
    assert!(diags.contains("requires"), "{diags}");
    assert!(diags.contains("a = 20"), "argument values missing:\n{diags}");
    assert!(diags.contains("b = 0"), "argument values missing:\n{diags}");
}

#[test]
fn an_ensures_violation_reports_the_result_too() {
    let diags = runtime_failure(
        "fn buggy_abs(x: i64) -> i64
            ensures result >= 0i64
        {
            -x
        }
        fn main() -> i64 { buggy_abs(5i64) }",
    );
    assert!(diags.contains("ensures"), "{diags}");
    assert!(diags.contains("x = 5"), "{diags}");
    assert!(diags.contains("result = -5"), "the returned value is the point:\n{diags}");
}

#[test]
fn a_satisfied_contract_stays_quiet() {
    common::assert_program_result_i64(
        "fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(20i64, 4i64) }",
        5,
    );
}

// --- arithmetic guards (P6-3) ----------------------------------------

#[test]
fn u64_subtraction_underflow_traps_instead_of_wrapping() {
    // `0u64 - 1u64` wrapping to 18446744073709551615 is a favourite way
    // to lose an afternoon: the value looks like a plausible large
    // number, so the symptom shows up far from the cause.
    let diags = runtime_failure(
        "fn main() -> u64 {
            val a: u64 = 0u64
            val b: u64 = 1u64
            a - b
        }",
    );
    assert!(diags.contains("underflow"), "{diags}");
    // Values and position come from the panic path (P6-1 / P6-2).
    assert!(diags.contains("0 - 1"), "operands should be named:\n{diags}");
    assert!(diags.contains("test.t:4:"), "{diags}");
}

#[test]
fn a_subtraction_that_fits_is_unaffected() {
    common::assert_program_result_u64(
        "fn main() -> u64 {
            val a: u64 = 5u64
            val b: u64 = 3u64
            a - b
        }",
        2,
    );
}

#[test]
fn signed_subtraction_going_negative_is_not_an_error() {
    // Only *unsigned* subtraction is guarded: `i64` has somewhere to go.
    common::assert_program_result_i64(
        "fn main() -> i64 {
            val a: i64 = 3i64
            val b: i64 = 10i64
            a - b
        }",
        -7,
    );
}

#[test]
fn u64_addition_still_wraps() {
    // Deliberately unguarded so far — this pins the current boundary of
    // P6-3 rather than endorsing it. `u64::MAX + 5` wraps to 4.
    common::assert_program_result_u64(
        "fn main() -> u64 {
            val a: u64 = 18446744073709551615u64
            a + 5u64
        }",
        4,
    );
}
