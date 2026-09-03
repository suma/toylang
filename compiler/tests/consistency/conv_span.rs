//! CONV-SPAN: turning an existing buffer into a `Span<T>`.
//!
//! `Span<T>` is the library's answer to `&[T]`, but until now the only
//! way to obtain one was `Ptr::alloc` — a fresh allocation. A buffer
//! that already existed (`String`, `Vec<T>`, an address from a C
//! library) could not be viewed through one, because `Ptr<T>` had no
//! constructor taking a raw address. That made the window type
//! unreachable for exactly the buffers it is most useful for.
//!
//! Two rules run through the API and are what these tests pin:
//!
//! - **A raw address entering the type system yields an `Option`**, so
//!   `Ptr<T>`'s non-null invariant (POINTER P5) holds by construction
//!   rather than by convention. An empty `Vec` has no allocation, so
//!   it has no window.
//! - **An out-of-range index panics**, like `Span::get` and `Vec::get`
//!   already do. A bad index is a program error, not a value.
//!
//! And the property that makes the whole thing worth having: every one
//! of these is a *view*. A write through a slice of a span has to be
//! visible through the vector it came from, or the copy the API exists
//! to avoid is still happening somewhere.

use super::harness::*;

/// `slice` is a window, not a copy, and its bounds are checked
/// against the parent.
#[test]
fn a_span_slice_views_the_same_memory() {
    let src = r#"
        fn main() -> u64 {
            val p: Ptr<u64> = Ptr::alloc(4u64)
            p.set(0u64, 1u64)
            p.set(1u64, 2u64)
            p.set(2u64, 3u64)
            p.set(3u64, 4u64)
            val whole: Option<Span<u64>> = Span::try_from_raw_parts(p.as_raw(), 4u64)
            match whole {
                Option::Some(win) => {
                    val mid: Span<u64> = win.slice(1u64, 2u64)
                    mid.set(0u64, 20u64)
                    # 20 read back through the slice, 3 through the
                    # slice's second element, 20 again through the
                    # *original* pointer — a copying `slice` would
                    # leave that one at 2.
                    mid.get(0u64) + mid.get(1u64) + p.get(1u64) + mid.len()
                }
                Option::None => 0u64,
            }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 45);
    assert_consistent(src, "span_slice_view");
}

/// A null address has no window. `Span::try_from_raw_parts` and
/// `Ptr::try_from_raw` answer the same way, so the invariant holds
/// however the window is built.
#[test]
fn a_null_address_yields_no_window() {
    let src = r#"
        fn main() -> u64 {
            val none_span: Option<Span<u64>> =
                Span::try_from_raw_parts(__builtin_null_ptr(), 4u64)
            val none_ptr: Option<Ptr<u64>> = Ptr::try_from_raw(__builtin_null_ptr())
            var acc: u64 = 0u64
            match none_span {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            match none_ptr {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 2u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 3);
    assert_consistent(src, "null_yields_no_window");
}

/// The buffer-filling shape the API is for: reserve once, let
/// something write into the reserved room, then declare how much of
/// it is real. `as_span` and `capacity_span` are the two halves —
/// live elements versus the whole allocation.
#[test]
fn reserved_room_is_writable_before_the_elements_are_live() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::with_capacity(4u64)
            var acc: u64 = 0u64
            acc = acc + v.capacity()
            # Nothing is live yet, so the live-element window is empty
            # while the allocation window is not.
            val live_before: Option<Span<u64>> = v.as_span()
            match live_before {
                Option::Some(s) => { acc = acc + s.len() }
                Option::None => { acc = acc + 100u64 }
            }
            val room: Option<Span<u64>> = v.capacity_span()
            match room {
                Option::Some(s) => {
                    s.set(0u64, 7u64)
                    acc = acc + s.len()
                }
                Option::None => { acc = acc + 100u64 }
            }
            # `set_size` is what makes the written element visible to
            # the vector's own API.
            v.set_size(1u64)
            acc = acc + v.size() + v.get(0u64)
            acc
        }
    "#;
    // 4 capacity + 0 live + 4 room + 1 size + 7 element.
    assert_eq!(interpreter_value(src) & 0xff, 16);
    assert_consistent(src, "vec_reserved_room");
}

