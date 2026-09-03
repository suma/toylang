//! STDLIB-NUMERIC: the integer side of the numeric library.
//!
//! `math.t` was eleven libm wrappers over `f64` plus `abs` / `min` /
//! `max`, and the integer side was empty. These pin what was added.

use super::harness::*;

// N0: the extreme values, which everything else is written against.
//
// They are functions rather than constants because a module's
// top-level `const` is not visible from another module -- so a program
// had no way to name `u64::MAX` at all, and `checked.t` spelled
// sixteen limits inline across eight widths.

#[test]
fn every_width_can_name_its_extremes() {
    let src = r#"
        fn main() -> u64 {
            println(limits::u8_min())
            println(limits::u8_max())
            println(limits::u16_max())
            println(limits::u32_max())
            println(limits::u64_max())
            println(limits::i8_min())
            println(limits::i8_max())
            println(limits::i16_min())
            println(limits::i16_max())
            println(limits::i32_min())
            println(limits::i32_max())
            println(limits::i64_min())
            println(limits::i64_max())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "limits_integer");
}

#[test]
fn the_float_values_that_have_no_literal() {
    // An infinity and a NaN cannot be written down. `1f64 / 0f64` and
    // `0f64 / 0f64` produce them, but a reader has to work out that
    // that is what was meant -- and for integers the same spelling is
    // a trap, so it reads like a bug.
    let src = r#"
        fn main() -> u64 {
            println(limits::f64_inf())
            println(limits::f64_neg_inf())
            println(limits::f64_nan())
            # Every NaN compares false against everything, itself
            # included -- which is the test for one.
            val n = limits::f64_nan()
            println(n == n)
            println(n != n)
            # The infinities are ordered, and absorb.
            val i = limits::f64_inf()
            println(i > limits::f64_max())
            println(limits::f64_neg_inf() < limits::f64_max())
            println(limits::f64_epsilon() > 0f64)
            println(limits::f64_min_positive() > 0f64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "limits_float");
}

#[test]
fn the_checked_arithmetic_still_finds_the_same_edges() {
    // `checked.t` is the first consumer: its inline literals are gone,
    // so this is the regression test for the rewrite as much as for
    // the limits.
    let src = r#"
        fn show(o: Option<u8>) -> u64 {
            match o {
                Option::Some(v) => v as u64,
                Option::None => 999u64,
            }
        }

        fn main() -> u64 {
            val a: u8 = 250u8
            val hit = a.checked_add(10u8)
            val fits = a.checked_add(5u8)
            println(show(hit))
            println(show(fits))
            println(a.saturating_add(10u8))
            val b: i8 = -120i8
            println(b.saturating_sub(20i8))
            val c: u64 = limits::u64_max()
            val over = c.checked_mul(2u64)
            match over {
                Option::Some(_) => { println("unexpectedly fits") }
                Option::None => { println("overflow") }
            }
            # The saturating edge is the limit itself.
            println(c.saturating_add(1u64) == limits::u64_max())
            val d: i64 = limits::i64_min()
            println(d.saturating_sub(1i64) == limits::i64_min())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "checked_uses_limits");
}

// N1: bit operations. Five externs over `u64` with the per-width
// correction in toylang -- nine operations across eight widths would
// otherwise be 72 boundary crossings.

#[test]
fn bit_operations_answer_per_width() {
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 0xB0u8
            println(a.popcount())
            println(a.leading_zeros())
            println(a.trailing_zeros())
            println(a.reverse_bits())
            val w: u32 = 0x12345678u32
            println(w.swap_bytes())
            println(w.rotate_left(8u32))
            println(w.rotate_right(8u32))
            # A rotation by the width is the identity, and by more
            # than the width wraps round.
            println(w.rotate_left(32u32) == w)
            println(w.rotate_left(40u32) == w.rotate_left(8u32))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "bits_per_width");
}

#[test]
fn zero_is_defined_and_the_edges_hold() {
    // The hardware instruction leaves an input of 0 undefined; this
    // answers the width, as Rust does. The extremes are the other
    // place a width correction goes wrong.
    let src = r#"
        fn main() -> u64 {
            val z8: u8 = 0u8
            val z64: u64 = 0u64
            println(z8.leading_zeros())
            println(z8.trailing_zeros())
            println(z64.leading_zeros())
            println(z64.trailing_zeros())
            println(z8.popcount())
            val m8: u8 = limits::u8_max()
            val m64: u64 = limits::u64_max()
            println(m8.popcount())
            println(m8.leading_zeros())
            println(m8.trailing_zeros())
            println(m64.popcount())
            println(m64.leading_zeros())
            println(m64.trailing_zeros())
            # A signed width answers about the bit pattern, not the
            # value.
            val neg: i8 = -1i8
            println(neg.popcount())
            val lo: i64 = limits::i64_min()
            println(lo.popcount())
            println(lo.leading_zeros())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "bits_zero_and_edges");
}

