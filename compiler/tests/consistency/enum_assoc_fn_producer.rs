//! ENUM-ASSOC-FN-PRODUCER: an associated function that returns an enum.
//!
//! An enum-producing position — an `if` arm, a `match` arm, a block's
//! tail, the payload of another enum — accepted a variant
//! construction, a binding, and (since ENUM-ARG-NEST) a call or a
//! method call. An *associated function* was still read as a variant
//! construction, so a fallible constructor was refused with a sentence
//! about the wrong things entirely:
//!
//! ```text
//! branch produces enum `Span` but the surrounding binding expects `Option`
//! ```
//!
//! `Span` is a struct, and `Span::try_from_raw_parts` produces exactly
//! the `Option` the slot wanted. The arm now falls through to
//! resolving the associated function when the name is not a variant of
//! the enum in hand, and takes it when its return type is that enum.

use super::harness::*;

/// A fallible constructor in the tail position of a function
/// returning the same enum — the shape the stdlib's window
/// constructors take.
#[test]
fn a_fallible_constructor_is_the_whole_body() {
    let src = r#"
        struct Handle { id: u64 }

        impl Handle {
            fn open(id: u64) -> Option<Handle> {
                if id == 0u64 {
                    Option::None
                } else {
                    Option::Some(Handle { id: id })
                }
            }
        }

        fn opened(id: u64) -> Option<Handle> {
            Handle::open(id)
        }

        fn main() -> u64 {
            var acc: u64 = 0u64
            match opened(7u64) {
                Option::Some(h) => { acc = acc + h.id }
                Option::None => { acc = acc + 100u64 }
            }
            match opened(0u64) {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "enum_assoc_fn_tail");
}

/// The same constructor filling one arm of an `if`, and one arm of a
/// `match` — the two branch shapes that write into pre-allocated enum
/// storage rather than returning.
#[test]
fn a_fallible_constructor_fills_a_branch() {
    let src = r#"
        struct Handle { id: u64 }

        impl Handle {
            fn open(id: u64) -> Option<Handle> { Option::Some(Handle { id: id }) }
        }

        fn main() -> u64 {
            val flag = true

            val from_if: Option<Handle> = if flag {
                Handle::open(3u64)
            } else {
                Option::None
            }

            val n: u64 = 1u64
            val from_match: Option<Handle> = match n {
                1u64 => Handle::open(4u64),
                _ => Option::None,
            }

            var acc: u64 = 0u64
            match from_if {
                Option::Some(h) => { acc = acc + h.id }
                Option::None => { acc = acc + 100u64 }
            }
            match from_match {
                Option::Some(h) => { acc = acc + h.id }
                Option::None => { acc = acc + 100u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "enum_assoc_fn_branches");
}

/// And as the payload of another enum, which is the position
/// ENUM-ARG-NEST opened for plain calls.
#[test]
fn a_fallible_constructor_is_a_payload() {
    let src = r#"
        struct Handle { id: u64 }

        impl Handle {
            fn open(id: u64) -> Result<Handle, u64> {
                if id == 0u64 { Result::Err(9u64) } else { Result::Ok(Handle { id: id }) }
            }
        }

        fn main() -> u64 {
            val nested: Option<Result<Handle, u64>> = Option::Some(Handle::open(5u64))
            match nested {
                Option::Some(inner) => {
                    match inner {
                        Result::Ok(h) => h.id,
                        Result::Err(e) => e,
                    }
                }
                Option::None => 100u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 5);
    assert_consistent(src, "enum_assoc_fn_payload");
}
