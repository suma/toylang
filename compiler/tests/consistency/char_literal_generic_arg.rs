//! CHAR-LITERAL-GENERIC-ARG: a literal argument in a generic slot.
//!
//! CHAR-LITERAL-NUM lets a char literal take the integer type the
//! position asks for — `val b: u8 = '0'`, `byte == 'h'`. The rule
//! reads "a position that names an integer type", and a method
//! parameter declared `T` looked like it named nothing, so the hint
//! was dropped and the literal stayed the `u32` a char is held as.
//!
//! But a receiver with concrete type arguments has already decided
//! what `T` is. `Span<u8>::set(&mut self, i: u64, v: T)` on a
//! `Span<u8>` declares a `u8` parameter, and `s.set(0u64, 'A')`
//! reached cranelift as an i32 against an i8 slot:
//!
//! ```text
//! arg 3 (v48) has type i32, expected i8
//! ```
//!
//! A verifier crash, not a diagnostic — the type checker had approved
//! the program. Substituting the receiver's type arguments into the
//! declared parameter types fixes it at the source, and the same
//! substitution settles a suffix-less integer literal in that slot,
//! which had been defaulting to `u64` for want of anything to name it.

use super::harness::*;

/// The shape the item was filed for: a char literal written into a
/// `Span<u8>`, whose element type only the receiver knows.
#[test]
fn a_char_literal_takes_the_receivers_element_type() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u8> = Vec::new()
            v.push(0u8)
            v.push(0u8)
            val window: Option<Span<u8>> = v.as_span()
            match window {
                Option::Some(s) => {
                    s.set(0u64, 'A')
                    s.set(1u64, 'B')
                    val a = s.get(0u64)
                    val b = s.get(1u64)
                    (a as u64) + (b as u64)
                }
                Option::None => 0u64,
            }
        }
    "#;
    // 'A' + 'B' — 65 + 66. A build that kept the literals 32 bits wide
    // never gets this far: cranelift refuses the call.
    assert_eq!(interpreter_value(src) & 0xff, 131);
    assert_consistent(src, "char_literal_span_set");
}

/// The same slot on `Vec<T>` itself, for both literal kinds — a char
/// literal and a suffix-less integer. Neither names a width; the
/// receiver does.
#[test]
fn a_literal_pushed_into_a_vec_takes_the_element_type() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u8> = Vec::new()
            v.push(65)
            v.push('B')
            val a = v.get(0u64)
            val b = v.get(1u64)
            (a as u64) + (b as u64)
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 131);
    assert_consistent(src, "char_literal_vec_push");
}

/// A parameter the *method* introduced is a different matter: nothing
/// about the receiver names `U`, so the literal decides it, as it
/// always has. This is here so a future widening of the substitution
/// does not quietly start claiming those slots too.
///
/// Interpreter-only on purpose: a method-level generic
/// (`fn pick<U>`) is not something the compiled lanes instantiate yet
/// (todo.md, JIT-INTERP-COVERAGE (a) and its AOT sibling), so
/// `assert_consistent` would be pinning that gap rather than this
/// rule.
#[test]
fn a_method_level_generic_is_still_decided_by_the_argument() {
    let src = r#"
        struct Holder<T> { v: T }

        impl<T> Holder<T> {
            fn pick<U>(&self, other: U) -> U { other }
        }

        fn main() -> u64 {
            val h: Holder<u8> = Holder { v: 1u8 }
            val n = h.pick(7u64)
            n
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
}