/// A vector that has never allocated has no window; one that
/// allocated and was emptied does.
#[test]
fn only_an_unallocated_vector_has_no_window() {
    let src = r#"
        fn main() -> u64 {
            val fresh: Vec<u64> = Vec::new()
            var used: Vec<u64> = Vec::with_capacity(2u64)
            used.set_size(0u64)
            var acc: u64 = 0u64
            match fresh.as_span() {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            match used.as_span() {
                Option::Some(s) => { acc = acc + 10u64 + s.len() }
                Option::None => { acc = acc + 100u64 }
            }
            acc
        }
    "#;
    // 1 for the never-allocated vector, 10 + 0 for the allocated but
    // empty one: `None` means "no memory", not "no elements".
    assert_eq!(interpreter_value(src) & 0xff, 11);
    assert_consistent(src, "vec_window_presence");
}

/// A `String`'s bytes, viewed and written in place.
#[test]
fn a_string_lends_its_bytes_as_a_span() {
    let src = r#"
        fn main() -> u64 {
            var s: String = String::from_str("abc")
            var acc: u64 = 0u64
            match s.as_span() {
                Option::Some(bytes) => {
                    # Write through the window; the String has to see it.
                    # Spelled `65u8` rather than `'A'`: a char literal
                    # passed to a generic parameter keeps its 32-bit
                    # default instead of narrowing to the instance's
                    # `u8`, and the compiled lanes fail their verifier
                    # (todo CHAR-LITERAL-GENERIC-ARG).
                    bytes.set(0u64, 65u8)
                    acc = acc + bytes.len()
                }
                Option::None => { acc = acc + 100u64 }
            }
            acc = acc + s.get(0u64) as u64
            # Bound first: a compiled `match` scrutinee has to be an
            # enum binding or a scalar expression (a documented MVP
            # limit, unrelated to spans).
            val fresh: String = String::new()
            match fresh.as_span() {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 1u64 }
            }
            acc
        }
    "#;
    // 3 bytes + 'A' (65) + 1 for the empty String's absent window.
    assert_eq!(interpreter_value(src) & 0xff, 69);
    assert_consistent(src, "string_as_span");
}


// PTR-READ-ASSIGN: `b = __builtin_ptr_read(p, i)`.
//
// The read's width comes from the annotation on a `val`, and an
// assignment has nowhere to put one -- so the same read had to be
// spelled as a fresh binding inside the loop. Worse, the type checker
// did not say so: it fell back to `u64`, and the mismatch was
// reported against whichever statement the recovery anchored on,
// naming a type from somewhere else entirely.
//
// The binding being written to already has a width, which is the same
// answer the annotation would have given.

#[test]
fn a_pointer_read_can_be_assigned_to_an_existing_binding() {
    let src = r#"
        unsafe fn sum(p: ptr, n: u64) -> u64 {
            var total: u64 = 0u64
            var b: u8 = 0u8
            var i: u64 = 0u64
            while i < n {
                b = __builtin_ptr_read(p, i)
                total = total + (b as u64)
                i = i + 1u64
            }
            total
        }

        unsafe fn wide(p: ptr) -> u64 {
            var w: u64 = 0u64
            w = __builtin_ptr_read(p, 0u64)
            w
        }

        unsafe fn main() -> u64 {
            val s = String::from_str("abc")
            val bytes = sum(s.as_ptr(), s.len())
            val q: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(q, 0u64, 41u64)
            bytes + wide(q)
        }
    "#;
    // 97 + 98 + 99 + 41. Both widths, so a narrow read that silently
    // became a `u64` would show up as the wrong sum rather than as an
    // error.
    assert_consistent(src, "ptr_read_assign");
}
