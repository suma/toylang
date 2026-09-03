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

// N5: `f32` had a type, literals and operators, and not one libm
// function -- nor a format spec.

#[test]
fn f32_has_the_single_precision_family() {
    let src = r#"
        fn main() -> u64 {
            val two: f32 = 2f32
            println(math::sqrt_f32(two))
            println(math::floor_f32(2.7f32))
            println(math::ceil_f32(2.1f32))
            println(math::round_f32(2.5f32))
            println(math::fabs_f32(-3f32))
            val n = limits::f32_nan()
            println(math::min_f32(n, 1f32))
            println(math::is_nan_f32(n))
            println(math::is_infinite_f32(limits::f32_inf()))
            println(math::is_finite_f32(limits::f32_inf()))
            println(math::is_finite_f32(1f32))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "f32_family");
}

#[test]
fn a_format_spec_applies_to_f32_too() {
    // At single precision, not promoted: `0.1f32` renders as `0.1`
    // here and as `0.10000000149011612` through f64.
    let src = r#"
        fn main() -> u64 {
            val x: f32 = 3.14159f32
            println("{x:.2}")
            println("{x:.4}")
            val y: f32 = 1f32
            println("{y}")
            val z: f32 = 0.1f32
            println("{z}")
            val w: f32 = 2.5f32
            println("{w:<10}|")
            println("{w:>10}|")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "f32_format_spec");
}

// N6: random numbers, all of them on one extern so the same seed
// gives the same sequence everywhere.

#[test]
fn a_seeded_sequence_is_the_same_on_every_backend() {
    let src = r#"
        fn main() -> u64 {
            io::random_seed(0x99u64)
            var i: u64 = 0u64
            while i < 8u64 {
                println(random::random_range(10u64, 20u64))
                i = i + 1u64
            }
            println(random::random_bool())
            println(random::random_i64_range(-5i64, 5i64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "random_seeded");
}

#[test]
fn the_random_shapes_stay_inside_their_ranges() {
    // The values themselves are the previous test's business; this is
    // about the invariants, which hold for any seed.
    let src = r#"
        fn main() -> u64 {
            io::random_seed(7u64)
            var bad: u64 = 0u64
            var i: u64 = 0u64
            while i < 200u64 {
                # A power-of-two span takes the mask path, anything
                # else the rejection path -- both are exercised.
                val a = random::random_range(0u64, 16u64)
                if a >= 16u64 { bad = bad + 1u64 }
                val b = random::random_range(3u64, 10u64)
                if b < 3u64 || b >= 10u64 { bad = bad + 1u64 }
                val c = random::random_i64_range(-4i64, 4i64)
                if c < -4i64 || c >= 4i64 { bad = bad + 1u64 }
                val f = random::random_f64()
                if f < 0f64 || f >= 1f64 { bad = bad + 1u64 }
                val n = random::random_normal()
                if math::is_finite(n) == false { bad = bad + 1u64 }
                i = i + 1u64
            }
            bad
        }
    "#;
    // Zero violations across a thousand draws.
    assert_consistent(src, "random_ranges");
}

#[test]
fn a_shuffle_is_a_permutation_and_the_same_one_everywhere() {
    let src = r#"
        fn main() -> u64 {
            io::random_seed(0x99u64)
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < 12u64 {
                v.push(i)
                i = i + 1u64
            }
            random::shuffle(&mut v)
            # The order is the interesting part -- it has to be the
            # same on every backend, since one seed feeds one PRNG.
            var k: u64 = 0u64
            while k < v.size() {
                print(v.get(k))
                print(" ")
                k = k + 1u64
            }
            println("")
            # ... and it has to still be the same twelve values. A
            # swap that dropped one would keep the length.
            var seen: u64 = 0u64
            var j: u64 = 0u64
            while j < v.size() {
                seen = seen | (1u64 << v.get(j))
                j = j + 1u64
            }
            println(seen)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "random_shuffle");
}

#[test]
fn shuffling_the_short_cases_does_not_trap() {
    // Zero and one element: `n - 1` would underflow, and `u64`
    // subtraction traps rather than wrapping.
    let src = r#"
        fn main() -> u64 {
            io::random_seed(1u64)
            var empty: Vec<u64> = Vec::new()
            random::shuffle(&mut empty)
            var one: Vec<u64> = Vec::new()
            one.push(42u64)
            random::shuffle(&mut one)
            empty.size() + one.get(0u64)
        }
    "#;
    assert_consistent(src, "random_shuffle_short");
}

// N2 (the rest): `checked_pow`, the one `Checked` member whose
// exponent is not another `Self`.

#[test]
fn checked_pow_reports_the_width_it_leaves() {
    let src = r#"
        fn show8(r: Option<u8>) -> u64 {
            match r {
                Option::Some(v) => { println(v) }
                Option::None => { println("none") }
            }
            0u64
        }

        fn show_i64(r: Option<i64>) -> u64 {
            match r {
                Option::Some(v) => { println(v) }
                Option::None => { println("none") }
            }
            0u64
        }

        fn main() -> u64 {
            # The receiver is a name, the shape the compiled backends
            # want (`docs/language.md` -> "Overflow-aware arithmetic").
            val three: u8 = 3u8
            val a: Option<u8> = three.checked_pow(5u32)
            show8(a)
            val b: Option<u8> = three.checked_pow(6u32)
            show8(b)
            # Anything to the zeroth is one, including zero.
            val zero: u8 = 0u8
            val c: Option<u8> = zero.checked_pow(0u32)
            show8(c)
            val neg: i64 = -3i64
            val d: Option<i64> = neg.checked_pow(3u32)
            show_i64(d)
            val two: i64 = 2i64
            val e: Option<i64> = two.checked_pow(62u32)
            show_i64(e)
            val f: Option<i64> = two.checked_pow(63u32)
            show_i64(f)
            # A huge exponent on a base that cannot overflow: the
            # square-and-multiply loop runs 32 times, not four
            # billion.
            val one: i64 = 1i64
            val g: Option<i64> = one.checked_pow(4000000000u32)
            show_i64(g)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "checked_pow");
}

#[test]
fn clamp_f32_holds_the_bounds() {
    let src = r#"
        fn main() -> u64 {
            println(math::clamp_f32(5.5f32, 0f32, 2f32))
            println(math::clamp_f32(-1f32, 0f32, 2f32))
            println(math::clamp_f32(1.5f32, 0f32, 2f32))
            # NaN is not outside the range -- it is not comparable to
            # it, so both tests fail and the value comes back out.
            println(math::is_nan_f32(math::clamp_f32(limits::f32_nan(), 0f32, 2f32)))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "clamp_f32");
}
