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

// --- RUNTIME-TRAP-NARROW: the same seven methods at the other six
// widths. The two traits that named `u64` / `i64` concretely became
// one `trait Checked` over `Self` with an impl per width; the bodies
// are the same shapes with each width's own `MAX` / `MIN` literals.
//
// The 4-lane agreement and the full per-width bound table live in
// `compiler/tests/consistency/checked_narrow.rs`. These are the
// interpreter-side value tests, one per width family, and they are
// where a wrong *bound* (as opposed to a wrong shape) shows up with
// the width in the test name.

#[test]
fn u8_reports_and_clamps_at_255() {
    assert_checked_u64(
        r#"
            val mx: u8 = 255u8
            val one: u8 = 1u8
            val two: u8 = 2u8
            val over = mx.checked_add(one)
            val a = match over {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val fits = mx.checked_div(two)
            val b = match fits {
                Option::Some(v) => if v == 127u8 { 1u64 } else { 0u64 },
                Option::None => 0u64,
            }
            val c = if mx.saturating_add(one) == mx { 1u64 } else { 0u64 }
            val d = if mx.saturating_mul(two) == mx { 1u64 } else { 0u64 }
            a + b + c + d
        "#,
        4,
    );
}

#[test]
fn u16_and_u32_report_and_clamp_at_their_own_max() {
    assert_checked_u64(
        r#"
            val m16: u16 = 65535u16
            val one16: u16 = 1u16
            val m32: u32 = 4294967295u32
            val one32: u32 = 1u32
            val over16 = m16.checked_add(one16)
            val a = match over16 {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val over32 = m32.checked_add(one32)
            val b = match over32 {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            # A `u32` bound written one width too wide would let this
            # through, so the clamped values are checked as well.
            val c = if m16.saturating_add(one16) == m16 { 1u64 } else { 0u64 }
            val d = if m32.saturating_add(one32) == m32 { 1u64 } else { 0u64 }
            a + b + c + d
        "#,
        4,
    );
}

#[test]
fn u8_subtraction_traps_and_checked_sub_reports() {
    // NARROW-UNSIGNED-SUB: narrow unsigned `-` below zero traps, as
    // `u64` does (it used to wrap to 251). `checked_sub` asks instead.
    assert_checked_u64(
        r#"
            val a: u8 = 5u8
            val b: u8 = 10u8
            val under = a.checked_sub(b)
            val reported = match under {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val clamped = if a.saturating_sub(b) == 0u8 { 1u64 } else { 0u64 }
            reported + clamped
        "#,
        2,
    );
    crate::common::assert_program_fails(
        r#"
        fn main() -> u64 {
            val a: u8 = 5u8
            val b: u8 = 10u8
            (a - b) as u64
        }
        "#,
    );
}

#[test]
fn i8_reports_at_both_bounds_and_covers_both_division_traps() {
    assert_checked_u64(
        r#"
            val mx: i8 = 127i8
            val mn: i8 = -128i8
            val one: i8 = 1i8
            val minus_one: i8 = -1i8
            val zero: i8 = 0i8
            val hi = mx.checked_add(one)
            val a = match hi {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val lo = mn.checked_sub(one)
            val b = match lo {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            # Both traps `/` raises at this width: a zero divisor and
            # `MIN / -1`, whose quotient 128 is not an `i8`.
            val by_zero = mx.checked_div(zero)
            val c = match by_zero {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val overflowing = mn.checked_div(minus_one)
            val d = match overflowing {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            val e = match mn.checked_mul(minus_one) {
                Option::Some(v) => 0u64,
                Option::None => 1u64,
            }
            a + b + c + d + e
        "#,
        5,
    );
}

#[test]
fn i16_and_i32_clamp_to_their_own_bounds() {
    assert_checked_u64(
        r#"
            val mx16: i16 = 32767i16
            val mn16: i16 = -32768i16
            val seven16: i16 = 7i16
            val mx32: i32 = 2147483647i32
            val mn32: i32 = -2147483648i32
            val seven32: i32 = 7i32
            val a = if mx16.saturating_add(seven16) == mx16 { 1u64 } else { 0u64 }
            val b = if mn16.saturating_sub(seven16) == mn16 { 1u64 } else { 0u64 }
            val c = if mx32.saturating_add(seven32) == mx32 { 1u64 } else { 0u64 }
            val d = if mn32.saturating_sub(seven32) == mn32 { 1u64 } else { 0u64 }
            a + b + c + d
        "#,
        4,
    );
}

#[test]
fn signed_saturating_mul_picks_the_bound_the_product_ran_past() {
    // The one method with no predecessor: `CheckedI64` stopped at
    // `saturating_sub`. `MIN * -1` clamps *up* to `MAX` while
    // `MIN * 3` clamps down, so a body that named one bound for every
    // overflow would fail here rather than at a single edge.
    assert_checked_u64(
        r#"
            val mn: i64 = -9223372036854775808i64
            val mx: i64 = 9223372036854775807i64
            val minus_one: i64 = -1i64
            val three: i64 = 3i64
            val small: i64 = 100i64
            val a = if mn.saturating_mul(minus_one) == mx { 1u64 } else { 0u64 }
            val b = if mn.saturating_mul(three) == mn { 1u64 } else { 0u64 }
            val c = if mx.saturating_mul(three) == mx { 1u64 } else { 0u64 }
            val d = if small.saturating_mul(three) == 300i64 { 1u64 } else { 0u64 }
            val e = if small.saturating_mul(minus_one) == -100i64 { 1u64 } else { 0u64 }
            a + b + c + d + e
        "#,
        5,
    );
}
