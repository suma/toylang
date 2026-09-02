//! TREE-WALKER-SELF-TYPE-ARG: a declared return type names `Self`'s
//! type arguments.
//!
//! `val w: Option<Win<u64>> = Win::try_from_raw(p)` has always worked:
//! the annotation is the only thing that says what `Self` is for a
//! constructor whose arguments mention none of its type parameters,
//! and every lane reads it. The same call in a body's tail —
//!
//! ```text
//! fn window(&self) -> Option<Win<u64>> { Win::try_from_raw(self.data) }
//! ```
//!
//! says exactly the same thing through the declared return type, and
//! the tree-walker was not consulting it. The value came back with no
//! type arguments, so a later `__builtin_sizeof::<T>()` reached by a
//! method on it failed on an unbound `T` while the three compiled
//! lanes ran the program.
//!
//! It mattered beyond the diagnostic: `Vec::as_span` and
//! `String::as_span` could not be written as the single
//! `Span::try_from_raw_parts` call the API exists to offer, and
//! carried a two-step spelling instead.

use super::harness::*;

/// The shape the gap was found in: the instantiation is available
/// only from the enclosing function's return type, and the caller is
/// not itself generic.
#[test]
fn a_return_type_instantiates_a_constructor() {
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
            fn stride(&self) -> u64 { __builtin_sizeof::<T>() }
        }

        struct Holder { data: ptr }

        impl Holder {
            fn window(&self) -> Option<Win<u64>> {
                Win::try_from_raw(self.data)
            }
            fn narrow(&self) -> Option<Win<u8>> {
                Win::try_from_raw(self.data)
            }
        }

        fn main() -> u64 {
            val h = Holder { data: __builtin_heap_alloc(8u64) }
            var acc: u64 = 0u64
            match h.window() {
                Option::Some(w) => { acc = acc + w.stride() }
                Option::None => { acc = acc + 100u64 }
            }
            match h.narrow() {
                Option::Some(w) => { acc = acc + w.stride() }
                Option::None => { acc = acc + 100u64 }
            }
            acc
        }
    "#;
    // 8 for the u64 window, 1 for the u8 one — two instantiations of
    // the same constructor, told apart only by the return types.
    assert_eq!(interpreter_value(src) & 0xff, 9);
    assert_consistent(src, "return_type_instantiates");
}

/// A free function, and an early `return` rather than a tail — the
/// annotation has to hold for the whole body, not just its last
/// expression.
#[test]
fn an_early_return_is_instantiated_too() {
    let src = r#"
        struct Win<T> { addr: ptr }

        impl<T> Win<T> {
            fn try_from_raw(p: ptr) -> Option<Self> { Option::Some(Win { addr: p }) }
            fn stride(&self) -> u64 { __builtin_sizeof::<T>() }
        }

        fn make(early: bool, p: ptr) -> Option<Win<u64>> {
            if early {
                return Win::try_from_raw(p)
            }
            Option::None
        }

        fn main() -> u64 {
            val p = __builtin_heap_alloc(8u64)
            match make(true, p) {
                Option::Some(w) => w.stride(),
                Option::None => 100u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "return_type_instantiates_early_return");
}

/// The window covers the whole body, so it has to stop at anything
/// that speaks for itself. A `val` with its own annotation keeps it —
/// otherwise every `Cell::of` inside a `-> Option<Cell<u64>>` would
/// silently become a `Cell<u64>`.
#[test]
fn a_bindings_own_annotation_wins_over_the_return_type() {
    let src = r#"
        struct Cell<T> { v: T }

        impl<T> Cell<T> {
            fn of(v: T) -> Self { Cell { v: v } }
            fn stride(&self) -> u64 { __builtin_sizeof::<T>() }
        }

        fn build() -> Option<Cell<u64>> {
            # `u8`, said here — the enclosing `Option<Cell<u64>>` must
            # not claim it.
            val small: Cell<u8> = Cell::of(3u8)
            val n = small.stride()
            val big: Cell<u64> = Cell::of(7u64)
            if n == 1u64 { Option::Some(big) } else { Option::None }
        }

        fn main() -> u64 {
            match build() {
                Option::Some(c) => c.stride(),
                Option::None => 100u64,
            }
        }
    "#;
    // The inner cell strides 1 (so the `Some` arm is taken) and the
    // returned one strides 8. A leak would make the inner one 8, take
    // the `None` arm, and answer 100.
    assert_eq!(interpreter_value(src) & 0xff, 8);
    assert_consistent(src, "return_type_does_not_leak");
}

/// The stdlib shape this unblocked: both window constructors are now
/// the single fallible call, and a write through the window still
/// reaches the owner's buffer.
#[test]
fn the_stdlib_windows_are_one_call_each() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u8> = Vec::new()
            v.push(1u8)
            v.push(2u8)

            var acc: u64 = 0u64
            match v.as_span() {
                Option::Some(s) => {
                    s.set(0u64, 9u8)
                    acc = acc + s.len()
                }
                Option::None => { acc = acc + 100u64 }
            }
            acc = acc + v.get(0u64) as u64

            var s: String = String::from_str("ab")
            match s.as_span() {
                Option::Some(bytes) => { acc = acc + bytes.len() }
                Option::None => { acc = acc + 100u64 }
            }

            val fresh: Vec<u8> = Vec::new()
            match fresh.as_span() {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            acc
        }
    "#;
    // 2 (len) + 9 (written through the window) + 2 (String bytes)
    // + 1 (an unallocated Vec has no window).
    assert_eq!(interpreter_value(src) & 0xff, 14);
    assert_consistent(src, "stdlib_windows_one_call");
}
