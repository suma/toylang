//! CODE-SIZE-SELF-ABI: receivers that travel as a pointer.
//!
//! Past eight leaves, a `&self` / `&mut self` receiver is handed over
//! as one address instead of as its flattened fields. The body is
//! unchanged -- it still names the same leaf locals -- but codegen
//! turns those reads and writes into loads and stores through the
//! incoming pointer, so the caller's storage is the only copy.
//!
//! Two things can go wrong, and neither announces itself:
//!
//! * a mutation writes a leaf local that nobody reads back, and the
//!   change is lost;
//! * a **by-value** receiver gets pointer-passed, and a change that
//!   should have stayed inside the callee escapes to the caller.
//!
//! So every test here reads the whole receiver back afterwards, and
//! the by-value case asserts the caller's copy did *not* move.
//!
//! Receivers are twelve leaves wide on purpose: eight or fewer ride in
//! argument registers and the pointer form never engages.

use super::harness::*;

/// The base case: mutate one field through a wide `&mut self`, read
/// every field back.
#[test]
fn a_wide_mut_receiver_mutates_in_place() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn bump(&mut self) { self.a = self.a + 10u64 }
            fn total(&self) -> u64 {
                self.a + self.b + self.c + self.d
                    + self.e + self.f + self.g + self.h
                    + self.i + self.j + self.k + self.l
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            w.bump()
            w.bump()
            w.bump()
            # 30 written into `a`, plus 77 the other fields still hold.
            w.total()
        }
    "#;
    assert_consistent(src, "ptr_self_mut_receiver");
}

/// Forwarding, which is the whole point of the change. `outer` never
/// touches a field itself; it hands `self` down twice. Once its own
/// receiver is a pointer, passing it on has to reach the same storage
/// the caller is looking at.
#[test]
fn a_receiver_forwarded_down_a_chain_reaches_one_storage() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn leaf(&mut self, n: u64) -> u64 {
                self.a = self.a + n
                self.a
            }
            fn mid(&mut self, n: u64) -> u64 {
                var t: u64 = 0u64
                t = t + self.leaf(n)
                t = t + self.leaf(n)
                t
            }
            fn outer(&mut self, n: u64) -> u64 {
                var t: u64 = 0u64
                t = t + self.mid(n)
                t = t + self.mid(n)
                t
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            # a runs 1,2,3,4 -> the running sums add to 10.
            val seen = w.outer(1u64)
            seen * 100u64 + w.a * 10u64 + w.l
        }
    "#;
    assert_consistent(src, "ptr_self_forwarded_chain");
}

/// An early `return` in the middle of a forwarding chain: the writes
/// made before it must still be visible, and the ones after must not
/// have happened.
#[test]
fn an_early_return_keeps_the_writes_made_before_it() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn step(&mut self, stop: bool) -> u64 {
                self.a = self.a + 1u64
                if stop { return 5u64 }
                self.b = self.b + 1u64
                6u64
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 0u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            var acc: u64 = 0u64
            acc = acc + w.step(true)
            acc = acc + w.step(false)
            # a = 2, b = 1, acc = 11
            acc * 100u64 + w.a * 10u64 + w.b
        }
    "#;
    assert_consistent(src, "ptr_self_early_return");
}

/// A **by-value** wide receiver must keep its own copy, so it is left
/// out of the pointer form: sharing the caller's storage would let a
/// write inside the callee escape.
///
/// The receiver is read, not written, because the natural way to write
/// one (`var s = self`, then assign through `s`) is a compound alias
/// whose tree-walker and compiled answers already disagree, with or
/// without this change -- see BY-VALUE-SELF-ALIAS in `todo.md`. What
/// is checked here is that a wide by-value receiver still arrives
/// intact, which is the half the pointer decision could break.
#[test]
fn a_by_value_receiver_still_arrives_whole() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn consumed(self: Self) -> u64 {
                self.a + self.b + self.c + self.d
                    + self.e + self.f + self.g + self.h
                    + self.i + self.j + self.k + self.l
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 1u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            # 78 from the callee's copy, and `w` still readable here.
            w.consumed() + w.a + w.l
        }
    "#;
    assert_consistent(src, "ptr_self_by_value_receiver");
}

