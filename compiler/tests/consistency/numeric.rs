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
