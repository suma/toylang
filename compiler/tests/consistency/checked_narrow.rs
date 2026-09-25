//! `core/std/checked.t` at every integer width (RUNTIME-TRAP-NARROW).
//!
//! The module used to be two traits naming `u64` and `i64`
//! concretely. That was not a choice about which widths deserved an
//! escape hatch — `Option<Self>` as a method return type did not
//! survive lowering, so the payload had to be a concrete type, and a
//! narrow receiver was missing from the lowering's dispatch table
//! anyway. Both were fixed on 2026-08-31, and the widths below
//! followed with no compiler change behind them.
//!
//! What these tests are for:
//!
//! - **A width silently dropping out.** This is the
//!   NUM-W-ENUMERATION failure mode (see `primitive_receivers.rs`):
//!   an omitted width is not a wrong answer, it is an unreachable
//!   method. `every_width_answers_at_its_own_bounds` calls all seven
//!   methods at all eight widths, so an omission is a compile error
//!   and a wrong bound is a named bit.
//! - **Narrow `-` is not `u64` `-`.** Subtraction below zero *wraps*
//!   on `u8` / `u16` / `u32` where `u64` traps, so at narrow widths
//!   `checked_sub` reports an underflow the program would otherwise
//!   have run past holding a plausible number.
//! - **The one genuinely new body.** `saturating_mul` for a signed
//!   type had no predecessor to copy: it has to pick a bound from the
//!   sign of the product it could not represent.
//!
//! The expected numbers all come from Rust's own `checked_*` /
//! `saturating_*` on the same inputs.

use super::harness::*;

