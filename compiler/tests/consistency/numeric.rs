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
