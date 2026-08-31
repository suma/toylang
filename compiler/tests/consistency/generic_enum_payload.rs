//! A generic type inside an enum payload, across a function boundary.
//!
//! `Option<Foo<T>>` used to be unwritable in two independent ways, one
//! per layer, and both diagnostics pointed at the caller rather than at
//! the gap:
//!
//! - **SELF-IN-TYPE-ARG.** The `Self` normalisations matched only the
//!   top level of a return type, so `-> Self` resolved and
//!   `-> Option<Self>` did not; a caller with a fully explicit
//!   `val o: Option<Ptr<u64>> = ...` was told it "expected
//!   Option<Ptr<u64>>, but got Option<Self>".
//! - **GENERIC-IN-ENUM-PAYLOAD.** The monomorphiser reads a generic
//!   type's arguments off the val annotation, but only when the
//!   annotation names that type at its top level. Wrapped in an enum,
//!   the annotation looked like it said nothing about `Ptr`, and the
//!   compiled lanes asked for "an explicit type annotation" that was
//!   already there. Fixed on both sides: the annotation is searched at
//!   depth, and an enum-returning associated call binds its result as
//!   an enum instead of falling through to a path that bound the
//!   receiver's struct.
//!
//! The tests below pin the shapes that were measured before the fix
//! (2026-08-31): each of the three ways `T` can be determined, plus the
//! `Self` spelling, plus the stdlib function the work was for.

use super::harness::*;

/// `T` comes from the return type alone, via `Self` — the spelling a
/// constructor actually uses.
#[test]
fn an_associated_function_returns_an_option_of_self() {
    let src = r#"
        struct Win<T> { addr: ptr }

        impl<T> Win<T> {
            fn try_from_raw(p: ptr) -> Option<Self> {
                if __builtin_ptr_is_null(p) {
                    Option::None
                } else {
                    Option::Some(Win { addr: p })
                }
            }
        }

        fn main() -> u64 {
            val some: Option<Win<u64>> = Win::try_from_raw(__builtin_heap_alloc(8u64))
            val none: Option<Win<u64>> = Win::try_from_raw(__builtin_null_ptr())
            var acc: u64 = 0u64
            match some {
                Option::Some(_) => { acc = acc + 2u64 }
                Option::None => { acc = acc + 20u64 }
            }
            match none {
                Option::Some(_) => { acc = acc + 200u64 }
                Option::None => { acc = acc + 1u64 }
            }
            acc
        }
    "#;
    // 2 + 1: the non-null address is Some, the null one is None. A
    // build that lost the null check lands on 202.
    assert_eq!(interpreter_value(src) & 0xff, 3);
    assert_consistent(src, "option_of_self");
}

/// Same shape written out as `Option<Win<T>>` rather than
/// `Option<Self>`, and with `T` determined by an argument instead of by
/// the return type — the monomorphiser has to reach the annotation
/// either way.
#[test]
fn a_generic_struct_crosses_a_function_boundary_inside_an_option() {
    let src = r#"
        struct Win<T> { addr: ptr }

        impl<T> Win<T> {
            fn from_raw(p: ptr) -> Self { Win { addr: p } }
            fn wrap(w: Win<T>) -> Option<Win<T>> { Option::Some(w) }
        }

        fn main() -> u64 {
            val w: Win<u64> = Win::from_raw(__builtin_heap_alloc(8u64))
            val o: Option<Win<u64>> = Win::wrap(w)
            match o {
                Option::Some(inner) => {
                    if __builtin_ptr_is_null(inner.addr) { 0u64 } else { 5u64 }
                }
                Option::None => 9u64,
            }
        }
    "#;
    // The payload has to survive the round trip: 5 means the address
    // came back, 0 means an empty window was reconstructed.
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "generic_struct_in_option");
}

/// The stdlib function this was for (CONV-SPAN): lifting an existing
/// address into a `Ptr<T>` without pretending a null one is valid.
#[test]
fn ptr_try_from_raw_rejects_null_and_keeps_the_address() {
    let src = r#"
        fn main() -> u64 {
            val owned: Ptr<u64> = Ptr::alloc(2u64)
            val view: Option<Ptr<u64>> = Ptr::try_from_raw(owned.as_raw())
            val nothing: Option<Ptr<u64>> = Ptr::try_from_raw(__builtin_null_ptr())
            var acc: u64 = 0u64
            match view {
                Option::Some(p) => {
                    # A window, not a copy: the write has to be visible
                    # through the pointer it was lifted from.
                    p.set(0u64, 7u64)
                    acc = acc + owned.get(0u64)
                }
                Option::None => { acc = acc + 100u64 }
            }
            match nothing {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            acc
        }
    "#;
    // 7 written through the lifted window and read back through the
    // original, plus 1 for the rejected null.
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "ptr_try_from_raw");
}

/// A phantom type parameter has to survive the payload. `Ptr<T>` keeps
/// `T` in the type and only an untyped address in the struct, so a
/// value that arrives through an `Option` payload rather than through
/// an annotated binding used to reach `fn get(&self, i: u64) -> T`
/// with `T` unbound — the tree-walker derived a value's type arguments
/// from its *field values* alone, which a phantom parameter is by
/// definition absent from.
#[test]
fn a_payload_bound_pointer_can_read_its_elements() {
    let src = r#"
        fn main() -> u64 {
            val owned: Ptr<u64> = Ptr::alloc(2u64)
            owned.set(0u64, 7u64)
            val view: Option<Ptr<u64>> = Ptr::try_from_raw(owned.as_raw())
            match view {
                Option::Some(p) => p.get(0u64),
                Option::None => 9u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "payload_ptr_get");
}
