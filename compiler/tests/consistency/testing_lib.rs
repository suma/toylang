//! TEST-TOOL T3: `core/std/testing.t`.
//!
//! The point of these is the *message*. `poc/logsearch` wrote all of
//! them by hand and every hand-written form had the same defect: it
//! said the check failed without saying what the values were. So what
//! is pinned here is that a failure names the position and both
//! sides — a 4 MiB segment that is one byte wrong is not helped by
//! being told that it is wrong.
//!
//! The passing paths are pinned across the lanes as usual; the
//! failure text is checked on one lane, since a panic message is
//! runtime-shared (`toy_panic`) and the lanes cannot render it
//! differently.

use super::harness::{assert_consistent, interpreter_error};

#[test]
fn the_assertions_pass_on_every_lane() {
    let src = r#"
        fn main() -> u64 {
            var acc: u64 = 0u64
            # Floats never compare exactly after a computation; the
            # tolerance form is the only one worth writing.
            testing::assert_close(0.1f64 + 0.2f64, 0.3f64, 0.000001f64)
            acc = acc + 1u64

            testing::assert_str_eq("abc", "abc")
            acc = acc + 2u64

            testing::assert_in_range(5i64, 1i64, 10i64)
            # Inclusive at both ends: the endpoints of a measurement
            # are legitimate answers.
            testing::assert_in_range(1i64, 1i64, 10i64)
            testing::assert_in_range(10i64, 1i64, 10i64)
            testing::assert_in_range_u64(7u64, 7u64, 7u64)
            acc = acc + 4u64

            val some: Option<u64> = Option::Some(9u64)
            acc = acc + testing::assert_some(some, "a value")

            val ok: Result<u64, str> = Result::Ok(3u64)
            acc = acc + testing::assert_ok(ok, "a call")

            val bad: Result<u64, u64> = Result::Err(11u64)
            acc = acc + testing::assert_err(bad, "a failing call")

            # Bytes: equal windows compare equal, and the check costs
            # one `bytes_eq` when it passes.
            var a: Vec<u8> = Vec::with_capacity(4u64)
            var b: Vec<u8> = Vec::with_capacity(4u64)
            var i: u64 = 0u64
            while i < 4u64 {
                a.push(i as u8)
                b.push(i as u8)
                i = i + 1u64
            }
            val sa: Option<Span<u8>> = a.as_span()
            val sb: Option<Span<u8>> = b.as_span()
            match sa {
                Option::Some(x) => {
                    match sb {
                        Option::Some(y) => {
                            testing::assert_bytes_eq(x, y)
                            acc = acc + 8u64
                        }
                        Option::None => { }
                    }
                }
                Option::None => { }
            }
            acc
        }
    "#;
    assert_consistent(src, "testing_lib_pass");
}

#[test]
fn a_byte_difference_is_reported_by_offset() {
    // The design's motivating case, in miniature.
    let err = interpreter_error(
        r#"
        fn main() -> u64 {
            var a: Vec<u8> = Vec::with_capacity(8u64)
            var b: Vec<u8> = Vec::with_capacity(8u64)
            var i: u64 = 0u64
            while i < 8u64 {
                a.push(i as u8)
                b.push(i as u8)
                i = i + 1u64
            }
            b.set(5u64, 99u8)
            val sa: Option<Span<u8>> = a.as_span()
            val sb: Option<Span<u8>> = b.as_span()
            match sa {
                Option::Some(x) => {
                    match sb {
                        Option::Some(y) => { testing::assert_bytes_eq(x, y) }
                        Option::None => { }
                    }
                }
                Option::None => { }
            }
            0u64
        }
    "#,
    );
    assert!(
        err.contains("byte 5 differs") && err.contains("left 5") && err.contains("right 99"),
        "the offset and both bytes have to be in the message: {err}"
    );
}

#[test]
fn a_length_difference_is_reported_as_a_length() {
    let err = interpreter_error(
        r#"
        fn main() -> u64 {
            testing::assert_str_eq("abc", "abcd")
            0u64
        }
    "#,
    );
    assert!(
        err.contains("left is 3 bytes") && err.contains("right is 4 bytes"),
        "{err}"
    );
}

#[test]
fn a_float_failure_carries_the_difference() {
    // Which is what separates "the tolerance is too tight" from "the
    // answer is wrong" — the two readings a reader has to choose
    // between.
    let err = interpreter_error(
        r#"
        fn main() -> u64 {
            testing::assert_close(1.0f64, 1.5f64, 0.01f64)
            0u64
        }
    "#,
    );
    assert!(
        err.contains("left 1.0") && err.contains("right 1.5") && err.contains("differ by 0.5"),
        "{err}"
    );
}

#[test]
fn the_heap_mark_measures_the_interval_not_the_function() {
    // `ensures allocates(N)` states a *function's* budget; this states
    // a *test's*, over an interval inside one block. A helper that
    // allocates and frees has not grown the heap, which is why the
    // counter read is `live_bytes` rather than the cumulative one.
    let err = interpreter_error(
        r#"
        fn main() -> u64 {
            val m: u64 = testing::heap_mark()
            var v: Vec<u64> = Vec::with_capacity(64u64)
            v.push(1u64)
            testing::assert_no_growth(m)
            0u64
        }
    "#,
    );
    assert!(
        err.contains("assert_no_growth failed") && err.contains("heap grew by 512 bytes"),
        "the message has to say how much: {err}"
    );
}

#[test]
fn a_budget_reports_how_far_over_it_went() {
    let err = interpreter_error(
        r#"
        fn main() -> u64 {
            val m: u64 = testing::heap_mark()
            var v: Vec<u64> = Vec::with_capacity(64u64)
            v.push(1u64)
            testing::assert_growth_at_most(m, 100u64)
            0u64
        }
    "#,
    );
    assert!(
        err.contains("grew 512 bytes") && err.contains("over by 412"),
        "{err}"
    );
}

#[test]
fn growth_within_the_budget_passes() {
    assert_consistent(
        r#"
        fn main() -> u64 {
            val m: u64 = testing::heap_mark()
            var v: Vec<u64> = Vec::with_capacity(64u64)
            v.push(1u64)
            testing::assert_growth_at_most(m, 4096u64)
            0u64
        }
    "#,
        "testing_lib_budget_ok",
    );
}