/// All seven methods, at all eight widths, at each width's own
/// bounds. The program returns a **bit per width** (bit 0 = `u8` …
/// bit 7 = `i64`) set when any of that width's eight answers is
/// wrong, so a failure names the width instead of just disagreeing.
///
/// `?? seven` folds the `Option` away: 7 is not the wrapped answer to
/// any of the four overflow cases, so it cannot be mistaken for a
/// `Some` that should have been `None`.
#[test]
fn every_width_answers_at_its_own_bounds() {
    let src = r#"        fn main() -> u64 {
            var mask: u64 = 0u64

            # u8 -> bit 0
            val u8_mn: u8 = 0u8
            val u8_mx: u8 = 255u8
            val u8_one: u8 = 1u8
            val u8_two: u8 = 2u8
            val u8_three: u8 = 3u8
            val u8_seven: u8 = 7u8
            val u8_zero: u8 = 0u8
            val u8_c1: u8 = u8_mx.checked_add(u8_one) ?? u8_seven
            val u8_c2: u8 = u8_mn.checked_sub(u8_one) ?? u8_seven
            val u8_c3: u8 = u8_mx.checked_mul(u8_two) ?? u8_seven
            val u8_c4: u8 = u8_mx.checked_div(u8_zero) ?? u8_seven
            val u8_c5: u8 = u8_mx.checked_div(u8_two) ?? u8_seven
            val u8_c6: u8 = u8_mx.saturating_add(u8_seven)
            val u8_c7: u8 = u8_mn.saturating_sub(u8_seven)
            val u8_c8: u8 = u8_mx.saturating_mul(u8_three)
            val u8_ok: bool = u8_c1 == u8_seven && u8_c2 == u8_seven
                && u8_c3 == u8_seven && u8_c4 == u8_seven
                && u8_c5 == 127u8 && u8_c6 == u8_mx
                && u8_c7 == u8_mn && u8_c8 == u8_mx
            if !u8_ok { mask = mask | 1u64 }

            # u16 -> bit 1
            val u16_mn: u16 = 0u16
            val u16_mx: u16 = 65535u16
            val u16_one: u16 = 1u16
            val u16_two: u16 = 2u16
            val u16_three: u16 = 3u16
            val u16_seven: u16 = 7u16
            val u16_zero: u16 = 0u16
            val u16_c1: u16 = u16_mx.checked_add(u16_one) ?? u16_seven
            val u16_c2: u16 = u16_mn.checked_sub(u16_one) ?? u16_seven
            val u16_c3: u16 = u16_mx.checked_mul(u16_two) ?? u16_seven
            val u16_c4: u16 = u16_mx.checked_div(u16_zero) ?? u16_seven
            val u16_c5: u16 = u16_mx.checked_div(u16_two) ?? u16_seven
            val u16_c6: u16 = u16_mx.saturating_add(u16_seven)
            val u16_c7: u16 = u16_mn.saturating_sub(u16_seven)
            val u16_c8: u16 = u16_mx.saturating_mul(u16_three)
            val u16_ok: bool = u16_c1 == u16_seven && u16_c2 == u16_seven
                && u16_c3 == u16_seven && u16_c4 == u16_seven
                && u16_c5 == 32767u16 && u16_c6 == u16_mx
                && u16_c7 == u16_mn && u16_c8 == u16_mx
            if !u16_ok { mask = mask | 2u64 }

            # u32 -> bit 2
            val u32_mn: u32 = 0u32
            val u32_mx: u32 = 4294967295u32
            val u32_one: u32 = 1u32
            val u32_two: u32 = 2u32
            val u32_three: u32 = 3u32
            val u32_seven: u32 = 7u32
            val u32_zero: u32 = 0u32
            val u32_c1: u32 = u32_mx.checked_add(u32_one) ?? u32_seven
            val u32_c2: u32 = u32_mn.checked_sub(u32_one) ?? u32_seven
            val u32_c3: u32 = u32_mx.checked_mul(u32_two) ?? u32_seven
            val u32_c4: u32 = u32_mx.checked_div(u32_zero) ?? u32_seven
            val u32_c5: u32 = u32_mx.checked_div(u32_two) ?? u32_seven
            val u32_c6: u32 = u32_mx.saturating_add(u32_seven)
            val u32_c7: u32 = u32_mn.saturating_sub(u32_seven)
            val u32_c8: u32 = u32_mx.saturating_mul(u32_three)
            val u32_ok: bool = u32_c1 == u32_seven && u32_c2 == u32_seven
                && u32_c3 == u32_seven && u32_c4 == u32_seven
                && u32_c5 == 2147483647u32 && u32_c6 == u32_mx
                && u32_c7 == u32_mn && u32_c8 == u32_mx
            if !u32_ok { mask = mask | 4u64 }

            # u64 -> bit 3
            val u64_mn: u64 = 0u64
            val u64_mx: u64 = 18446744073709551615u64
            val u64_one: u64 = 1u64
            val u64_two: u64 = 2u64
            val u64_three: u64 = 3u64
            val u64_seven: u64 = 7u64
            val u64_zero: u64 = 0u64
            val u64_c1: u64 = u64_mx.checked_add(u64_one) ?? u64_seven
            val u64_c2: u64 = u64_mn.checked_sub(u64_one) ?? u64_seven
            val u64_c3: u64 = u64_mx.checked_mul(u64_two) ?? u64_seven
            val u64_c4: u64 = u64_mx.checked_div(u64_zero) ?? u64_seven
            val u64_c5: u64 = u64_mx.checked_div(u64_two) ?? u64_seven
            val u64_c6: u64 = u64_mx.saturating_add(u64_seven)
            val u64_c7: u64 = u64_mn.saturating_sub(u64_seven)
            val u64_c8: u64 = u64_mx.saturating_mul(u64_three)
            val u64_ok: bool = u64_c1 == u64_seven && u64_c2 == u64_seven
                && u64_c3 == u64_seven && u64_c4 == u64_seven
                && u64_c5 == 9223372036854775807u64 && u64_c6 == u64_mx
                && u64_c7 == u64_mn && u64_c8 == u64_mx
            if !u64_ok { mask = mask | 8u64 }

            # i8 -> bit 4
            val i8_mn: i8 = -128i8
            val i8_mx: i8 = 127i8
            val i8_one: i8 = 1i8
            val i8_two: i8 = 2i8
            val i8_three: i8 = 3i8
            val i8_seven: i8 = 7i8
            val i8_zero: i8 = 0i8
            val i8_c1: i8 = i8_mx.checked_add(i8_one) ?? i8_seven
            val i8_c2: i8 = i8_mn.checked_sub(i8_one) ?? i8_seven
            val i8_c3: i8 = i8_mx.checked_mul(i8_two) ?? i8_seven
            val i8_c4: i8 = i8_mx.checked_div(i8_zero) ?? i8_seven
            val i8_c5: i8 = i8_mx.checked_div(i8_two) ?? i8_seven
            val i8_c6: i8 = i8_mx.saturating_add(i8_seven)
            val i8_c7: i8 = i8_mn.saturating_sub(i8_seven)
            val i8_c8: i8 = i8_mx.saturating_mul(i8_three)
            val i8_ok: bool = i8_c1 == i8_seven && i8_c2 == i8_seven
                && i8_c3 == i8_seven && i8_c4 == i8_seven
                && i8_c5 == 63i8 && i8_c6 == i8_mx
                && i8_c7 == i8_mn && i8_c8 == i8_mx
            if !i8_ok { mask = mask | 16u64 }

            # i16 -> bit 5
            val i16_mn: i16 = -32768i16
            val i16_mx: i16 = 32767i16
            val i16_one: i16 = 1i16
            val i16_two: i16 = 2i16
            val i16_three: i16 = 3i16
            val i16_seven: i16 = 7i16
            val i16_zero: i16 = 0i16
            val i16_c1: i16 = i16_mx.checked_add(i16_one) ?? i16_seven
            val i16_c2: i16 = i16_mn.checked_sub(i16_one) ?? i16_seven
            val i16_c3: i16 = i16_mx.checked_mul(i16_two) ?? i16_seven
            val i16_c4: i16 = i16_mx.checked_div(i16_zero) ?? i16_seven
            val i16_c5: i16 = i16_mx.checked_div(i16_two) ?? i16_seven
            val i16_c6: i16 = i16_mx.saturating_add(i16_seven)
            val i16_c7: i16 = i16_mn.saturating_sub(i16_seven)
            val i16_c8: i16 = i16_mx.saturating_mul(i16_three)
            val i16_ok: bool = i16_c1 == i16_seven && i16_c2 == i16_seven
                && i16_c3 == i16_seven && i16_c4 == i16_seven
                && i16_c5 == 16383i16 && i16_c6 == i16_mx
                && i16_c7 == i16_mn && i16_c8 == i16_mx
            if !i16_ok { mask = mask | 32u64 }

            # i32 -> bit 6
            val i32_mn: i32 = -2147483648i32
            val i32_mx: i32 = 2147483647i32
            val i32_one: i32 = 1i32
            val i32_two: i32 = 2i32
            val i32_three: i32 = 3i32
            val i32_seven: i32 = 7i32
            val i32_zero: i32 = 0i32
            val i32_c1: i32 = i32_mx.checked_add(i32_one) ?? i32_seven
            val i32_c2: i32 = i32_mn.checked_sub(i32_one) ?? i32_seven
            val i32_c3: i32 = i32_mx.checked_mul(i32_two) ?? i32_seven
            val i32_c4: i32 = i32_mx.checked_div(i32_zero) ?? i32_seven
            val i32_c5: i32 = i32_mx.checked_div(i32_two) ?? i32_seven
            val i32_c6: i32 = i32_mx.saturating_add(i32_seven)
            val i32_c7: i32 = i32_mn.saturating_sub(i32_seven)
            val i32_c8: i32 = i32_mx.saturating_mul(i32_three)
            val i32_ok: bool = i32_c1 == i32_seven && i32_c2 == i32_seven
                && i32_c3 == i32_seven && i32_c4 == i32_seven
                && i32_c5 == 1073741823i32 && i32_c6 == i32_mx
                && i32_c7 == i32_mn && i32_c8 == i32_mx
            if !i32_ok { mask = mask | 64u64 }

            # i64 -> bit 7
            val i64_mn: i64 = -9223372036854775808i64
            val i64_mx: i64 = 9223372036854775807i64
            val i64_one: i64 = 1i64
            val i64_two: i64 = 2i64
            val i64_three: i64 = 3i64
            val i64_seven: i64 = 7i64
            val i64_zero: i64 = 0i64
            val i64_c1: i64 = i64_mx.checked_add(i64_one) ?? i64_seven
            val i64_c2: i64 = i64_mn.checked_sub(i64_one) ?? i64_seven
            val i64_c3: i64 = i64_mx.checked_mul(i64_two) ?? i64_seven
            val i64_c4: i64 = i64_mx.checked_div(i64_zero) ?? i64_seven
            val i64_c5: i64 = i64_mx.checked_div(i64_two) ?? i64_seven
            val i64_c6: i64 = i64_mx.saturating_add(i64_seven)
            val i64_c7: i64 = i64_mn.saturating_sub(i64_seven)
            val i64_c8: i64 = i64_mx.saturating_mul(i64_three)
            val i64_ok: bool = i64_c1 == i64_seven && i64_c2 == i64_seven
                && i64_c3 == i64_seven && i64_c4 == i64_seven
                && i64_c5 == 4611686018427387903i64 && i64_c6 == i64_mx
                && i64_c7 == i64_mn && i64_c8 == i64_mx
            if !i64_ok { mask = mask | 128u64 }

            mask
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 0);
    assert_consistent(src, "checked_every_width");
}

