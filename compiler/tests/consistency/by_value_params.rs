//! BY-VALUE-SELF-ALIAS: a by-value parameter is the callee's own copy.
//!
//! Bindings in the tree-walker share their `Rc`, which is what makes
//! `val b = a` an alias for a compound -- the language says so. A
//! parameter passed **by value** was sharing too, so a write inside
//! the callee reached the caller's binding. The compiled lanes copy
//! the flattened leaves and never did that, so the two disagreed --
//! and the tree-walker is the oracle the others are checked against.
//!
//! `design-docs/BACKEND.md` settles which is right: a by-value
//! receiver's mutations are local, and only `&mut self` propagates.
//!
//! Two programs in the tree had been relying on the old behaviour --
//! `interpreter/example/allocator_list.t` and the struct `__setitem__`
//! tests -- and both of them meant `&mut self` all along.

use super::harness::*;

/// A by-value receiver: the write must not escape.
#[test]
fn a_by_value_receiver_keeps_its_mutation_to_itself() {
    let src = r#"
        struct W { a: u64, b: u64 }

        impl W {
            fn consumed(self: Self) -> u64 {
                var s = self
                s.a = s.a + 1000u64
                s.a
            }
        }

        fn main() -> u64 {
            var w = W { a: 7u64, b: 1u64 }
            val inside = w.consumed()
            # 1007 seen inside; `w.a` must still be the 7 it started with.
            inside - w.a
        }
    "#;
    assert_consistent(src, "by_value_receiver_mutation_is_local");
}

/// The same for an ordinary by-value parameter.
#[test]
fn a_by_value_parameter_keeps_its_mutation_to_itself() {
    let src = r#"
        struct W { a: u64, b: u64 }

        fn take(v: W) -> u64 {
            var t = v
            t.a = t.a + 500u64
            t.a
        }

        fn main() -> u64 {
            var x = W { a: 3u64, b: 1u64 }
            val got = take(x)
            got - x.a
        }
    "#;
    assert_consistent(src, "by_value_param_mutation_is_local");
}

/// `&mut` is the spelling that *does* propagate, and it still does.
/// Without this the fix could have been "copy everything", which would
/// be wrong in the other direction and just as quiet.
#[test]
fn a_mut_reference_still_propagates() {
    let src = r#"
        struct W { a: u64, b: u64 }

        impl W {
            fn bump(&mut self) { self.a = self.a + 1000u64 }
        }

        fn add_to(v: &mut W, n: u64) { v.a = v.a + n }

        fn main() -> u64 {
            var w = W { a: 7u64, b: 1u64 }
            w.bump()
            add_to(&mut w, 1u64)
            # 7 + 1000 + 1, minus the thousand so the exit code fits.
            w.a - 1000u64
        }
    "#;
    assert_consistent(src, "mut_reference_still_propagates");
}

/// The copy is structural, not deep. A `Vec`'s buffer is reached
/// through a pointer, so the copy shares it -- three words move, not
/// the elements. An owning struct also *moves* when passed by value
/// (`[E0014]`), which is why the caller does not read `h` afterwards:
/// what this pins is that the copy did not disturb the buffer or the
/// bookkeeping on the way in.
#[test]
fn a_by_value_copy_shares_the_heap_buffer() {
    let src = r#"
        struct Holder { xs: Vec<u64>, n: u64 }

        fn sum_it(h: Holder) -> u64 {
            var t: u64 = 0u64
            var i: u64 = 0u64
            while i < h.xs.size() {
                t = t + h.xs.get(i)
                i = i + 1u64
            }
            t + h.n
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(40u64)
            v.push(2u64)
            val h = Holder { xs: v, n: 1u64 }
            # 40 + 2 read through the shared buffer, plus n.
            sum_it(h)
        }
    "#;
    assert_consistent(src, "by_value_copy_shares_buffer");
}

/// Nested compounds copy through: the spine is rebuilt at every level,
/// so a write to an inner field of the callee's copy stays there.
#[test]
fn a_nested_field_of_a_by_value_copy_is_separate() {
    let src = r#"
        struct Inner { v: u64 }
        struct Outer { inner: Inner, tag: u64 }

        fn touch(o: Outer) -> u64 {
            var t = o
            t.inner.v = t.inner.v + 100u64
            t.inner.v
        }

        fn main() -> u64 {
            var o = Outer { inner: Inner { v: 5u64 }, tag: 0u64 }
            val seen = touch(o)
            # 105 inside, and `o.inner.v` still 5.
            seen - o.inner.v * 21u64
        }
    "#;
    assert_consistent(src, "by_value_nested_field_is_separate");
}
