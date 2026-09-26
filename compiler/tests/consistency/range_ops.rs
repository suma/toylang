//! MEMORY-ACCESS M3: range operations on `Span<T>`.
//!
//! The stdlib's unit of memory access was the element: a comparison, a
//! search, a fill were each a hand-written loop in toylang, and the
//! ones that mattered for speed were then written a *second* time with
//! `__simd_load` plus a scalar tail. `core/std/string.t` had the same
//! substring scan spelled out in five methods.
//!
//! `__builtin_mem_eq` / `mem_find` / `mem_find_seq` answer about a
//! whole range in one call, defined once in `toylang_rt` (not libc --
//! `memmem` is not portable, and a search that answers differently per
//! platform is not a search). `Span<T>` wraps them; these tests pin
//! that every lane gets the same answer.

use super::harness::*;

/// `find` and `find_seq` agree on where a byte and a sequence are, and
/// on the absence of both. A miss is `None`, not the length that the
/// builtin underneath returns.
#[test]
fn a_byte_window_answers_where_things_are() {
    let src = r#"
        unsafe fn write_at(s: Span<u8>, i: u64, text: str) {
            val src: String = String::from_str(text)
            var k: u64 = 0u64
            while k < src.size() {
                s.set(i + k, src.get(k))
                k = k + 1u64
            }
        }

        unsafe fn main() -> u64 {
            val hp: Ptr<u8> = Ptr::alloc(11u64)
            val np: Ptr<u8> = Ptr::alloc(3u64)
            val hay: Span<u8> = Span::from_parts(hp, 11u64)
            val needle: Span<u8> = Span::from_parts(np, 3u64)
            write_at(hay, 0u64, "hello world")
            write_at(needle, 0u64, "wor")
            var acc: u64 = 0u64
            # "wor" starts at 6
            val at: Option<u64> = hay.find_seq(needle)
            match at {
                Option::Some(k) => { acc = acc + k }
                Option::None => { acc = acc + 100u64 }
            }
            # 'w' is at 6 too
            val one: Option<u64> = hay.find(0x77u8)
            match one {
                Option::Some(k) => { acc = acc + k }
                Option::None => { acc = acc + 100u64 }
            }
            write_at(needle, 0u64, "xyz")
            val miss: Option<u64> = hay.find_seq(needle)
            match miss {
                Option::Some(_) => { acc = acc + 1000u64 }
                Option::None => { acc = acc + 5u64 }
            }
            val gone: Option<u64> = hay.find(0x7Fu8)
            match gone {
                Option::Some(_) => { acc = acc + 1000u64 }
                Option::None => { acc = acc + 7u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src), 24);
    assert_consistent(src, "span_find");
}

/// `fill`, `copy_from` and `bytes_eq` over the same window: the copy
/// is a view-to-view range move, and the comparison is what says it
/// landed.
#[test]
fn a_window_can_be_filled_copied_and_compared() {
    let src = r#"
        unsafe fn main() -> u64 {
            val ap: Ptr<u8> = Ptr::alloc(16u64)
            val bp: Ptr<u8> = Ptr::alloc(16u64)
            val a: Span<u8> = Span::from_parts(ap, 16u64)
            val b: Span<u8> = Span::from_parts(bp, 16u64)
            a.fill(0x41u8)
            b.fill(0x42u8)
            var acc: u64 = 0u64
            if a.bytes_eq(b) { acc = acc + 100u64 }
            b.copy_from(a)
            if b.bytes_eq(a) { acc = acc + 1u64 }
            b.set(15u64, 0x5Au8)
            if b.bytes_eq(a) { acc = acc + 100u64 }
            # A shorter window is never equal to a longer one, whatever
            # the shared prefix says.
            val half: Span<u8> = a.slice(0u64, 8u64)
            if half.bytes_eq(a) { acc = acc + 100u64 }
            acc + a.get(0u64) as u64
        }
    "#;
    assert_eq!(interpreter_value(src), 66);
    assert_consistent(src, "span_fill_copy");
}

/// The edges the builtins define: an empty range equals itself, an
/// empty needle is found at 0, and a needle longer than the haystack
/// is not found.
#[test]
fn the_empty_cases_answer_the_way_the_builtins_say() {
    let src = r#"
        unsafe fn main() -> u64 {
            val ap: Ptr<u8> = Ptr::alloc(4u64)
            val a: Span<u8> = Span::from_parts(ap, 4u64)
            a.fill(0x41u8)
            val empty: Span<u8> = a.slice(0u64, 0u64)
            val long: Span<u8> = a.slice(0u64, 4u64)
            val short: Span<u8> = a.slice(0u64, 2u64)
            var acc: u64 = 0u64
            if empty.bytes_eq(empty) { acc = acc + 1u64 }
            val at_empty: Option<u64> = a.find_seq(empty)
            match at_empty {
                Option::Some(k) => { acc = acc + k + 2u64 }
                Option::None => { acc = acc + 100u64 }
            }
            # The needle is longer than what is left after index 2.
            val tail: Span<u8> = a.slice(2u64, 2u64)
            val over: Option<u64> = tail.find_seq(long)
            match over {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 4u64 }
            }
            val inside: Option<u64> = a.find_seq(short)
            match inside {
                Option::Some(k) => { acc = acc + k + 8u64 }
                Option::None => { acc = acc + 100u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src), 15);
    assert_consistent(src, "span_edges");
}

/// `String::eq` and `String::find_from` are the two callers rewritten
/// onto the range builtins, so their answers are what says the rewrite
/// kept its meaning -- including the SIMD chunk boundary the old
/// hand-written `eq` loop had to get right (16 bytes).
#[test]
fn the_rewritten_string_methods_answer_as_before() {
    let src = r#"
        fn main() -> u64 {
            val a: String = String::from_str("hello world, a good long string")
            val b: String = String::from_str("hello world, a good long string")
            val c: String = String::from_str("hello world, a good long strinX")
            val short: String = String::from_str("hello")
            var acc: u64 = 0u64
            if a == b { acc = acc + 1u64 }
            if a == c { acc = acc + 100u64 }
            if a == short { acc = acc + 100u64 }
            if a != c { acc = acc + 2u64 }
            val h: String = String::from_str("hello world hello")
            val n: String = String::from_str("hello")
            val at: Option<u64> = h.find(n)
            match at {
                Option::Some(k) => { acc = acc + k + 4u64 }
                Option::None => { acc = acc + 100u64 }
            }
            val last: Option<u64> = h.rfind(n)
            match last {
                Option::Some(k) => { acc = acc + k }
                Option::None => { acc = acc + 100u64 }
            }
            val from: Option<u64> = h.find_from(n, 1u64)
            match from {
                Option::Some(k) => { acc = acc + k }
                Option::None => { acc = acc + 100u64 }
            }
            val nope: String = String::from_str("zzz")
            val gone: Option<u64> = h.find(nope)
            match gone {
                Option::Some(_) => { acc = acc + 100u64 }
                Option::None => { acc = acc + 8u64 }
            }
            acc
        }
    "#;
    assert_eq!(interpreter_value(src), 39);
    assert_consistent(src, "string_range_ops");
}

/// SPAN-RANGE-INTRINSIC: `copy_from` / `move_from` / `bytes_eq` are
/// the range instruction itself in every lane, not a call to the
/// method. Wider elements scale the byte count (`count * 8` here), and
/// `move_from` over an overlapping window slides the elements the way
/// `memmove` does.
#[test]
fn wide_windows_copy_move_and_compare_by_element() {
    let src = r#"
        fn main() -> u64 {
            val p: Ptr<u64> = Ptr::alloc(5u64)
            val q: Ptr<u64> = Ptr::alloc(5u64)
            val a: Span<u64> = Span::from_parts(p, 5u64)
            val b: Span<u64> = Span::from_parts(q, 5u64)
            for i in 0u64..5u64 { a.set(i, i + 1u64) }
            b.copy_from(a)
            var acc: u64 = 0u64
            if b.bytes_eq(a) { acc = acc + 1u64 }
            # Only the last element differs: all 40 bytes are compared.
            b.set(4u64, 9u64)
            if b.bytes_eq(a) { acc = acc + 100u64 }
            # Slide [2, 3, 4, 5] down over [1, 2, 3, 4].
            val head: Span<u64> = a.slice(0u64, 4u64)
            val tail: Span<u64> = a.slice(1u64, 4u64)
            head.move_from(tail)
            # a = [2, 3, 4, 5, 5]
            acc + a.get(0u64) * 10u64 + a.get(3u64) * 1000u64 + a.get(4u64) * 100000u64
        }
    "#;
    assert_eq!(interpreter_value(src), 505_021);
    assert_consistent(src, "span_wide_range_ops");
}

/// With no frame for `Span::copy_from`, its length-mismatch panic is
/// reported where the call was written -- by the tree-walker as well as
/// the compiled lanes, so the oracle and the lanes agree on the line.
#[test]
fn a_length_mismatch_is_reported_at_the_call() {
    let src = "fn main() -> u64 {\n\
               \x20   val p: Ptr<u64> = Ptr::alloc(4u64)\n\
               \x20   val a: Span<u64> = Span::from_parts(p, 4u64)\n\
               \x20   val short: Span<u64> = a.slice(0u64, 3u64)\n\
               \x20   a.copy_from(short)\n\
               \x20   0u64\n\
               }\n";
    let tree = tree_walker_error(src);
    assert!(tree.contains("Span::copy_from length mismatch"), "{tree}");
    assert!(tree.contains("test.t:5:"), "tree-walker: {tree}");
    assert!(!tree.contains("span.t"), "tree-walker: {tree}");

    let vm = interpreter_error(src);
    assert!(vm.contains("test.t:5:"), "IR VM: {vm}");

    if skip_e2e() {
        return;
    }
    let (code, stderr) = compiled_run_output(src, "span_mismatch_site").expect("compiles");
    assert_ne!(code, 0);
    assert!(stderr.contains("Span::copy_from length mismatch"), "{stderr}");
    assert!(stderr.contains(":5:"), "AOT: {stderr}");
    assert!(!stderr.contains("span.t"), "AOT: {stderr}");
}
