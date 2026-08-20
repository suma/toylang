// RUNTIME-TRAP: `core/std/checked.t` — the escape hatch from wrapping
// arithmetic. Interpreter-side value tests; the 3-backend agreement is
// pinned by `interpreter/example/checked_arith.t` through
// `compiler/tests/example_consistency.rs`, and the traps these methods
// let a program avoid are pinned in `compiler/tests/consistency.rs`.

use crate::common::assert_program_result_u64;

/// `Option::Some(v) => v, Option::None => sentinel`, folded to a u64
/// so the whole family can be checked with one helper.
fn assert_checked_u64(expr: &str, expected: u64) {
    assert_program_result_u64(
        &format!(
            r#"
        fn main() -> u64 {{
            {expr}
        }}
        "#
        ),
        expected,
    );
}

#[test]
fn u64_checked_add_reports_overflow() {
    assert_checked_u64(
        r#"
            val big: u64 = 18446744073709551615u64
            val one: u64 = 1u64
            val r = big.checked_add(one)
            match r {
                Option::Some(v) => v,
                Option::None => 7u64,
            }
        "#,
        7,
    );
}

#[test]
fn u64_checked_add_passes_through_in_range() {
    assert_checked_u64(
        r#"
            val a: u64 = 40u64
            val b: u64 = 2u64
            val r = a.checked_add(b)
            match r {
                Option::Some(v) => v,
                Option::None => 0u64,
            }
        "#,
        42,
    );
}

#[test]
fn u64_checked_sub_reports_underflow() {
    assert_checked_u64(
        r#"
            val a: u64 = 5u64
            val b: u64 = 10u64
            val r = a.checked_sub(b)
            match r {
                Option::Some(v) => v,
                Option::None => 7u64,
            }
        "#,
        7,
    );
}

#[test]
fn u64_checked_mul_reports_overflow_and_zero_is_not_overflow() {
    assert_checked_u64(
        r#"
            val big: u64 = 18446744073709551615u64
            val two: u64 = 2u64
            val zero: u64 = 0u64
            val over = big.checked_mul(two)
            val none = match over {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val by_zero = big.checked_mul(zero)
            val zeroed = match by_zero {
                Option::Some(v) => if v == 0u64 { 1u64 } else { 0u64 },
                Option::None => 0u64,
            }
            none + zeroed
        "#,
        2,
    );
}

#[test]
fn u64_checked_div_reports_zero_divisor() {
    assert_checked_u64(
        r#"
            val a: u64 = 10u64
            val zero: u64 = 0u64
            val two: u64 = 2u64
            val bad = a.checked_div(zero)
            val none = match bad {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val good = a.checked_div(two)
            val five = match good {
                Option::Some(v) => v,
                Option::None => 0u64,
            }
            none + five
        "#,
        6,
    );
}

#[test]
fn u64_saturating_clamps_at_both_ends() {
    assert_checked_u64(
        r#"
            val big: u64 = 18446744073709551615u64
            val five: u64 = 5u64
            val ten: u64 = 10u64
            val at_zero = five.saturating_sub(ten)
            val at_max = big.saturating_add(ten)
            val mul_max = big.saturating_mul(ten)
            if at_zero == 0u64 && at_max == big && mul_max == big {
                1u64
            } else {
                0u64
            }
        "#,
        1,
    );
}

#[test]
fn i64_checked_add_reports_both_bounds() {
    assert_checked_u64(
        r#"
            val hi: i64 = 9223372036854775807i64
            val lo: i64 = -9223372036854775808i64
            val one: i64 = 1i64
            val minus_one: i64 = -1i64
            val over = hi.checked_add(one)
            val a = match over {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val under = lo.checked_add(minus_one)
            val b = match under {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            a + b
        "#,
        2,
    );
}

#[test]
fn i64_checked_sub_reports_both_bounds() {
    assert_checked_u64(
        r#"
            val hi: i64 = 9223372036854775807i64
            val lo: i64 = -9223372036854775808i64
            val one: i64 = 1i64
            val minus_one: i64 = -1i64
            val over = hi.checked_sub(minus_one)
            val a = match over {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val under = lo.checked_sub(one)
            val b = match under {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            a + b
        "#,
        2,
    );
}

#[test]
fn i64_checked_mul_covers_min_times_minus_one() {
    // The `MIN * -1` case is the one product the division-based
    // check inside `checked_mul` cannot test for, since that test
    // would itself divide `MIN` by `-1` and trap.
    assert_checked_u64(
        r#"
            val lo: i64 = -9223372036854775808i64
            val minus_one: i64 = -1i64
            val six: i64 = 6i64
            val seven: i64 = 7i64
            val bad = lo.checked_mul(minus_one)
            val a = match bad {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val good = six.checked_mul(seven)
            val b = match good {
                Option::Some(v) => if v == 42i64 { 1u64 } else { 0u64 },
                Option::None => 0u64,
            }
            a + b
        "#,
        2,
    );
}

#[test]
fn i64_checked_div_covers_both_traps() {
    // Zero divisor and `MIN / -1` are exactly the two inputs on
    // which the `/` operator traps, so `checked_div` is the way to
    // divide by a value that might be either.
    assert_checked_u64(
        r#"
            val lo: i64 = -9223372036854775808i64
            val minus_one: i64 = -1i64
            val zero: i64 = 0i64
            val ten: i64 = 10i64
            val two: i64 = 2i64
            val overflow = lo.checked_div(minus_one)
            val a = match overflow {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val by_zero = ten.checked_div(zero)
            val b = match by_zero {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val good = ten.checked_div(two)
            val c = match good {
                Option::Some(v) => if v == 5i64 { 1u64 } else { 0u64 },
                Option::None => 0u64,
            }
            a + b + c
        "#,
        3,
    );
}

#[test]
fn i64_saturating_clamps_at_both_bounds() {
    assert_checked_u64(
        r#"
            val hi: i64 = 9223372036854775807i64
            val lo: i64 = -9223372036854775808i64
            val one: i64 = 1i64
            val at_max = hi.saturating_add(one)
            val at_min = lo.saturating_sub(one)
            if at_max == hi && at_min == lo {
                1u64
            } else {
                0u64
            }
        "#,
        1,
    );
}
