//! NUM-W-FOR-RANGE: a `for` range in a narrow integer type.
//!
//! `for i in -3i32..2i32 { .. }` type-checked and then did three
//! different things:
//!
//! - the IR VM ran it, five iterations;
//! - the tree-walker refused it — "For loop range must be UInt64 or
//!   Int64", a dispatch that knew only the two wide widths;
//! - the AOT and JIT lanes crashed cranelift's verifier —
//!   `arg 1 (v18) has type i64, expected i32` — because the loop's
//!   step constant was picked between `I64` and `U64` only, so a
//!   32-bit counter was being incremented by a 64-bit one.
//!
//! Writable but not runnable, and the failure was a crash rather than
//! a diagnostic. Both halves are fixed at their own layer: the step
//! constant is built in the induction variable's type, and the
//! tree-walker dispatches on the value pair so every width has an arm.

use super::harness::*;

/// The shape the item was filed for: a signed 32-bit range that
/// starts below zero, so the comparison has to be signed too.
#[test]
fn a_signed_narrow_range_counts_the_same_everywhere() {
    let src = r#"
        fn main() -> u64 {
            var n: u64 = 0u64
            var sum: i32 = 0i32
            for i in -3i32..2i32 {
                n = n + 1u64
                sum = sum + i
            }
            # -3 + -2 + -1 + 0 + 1 = -5, so 100 - 5 = 95
            n * 1000u64 + ((sum + 100i32) as u64)
        }
    "#;
    // Five iterations, and the sum is negative — a lane that compared
    // the counter as unsigned would not run the loop at all.
    assert_eq!(interpreter_value(src) % 1000, 95);
    assert_consistent(src, "narrow_for_range_i32");
}

/// Every width, so no arm is left out of the tree-walker's dispatch
/// or the step constant's type. The unsigned widths run right up to
/// their maximum, where a step in the wrong type would overflow.
#[test]
fn every_integer_width_can_drive_a_range() {
    let src = r#"
        fn main() -> u64 {
            var total: u64 = 0u64

            for a in 0u8..255u8 { total = total + 1u64 }
            for b in -128i8..127i8 { total = total + 1u64 }
            for c in 0u16..300u16 { total = total + 1u64 }
            for d in -300i16..300i16 { total = total + 1u64 }
            for e in 0u32..7u32 { total = total + 1u64 }
            for f in -2i32..2i32 { total = total + 1u64 }
            for g in 0u64..3u64 { total = total + 1u64 }
            for h in -1i64..1i64 { total = total + 1u64 }

            total
        }
    "#;
    // 255 + 255 + 300 + 600 + 7 + 4 + 3 + 2 = 1426
    assert_eq!(interpreter_value(src), 1426);
    assert_consistent(src, "narrow_for_range_all_widths");
}

/// `break` and `continue` still land on the step block, and the
/// narrow counter survives the round trip through it.
#[test]
fn break_and_continue_work_over_a_narrow_range() {
    let src = r#"
        fn main() -> u64 {
            var seen: u64 = 0u64
            for i in 0i16..100i16 {
                if i == 3i16 { continue }
                if i == 6i16 { break }
                seen = seen + 1u64
            }
            seen
        }
    "#;
    // 0, 1, 2, 4, 5 — three skipped or stopped on.
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "narrow_for_range_break_continue");
}

/// The `to` spelling of the same range, which lowers through the
/// identical header, plus a nested pair so the inner loop's counter
/// does not disturb the outer one.
#[test]
fn the_to_spelling_and_nesting_agree() {
    let src = r#"
        fn main() -> u64 {
            var total: u64 = 0u64
            for i in 0i8 to 4i8 {
                for j in 0u8..3u8 {
                    total = total + 1u64
                }
            }
            total
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 12);
    assert_consistent(src, "narrow_for_range_to_nested");
}
