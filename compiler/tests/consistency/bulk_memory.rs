//! MEMORY-ACCESS M0: the bulk-memory builtins on every lane.
//!
//! `__builtin_mem_copy` has been lowered by the compiled lanes since
//! Phase 3e, but its two siblings were never wired up: `mem_move` and
//! `mem_set` existed only in the tree-walker, so a program using them
//! died at compile time with "compiler MVP cannot lower builtin yet".
//! Nothing in `core/std/*.t` called them — which is the reason the gap
//! survived, and the reason the stdlib writes byte loops where a range
//! operation belongs (see design-docs/MEMORY_ACCESS.md).
//!
//! The other half of M0 is `mem_set`'s fill value. `docs/language.md`
//! and the AST comment both said `u8`; the type checker's table said
//! `u64` and never enforced it either way, so each lane truncated on
//! its own terms. It is a `u8` now, checked at the call.

use super::harness::*;

/// `mem_set` fills a range with one byte, and `mem_move` tolerates
/// overlap — the property that distinguishes it from `mem_copy`.
/// Shifting a filled range one byte to the right and reading the
/// whole thing back is the smallest program that needs both.
#[test]
fn mem_set_fills_and_mem_move_overlaps() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            __builtin_mem_set(p, 0x41u8, 8u64)
            # Overlapping ranges: bytes 0..8 slide to 1..9. A memcpy
            # here would be free to clobber its own source.
            __builtin_mem_move(p, __builtin_ptr_offset(p, 1u64), 8u64)
            __builtin_mem_set(p, 0x42u8, 1u64)
            var acc: u64 = 0u64
            var i: u64 = 0u64
            while i < 9u64 {
                val b: u8 = __builtin_ptr_read::<u8>(p, i)
                acc = acc + (b as u64)
                i = i + 1u64
            }
            __builtin_heap_free(p)
            # 0x42 + 8 * 0x41
            acc
        }
    "#;
    assert_eq!(interpreter_value(src), 586);
    assert_consistent(src, "mem_set_move");
}

/// The fill value is a byte, so it takes its type from the argument
/// position the way every other argument does: a suffix-less literal
/// becomes `u8`, and a character literal that fits does too
/// (CHAR-LITERAL-NUM).
#[test]
fn the_fill_byte_takes_the_argument_position_s_type() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            __builtin_mem_set(p, 0, 4u64)
            __builtin_mem_set(__builtin_ptr_offset(p, 4u64), '0', 4u64)
            val zero: u8 = __builtin_ptr_read::<u8>(p, 0u64)
            val digit: u8 = __builtin_ptr_read::<u8>(p, 4u64)
            __builtin_heap_free(p)
            (zero as u64) + (digit as u64)
        }
    "#;
    assert_eq!(interpreter_value(src), 48);
    assert_consistent(src, "mem_set_fill_literal");
}

/// A zero-length range is a no-op on every lane, not a fault — the
/// rule libc's `memcpy` / `memmove` / `memset` follow for `n == 0`.
#[test]
fn a_zero_length_range_is_a_no_op() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            __builtin_mem_set(p, 0x41u8, 8u64)
            __builtin_mem_set(p, 0x00u8, 0u64)
            __builtin_mem_move(p, __builtin_ptr_offset(p, 1u64), 0u64)
            __builtin_mem_copy(p, __builtin_ptr_offset(p, 1u64), 0u64)
            val untouched: u8 = __builtin_ptr_read::<u8>(p, 0u64)
            __builtin_heap_free(p)
            untouched as u64
        }
    "#;
    assert_eq!(interpreter_value(src), 65);
    assert_consistent(src, "mem_zero_length");
}