/// NARROW-UNSIGNED-SUB: `u8` subtraction below zero traps, as `u64`
/// always has (it used to wrap to 251 on every lane). `checked_sub` /
/// `saturating_sub` are the ways to ask instead of stopping.
#[test]
fn narrow_unsigned_subtraction_traps_like_u64() {
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 5u8
            val b: u8 = 10u8
            val d = a.checked_sub(b)
            match d {
                Option::Some(v) => println(v),
                Option::None => println("underflows"),
            }
            println(a.saturating_sub(b))
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "checked_narrow_report", true),
        "underflows\n0\n"
    );
    assert_stdout_consistent(src, "checked_narrow_report");
    for (ty, a, b) in [("u8", "5u8", "10u8"), ("u16", "1u16", "2u16"), ("u32", "0u32", "7u32")] {
        let trap = format!(
            r#"
            fn sub(x: {ty}, y: {ty}) -> {ty} {{ x - y }}
            fn main() -> u64 {{
                sub({a}, {b}) as u64
            }}
        "#
        );
        assert_diagnostic_consistent(&trap, &format!("narrow_sub_trap_{ty}"));
    }
}

/// Signed `saturating_mul` is the one method the split traits never
/// had (`CheckedI64` stopped at `saturating_sub`). Past the zero and
/// `MIN * -1` arms it multiplies, divides back to detect the
/// overflow, and then clamps to the bound the *true* product ran
/// past — so a negative product lands on `MIN` and a positive one on
/// `MAX`, rather than both landing on whichever the last branch
/// happened to name.
#[test]
fn signed_saturating_mul_clamps_toward_the_products_own_sign() {
    let src = r#"
        fn main() -> u64 {
            val mn: i16 = -32768i16
            val mx: i16 = 32767i16
            val neg: i16 = -1i16
            val three: i16 = 3i16
            val small: i16 = 100i16
            println(mn.saturating_mul(neg))
            println(mn.saturating_mul(three))
            println(mx.saturating_mul(three))
            println(mx.saturating_mul(neg))
            println(small.saturating_mul(three))
            println(small.saturating_mul(neg))
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "checked_sat_mul_sign", true),
        "32767\n-32768\n32767\n-32767\n300\n-100\n"
    );
    assert_stdout_consistent(src, "checked_sat_mul_sign");
}