/// A wide receiver holding an owning field. `Vec`'s own methods are
/// narrow, so this exercises the mixed case: a narrow `&mut self` call
/// whose writeback lands in leaf locals that are themselves behind the
/// outer receiver's pointer.
#[test]
fn a_vec_field_grows_through_a_pointer_receiver() {
    let src = r#"
        struct Holder {
            xs: Vec<u64>,
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64,
        }

        impl Holder {
            fn add(&mut self, v: u64) { self.xs.push(v) }
            fn sum(&self) -> u64 {
                var t: u64 = 0u64
                var n: u64 = 0u64
                while n < self.xs.size() {
                    t = t + self.xs.get(n)
                    n = n + 1u64
                }
                t
            }
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var hd = Holder {
                xs: v,
                a: 1u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64,
            }
            hd.add(10u64)
            hd.add(20u64)
            hd.add(30u64)
            hd.sum() + hd.a + hd.j
        }
    "#;
    assert_consistent(src, "ptr_self_vec_field");
}

/// `dyn` dispatch. The thunk already writes the receiver behind
/// `data_ptr` using the same layout the pointer ABI expects, so it
/// forwards that address rather than unpacking and repacking. Calling
/// the same method directly and through the trait object has to leave
/// the receiver in the same state either way.
#[test]
fn a_dyn_dispatch_forwards_the_data_pointer() {
    let src = r#"
        trait Step {
            fn step(&mut self) -> u64
        }

        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Step for Wide {
            fn step(&mut self) -> u64 {
                self.a = self.a + 1u64
                self.a
            }
        }

        fn pump(s: &mut dyn Step) -> u64 { s.step() }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            val direct = w.step()
            val viaDyn = pump(&mut w)
            val again = w.step()
            # 1 + 2 + 3 = 6, and `a` ends at 3.
            direct + viaDyn + again + w.a * 10u64
        }
    "#;
    assert_consistent(src, "ptr_self_dyn_dispatch");
}

/// A receiver that is exactly at the threshold stays in registers, and
/// one leaf wider does not. Both spellings have to give the same
/// answer, which is what says the two paths agree rather than merely
/// each being self-consistent.
#[test]
fn the_threshold_does_not_change_the_answer() {
    let src = r#"
        struct Eight {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
        }
        struct Nine {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64,
        }

        impl Eight {
            fn bump(&mut self) -> u64 { self.a = self.a + 1u64  self.a + self.h }
        }
        impl Nine {
            fn bump(&mut self) -> u64 { self.a = self.a + 1u64  self.a + self.h }
        }

        fn main() -> u64 {
            var p = Eight { a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                            e: 5u64, f: 6u64, g: 7u64, h: 8u64 }
            var q = Nine { a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                           e: 5u64, f: 6u64, g: 7u64, h: 8u64, i: 9u64 }
            val x = p.bump() + p.bump()
            val y = q.bump() + q.bump()
            # The two shapes must agree; the difference is 0.
            (x - y) + p.a + q.a
        }
    "#;
    assert_consistent(src, "ptr_self_threshold");
}

/// S3(a): a wide `&mut T` **parameter** of a free function takes the
/// same pointer form the receiver does, and the methods it calls on
/// that parameter forward the address rather than rebuilding a copy.
/// This is the shape `flush_segment(w: &mut ArchiveWriter, ...)` has
/// in `poc/logsearch`, and it was the largest remaining cost there.
///
/// (Handing the parameter on to another `&mut T` *function* is not
/// spelled here: the type checker has no reborrow, so `leaf(w, n)`
/// inside `fn outer(w: &mut Wide)` is rejected as `Wide` vs
/// `&mut Wide` -- a language gap, not an ABI one.)
#[test]
fn a_wide_mut_reference_parameter_is_passed_by_address() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn bump(&mut self, n: u64) -> u64 {
                self.a = self.a + n
                self.a
            }
            fn peek(&self) -> u64 { self.a + self.l }
        }

        fn drive(w: &mut Wide, n: u64) -> u64 {
            var t: u64 = 0u64
            t = t + w.bump(n)
            t = t + w.bump(n)
            t + w.peek()
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            # a runs 1 then 2 -> 3, plus peek() = 2 + 12.
            val seen = drive(&mut w, 1u64)
            seen * 100u64 + w.a * 10u64 + w.l
        }
    "#;
    assert_consistent(src, "ptr_self_mut_ref_param");
}

