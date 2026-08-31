//! Extension-trait methods on primitive receivers, at every width.
//!
//! `impl <Trait> for u64` has worked since the extension-trait
//! dispatch landed; `impl <Trait> for u8` parsed, type-checked, lowered
//! — and was then unreachable. The receiver-type-to-target-name mapping
//! existed in **four** copies (the type checker's two directions, the
//! lowering's registration side, and its dispatch side), and each copy
//! that omitted a width disabled that width silently. The narrow ints
//! were missing from the dispatch side and `f32` from all four, so a
//! call fell past the primitive path into the struct/enum binding path
//! and failed with "the method receiver must be a struct or enum
//! binding" — a message that names neither the width nor the impl.
//!
//! That is the enumeration `todo.md`'s NUM-W-ENUMERATION describes, and
//! the second bug it has produced. These tests pin every width so a
//! future copy cannot omit one quietly.

use super::harness::*;

/// All six narrow integer widths dispatch a trait method.
#[test]
fn narrow_integer_receivers_dispatch_extension_methods() {
    let src = r#"
        trait Twice { fn twice(self: Self) -> Self }
        impl Twice for u8  { fn twice(self: Self) -> Self { self * 2u8 } }
        impl Twice for u16 { fn twice(self: Self) -> Self { self * 2u16 } }
        impl Twice for u32 { fn twice(self: Self) -> Self { self * 2u32 } }
        impl Twice for i8  { fn twice(self: Self) -> Self { self * 2i8 } }
        impl Twice for i16 { fn twice(self: Self) -> Self { self * 2i16 } }
        impl Twice for i32 { fn twice(self: Self) -> Self { self * 2i32 } }

        fn main() -> u64 {
            val a: u8 = 1u8
            val b: u16 = 2u16
            val c: u32 = 4u32
            val d: i8 = 8i8
            val e: i16 = 16i16
            val f: i32 = 32i32
            a.twice() as u64 + b.twice() as u64 + c.twice() as u64
                + d.twice() as u64 + e.twice() as u64 + f.twice() as u64
        }
    "#;
    // 2 + 4 + 8 + 16 + 32 + 64. A width that falls out of the mapping
    // does not answer wrong — it fails to compile, so any omission
    // shows up as a failure rather than a number.
    assert_eq!(interpreter_value(src) & 0xff, 126);
    assert_consistent(src, "narrow_receiver_methods");
}

/// `f32` was absent from all four tables, so this impl used to
/// type-check as one on an unknown identifier and report the
/// memorable "expected f32, but got f32".
#[test]
fn an_f32_receiver_dispatches_an_extension_method() {
    let src = r#"
        trait Twice { fn twice(self: Self) -> Self }
        impl Twice for f32 { fn twice(self: Self) -> Self { self * 2f32 } }

        fn main() -> u64 {
            val g: f32 = 64f32
            val d: f32 = g.twice()
            d as u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 128);
    assert_consistent(src, "f32_receiver_method");
}

/// A narrow receiver returning `Option<Self>` — the shape
/// `core/std/checked.t` wants for widths other than 64-bit, and the
/// combination of both fixes: the nested `Self` and the narrow
/// receiver.
#[test]
fn a_narrow_receiver_returns_an_option_of_self() {
    let src = r#"
        trait Dec { fn safe_dec(self: Self) -> Option<Self> }
        impl Dec for u8 {
            fn safe_dec(self: Self) -> Option<Self> {
                if self == 0u8 { Option::None } else { Option::Some(self - 1u8) }
            }
        }

        fn main() -> u64 {
            val x: u8 = 5u8
            val a: Option<u8> = x.safe_dec()
            val zero: u8 = 0u8
            val b: Option<u8> = zero.safe_dec()
            val hi = match a { Option::Some(v) => v as u64, Option::None => 99u64 }
            val lo = match b { Option::Some(_) => 99u64, Option::None => 1u64 }
            hi * 10u64 + lo
        }
    "#;
    // 4 from the decrement, 1 from the guarded zero: the `None` arm
    // has to be reachable, or a subtraction underflow would trap.
    assert_eq!(interpreter_value(src) & 0xff, 41);
    assert_consistent(src, "narrow_option_self");
}

/// A literal receiver, without a name binding. The primitive path
/// lowers the receiver as an expression, so this needs no binding —
/// but it only gets there once the width is in the mapping.
#[test]
fn a_literal_narrow_receiver_needs_no_binding() {
    let src = r#"
        trait Twice { fn twice(self: Self) -> Self }
        impl Twice for u8 { fn twice(self: Self) -> Self { self * 2u8 } }

        fn main() -> u64 {
            21u8.twice() as u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 42);
    assert_consistent(src, "literal_narrow_receiver");
}