#[test]
fn powers_of_two_round_up_and_zero_is_not_one() {
    let src = r#"
        fn main() -> u64 {
            val z: u64 = 0u64
            println(z.is_power_of_two())
            println(z.next_power_of_two())
            val one: u64 = 1u64
            println(one.is_power_of_two())
            println(one.next_power_of_two())
            val n: u64 = 100u64
            println(n.next_power_of_two())
            val exact: u64 = 128u64
            println(exact.is_power_of_two())
            println(exact.next_power_of_two())
            val small: u8 = 100u8
            println(small.next_power_of_two())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "bits_power_of_two");
}

// N2: integer math. Overflow wraps, like `+` and `*` -- `Checked` is
// where a caller goes to be told about it.

#[test]
fn integer_math_answers_the_arithmetic_the_operators_do_not() {
    let src = r#"
        fn main() -> u64 {
            println(math::pow_u64(3u64, 5u32))
            println(math::pow_u64(7u64, 0u32))
            println(math::pow_i64(-2i64, 3u32))
            println(math::gcd_u64(48u64, 18u64))
            println(math::gcd_u64(0u64, 0u64))
            println(math::gcd_u64(9u64, 0u64))
            println(math::lcm_u64(4u64, 6u64))
            println(math::lcm_u64(0u64, 5u64))
            println(math::sign_i64(-5i64))
            println(math::sign_i64(0i64))
            # The average of the two largest values, which `(a+b)/2`
            # cannot compute.
            println(math::midpoint_u64(limits::u64_max(), limits::u64_max()))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "integer_math");
}

#[test]
fn floor_division_is_named_apart_from_the_operators() {
    // `/` and `%` truncate toward zero, so `-7 / 3` is -2. Indexing a
    // ring buffer wants the other convention; the operators are not
    // changing, so the names are how the two are told apart.
    let src = r#"
        fn main() -> u64 {
            println(-7i64 / 3i64)
            println(math::div_floor_i64(-7i64, 3i64))
            println(-7i64 % 3i64)
            println(math::mod_floor_i64(-7i64, 3i64))
            # Positive operands agree with the operators.
            println(math::div_floor_i64(7i64, 3i64))
            println(math::mod_floor_i64(7i64, 3i64))
            # The remainder always takes the divisor's sign.
            println(math::mod_floor_i64(7i64, -3i64))
            println(math::mod_floor_i64(-6i64, 3i64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "floor_division");
}

#[test]
fn integer_square_root_is_exact_where_floats_are_not() {
    // `sqrt(x as f64) as u64` is off by one above 2^53, where an f64
    // can no longer hold every integer. The postcondition is what
    // `--check` uses as an oracle -- and what found the overflow in
    // the first draft, where the initial guess of `x` itself made
    // `r + x / r` wrap for `u64::MAX` and the next step divide by 0.
    let src = r#"
        fn main() -> u64 {
            println(math::isqrt_u64(0u64))
            println(math::isqrt_u64(1u64))
            println(math::isqrt_u64(2u64))
            println(math::isqrt_u64(15u64))
            println(math::isqrt_u64(16u64))
            println(math::isqrt_u64(1000000000000u64))
            # Past 2^53, where the float route goes wrong.
            println(math::isqrt_u64(9007199254740993u64))
            println(math::isqrt_u64(limits::u64_max()))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "isqrt");
}

// N3 / N4.

#[test]
fn min_max_and_clamp_span_the_widths_and_put_nan_last() {
    let src = r#"
        fn main() -> u64 {
            println(math::min_u8(3u8, 9u8))
            println(math::max_u8(3u8, 9u8))
            println(math::clamp_i32(50i32, 0i32, 10i32))
            println(math::clamp_i32(-50i32, 0i32, 10i32))
            println(math::clamp_i32(5i32, 0i32, 10i32))
            println(math::min_i16(limits::i16_min(), 0i16))
            println(math::max_u64(limits::u64_max(), 0u64))
            # IEEE 754's `minNum`: a NaN loses to a number, either way
            # round. Written out because `<` alone propagates it.
            val n = limits::f64_nan()
            println(math::min_f64(n, 1f64))
            println(math::min_f64(1f64, n))
            println(math::max_f64(n, 1f64))
            println(math::min_f64(2f64, 1f64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "min_max_clamp");
}

#[test]
fn the_rest_of_f64() {
    let src = r#"
        fn main() -> u64 {
            # Halves round away from zero, not to even.
            println(math::round(2.5f64))
            println(math::round(-2.5f64))
            println(math::round(2.4f64))
            println(math::trunc(-2.7f64))
            println(math::log10(1000f64))
            println(math::hypot(3f64, 4f64))
            println(math::atan2(0f64, 1f64))
            println(math::asin(0f64))
            println(math::acos(1f64))
            # Classification: a NaN is the only value unequal to
            # itself, which is both the definition and the test.
            val n = limits::f64_nan()
            val i = limits::f64_inf()
            println(math::is_nan(n))
            println(math::is_nan(1f64))
            println(math::is_infinite(i))
            println(math::is_infinite(n))
            println(math::is_finite(1f64))
            println(math::is_finite(i))
            println(math::is_finite(n))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "f64_rest");
}
