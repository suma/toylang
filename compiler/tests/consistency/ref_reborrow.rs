//! REF-REBORROW: forwarding an existing `&mut` needs no borrow written.
//!
//! Reading a reference binding auto-dereferences it, so handing a
//! `&mut T` parameter straight to another `&mut T` parameter used to
//! be `expected &mut T, but got T`. Every function that rewrites a
//! tree or a graph has that shape, and the only way out was to write
//! `&mut arena` -- re-borrowing something already borrowed.
//!
//! Only **forwarding** became implicit. Taking a `&mut` of an owned
//! value still has to be written, because that is a decision about the
//! local: it is the difference between the callee seeing your value
//! and the callee changing it. Forwarding decides nothing -- the
//! caller already granted mutable access, and passing it on cannot
//! grant more. `frontend/tests/method_resolution_tests.rs` holds the
//! rejection half; what matters here is that the writes actually
//! arrive, on every lane.

use super::harness::*;

/// A free function forwarding its own `&mut` parameter.
#[test]
fn a_forwarded_mut_parameter_still_writes_through() {
    let src = r#"
        struct P { x: u64, y: u64 }

        fn leaf(p: &mut P, n: u64) { p.x = p.x + n }
        fn outer(p: &mut P, n: u64) {
            leaf(p, n)
            leaf(p, n)
        }

        fn main() -> u64 {
            var p = P { x: 0u64, y: 1u64 }
            # The borrow is written once, where the decision is made.
            outer(&mut p, 5u64)
            p.x + p.y
        }
    "#;
    assert_consistent(src, "reborrow_free_function");
}

/// Into a method's `&mut` parameter, from a function that only has a
/// reference itself. This is the shape the frontend test used to
/// reject; the point of accepting it is that the write arrives.
#[test]
fn a_forwarded_mut_parameter_reaches_a_method() {
    let src = r#"
        struct Sink { n: u64 }
        impl Sink {
            fn add(&mut self, d: u64) { self.n = self.n + d }
        }

        struct Helper { k: u64 }
        impl Helper {
            fn fill(&self, out: &mut Sink) { out.add(1u64) }
        }

        fn outer(out: &mut Sink) {
            val h = Helper { k: 0u64 }
            h.fill(out)
            h.fill(out)
        }

        fn main() -> u64 {
            var s = Sink { n: 0u64 }
            outer(&mut s)
            s.n
        }
    "#;
    assert_consistent(src, "reborrow_into_method_param");
}

/// The recursive case the item was filed for: a function that rewrites
/// an arena hands its `&mut` on to itself.
#[test]
fn a_recursive_call_forwards_its_own_borrow() {
    let src = r#"
        struct Node { v: u64, used: u64 }

        fn insert(arena: &mut Vec<Node>, depth: u64, v: u64) -> u64 {
            if depth == 0u64 {
                arena.push(Node { v: v, used: 1u64 })
                return arena.size()
            }
            insert(arena, depth - 1u64, v + 1u64)
        }

        fn main() -> u64 {
            var arena: Vec<Node> = Vec::new()
            insert(&mut arena, 2u64, 10u64)
            insert(&mut arena, 0u64, 99u64)
            val n = arena.size()
            val first = arena.get(0u64)
            # Two nodes, the first pushed after two levels of recursion.
            n * 100u64 + first.v
        }
    "#;
    assert_consistent(src, "reborrow_recursive_arena");
}

/// A wide receiver *and* a forwarded wide `&mut` parameter, so the
/// reborrow rides the pointer ABI rather than the leaf-by-leaf one.
/// If the address were copied instead of handed on, the innermost
/// write would not reach `main`.
#[test]
fn a_forwarded_borrow_survives_the_pointer_abi() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64, e: u64, f: u64,
            g: u64, h: u64, i: u64, j: u64, k: u64, l: u64,
        }

        fn leaf(w: &mut Wide, n: u64) { w.a = w.a + n }
        fn mid(w: &mut Wide, n: u64) {
            leaf(w, n)
            leaf(w, n)
        }
        fn outer(w: &mut Wide, n: u64) {
            mid(w, n)
            mid(w, n)
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 0u64, c: 0u64, d: 0u64, e: 0u64, f: 0u64,
                g: 0u64, h: 0u64, i: 0u64, j: 0u64, k: 0u64, l: 9u64,
            }
            outer(&mut w, 1u64)
            # Four writes of 1, plus the untouched l.
            w.a * 10u64 + w.l
        }
    "#;
    assert_consistent(src, "reborrow_through_pointer_abi");
}

/// A shared `&T` forwards the same way, and must not become mutable on
/// the way.
#[test]
fn a_forwarded_shared_reference_still_reads() {
    let src = r#"
        struct P { x: u64, y: u64 }

        fn readit(p: &P) -> u64 { p.x }
        fn outer(p: &P) -> u64 { readit(p) + readit(p) }

        fn main() -> u64 {
            var q = P { x: 9u64, y: 1u64 }
            outer(&q) + q.x
        }
    "#;
    assert_consistent(src, "reborrow_shared_reference");
}