/// A read-only `&T` parameter is just as expensive to spread out, so
/// it takes the pointer form too -- and must not let a write escape,
/// since there is none to make.
#[test]
fn a_wide_shared_reference_parameter_reads_correctly() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        fn total(w: &Wide) -> u64 {
            w.a + w.b + w.c + w.d + w.e + w.f
                + w.g + w.h + w.i + w.j + w.k + w.l
        }
        fn twice(w: &Wide) -> u64 { total(w) + total(w) }

        fn main() -> u64 {
            var w = Wide {
                a: 1u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            # 78 twice, and `w` unchanged.
            twice(&w) + w.a
        }
    "#;
    assert_consistent(src, "ptr_self_shared_ref_param");
}

/// A receiver and a reference parameter that are both wide, in one
/// signature: the two pointers must not be crossed.
#[test]
fn a_receiver_and_a_reference_parameter_stay_distinct() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn take_from(&mut self, other: &Wide) -> u64 {
                self.a = self.a + other.a
                self.b = self.b + other.l
                self.a * 100u64 + self.b
            }
        }

        fn main() -> u64 {
            var dst = Wide {
                a: 1u64, b: 1u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            var src2 = Wide {
                a: 100u64, b: 0u64, c: 0u64, d: 0u64,
                e: 0u64, f: 0u64, g: 0u64, h: 0u64,
                i: 0u64, j: 0u64, k: 0u64, l: 200u64,
            }
            val seen = dst.take_from(&src2)
            # dst.a = 101, dst.b = 201; src2 must be untouched.
            seen + dst.a + dst.b + src2.a + src2.l
        }
    "#;
    assert_consistent(src, "ptr_self_receiver_and_ref_param");
}

/// A wide **generic** struct. The receiver decision is made when a
/// method is declared, and a generic method is instantiated later on a
/// separate path -- so this pins that the two agree, and that a type
/// parameter in the middle of the leaf list does not shift the layout.
#[test]
fn a_wide_generic_struct_round_trips() {
    let src = r#"
        struct Box9<T> {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, v: T,
        }

        impl<T> Box9<T> {
            fn bump(&mut self, k: u64) -> u64 {
                self.a = self.a + k
                self.a
            }
            fn total(&self) -> u64 { self.a + self.i }
        }

        fn drive(b: &mut Box9<u64>, k: u64) -> u64 {
            var t: u64 = 0u64
            t = t + b.bump(k)
            t = t + b.bump(k)
            t + b.total()
        }

        fn main() -> u64 {
            var b: Box9<u64> = Box9 {
                a: 0u64, b: 0u64, c: 0u64, d: 0u64,
                e: 0u64, f: 0u64, g: 0u64, h: 0u64,
                i: 5u64, v: 7u64,
            }
            # bump twice -> 1 + 2 = 3, total() = 2 + 5.
            val seen = drive(&mut b, 1u64)
            seen * 100u64 + b.a * 10u64 + b.v
        }
    "#;
    assert_consistent(src, "ptr_self_generic_struct");
}

/// Reborrowing: a `&mut` parameter handed on to a free function and to
/// a method of a *field*, while the enclosing method also writes
/// through its own receiver. Two pointers and a reborrow of one of
/// them, in one body.
#[test]
fn a_reborrowed_mut_parameter_reaches_the_same_storage() {
    let src = r#"
        struct Sink {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, n: u64,
        }

        impl Sink {
            fn add(&mut self, k: u64) { self.n = self.n + k }
            fn len(&self) -> u64 { self.n }
        }

        struct Inner { q: u64 }
        impl Inner {
            fn emit(&mut self, out: &mut Sink) { out.add(4u64) }
        }

        fn emit_free(out: &mut Sink) { out.add(2u64) }

        struct Writer { inner: Inner, count: u64 }
        impl Writer {
            fn write(&mut self, out: &mut Sink) -> u64 {
                out.add(1u64)
                emit_free(&mut out)
                self.inner.emit(&mut out)
                self.count = self.count + 1u64
                out.len()
            }
        }

        fn main() -> u64 {
            var s = Sink {
                a: 0u64, b: 0u64, c: 0u64, d: 0u64,
                e: 0u64, f: 0u64, g: 0u64, h: 0u64,
                i: 0u64, j: 0u64, n: 0u64,
            }
            var w = Writer { inner: Inner { q: 0u64 }, count: 0u64 }
            # 1 + 2 + 4 all land in the one `Sink`.
            val n = w.write(&mut s)
            n * 100u64 + s.n * 10u64 + w.count
        }
    "#;
    assert_consistent(src, "ptr_self_reborrowed_param");
}
