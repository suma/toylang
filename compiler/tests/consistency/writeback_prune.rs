//! CODE-SIZE-WB-PRUNE: narrowing the `&mut self` writeback tail.
//!
//! A `&mut self` method returns every leaf of its receiver so the
//! caller can store the mutated values back. `compiler_lower`'s
//! `writeback_prune` drops the slots whose leaf the body never writes
//! — the caller already holds those values, so returning them copies
//! what it just passed in.
//!
//! The pass is invisible when it is right and silently loses updates
//! when it is wrong, which is what these tests are for. Each one
//! mutates a subset of a wide receiver and reads **all** of it back,
//! so a slot dropped too eagerly shows up as a stale field rather
//! than as a crash.
//!
//! The receivers here are deliberately wider than the eight argument
//! registers: below that the whole tail fits in registers and the
//! pass has nothing to prove.

use super::harness::*;

/// The base case: one field written, eleven untouched, all twelve read
/// back. The untouched ones must survive the round trip even though
/// their return slots are gone.
#[test]
fn an_untouched_field_survives_a_narrowed_writeback() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn bump(&mut self) { self.a = self.a + 100u64 }
            fn total(&self) -> u64 {
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
            w.bump()
            w.bump()
            # 78 (the sum 1..12) + 200
            w.total()
        }
    "#;
    assert_consistent(src, "wb_prune_untouched_field");
}

/// Several `return` sites, each of which used to materialise the whole
/// tail. The early exits must still carry the write that happened
/// before them.
#[test]
fn every_return_site_carries_the_same_narrowed_tail() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            # Three exits, one of them after the write.
            fn step(&mut self, mode: u64) -> u64 {
                if mode == 0u64 { return 7u64 }
                self.a = self.a + 1u64
                if mode == 1u64 { return 8u64 }
                self.b = self.b + 1u64
                9u64
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 0u64, b: 0u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            var acc: u64 = 0u64
            acc = acc + w.step(0u64)   # no write
            acc = acc + w.step(1u64)   # a only
            acc = acc + w.step(2u64)   # a and b
            # 7 + 8 + 9 = 24, a = 2, b = 1, c = 3
            acc + w.a * 100u64 + w.b * 10u64 + w.c
        }
    "#;
    assert_consistent(src, "wb_prune_early_returns");
}

/// The fixpoint. `outer` writes nothing itself; it only calls `inner`.
/// Until `inner` is narrowed, `outer` looks like it writes every leaf
/// (the call's dests cover them all), so this only passes once the
/// pass iterates.
#[test]
fn a_caller_narrows_after_its_callee_does() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        impl Wide {
            fn inner(&mut self) { self.c = self.c + 5u64 }
            fn outer(&mut self) -> u64 {
                self.inner()
                self.inner()
                self.c
            }
        }

        fn main() -> u64 {
            var w = Wide {
                a: 1u64, b: 2u64, c: 0u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 8u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            val seen = w.outer()
            # c is 10 after two calls; a and l must be untouched.
            seen + w.c + w.a + w.l
        }
    "#;
    assert_consistent(src, "wb_prune_fixpoint");
}

/// A `&mut <compound>` **parameter** rides the same writeback list as
/// the receiver, so it is pruned by the same rule.
#[test]
fn a_mut_compound_parameter_narrows_too() {
    let src = r#"
        struct Wide {
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
            i: u64, j: u64, k: u64, l: u64,
        }

        fn touch_one(w: &mut Wide) { w.h = w.h + 3u64 }

        fn main() -> u64 {
            var w = Wide {
                a: 1u64, b: 2u64, c: 3u64, d: 4u64,
                e: 5u64, f: 6u64, g: 7u64, h: 0u64,
                i: 9u64, j: 10u64, k: 11u64, l: 12u64,
            }
            touch_one(&mut w)
            touch_one(&mut w)
            w.h * 10u64 + w.a + w.l
        }
    "#;
    assert_consistent(src, "wb_prune_mut_param");
}

/// A receiver holding an owning type. `Vec`'s own methods (`push`,
/// `clear`, `set_size`) are among the ones the pass narrows, so this
/// exercises the pruning inside the stdlib as well as at this level.
#[test]
fn a_narrowed_writeback_still_grows_a_vec_field() {
    let src = r#"
        struct Holder {
            xs: Vec<u64>,
            a: u64, b: u64, c: u64, d: u64,
            e: u64, f: u64, g: u64, h: u64,
        }

        impl Holder {
            fn add(&mut self, v: u64) { self.xs.push(v) }
            fn sum(&self) -> u64 {
                var t: u64 = 0u64
                var i: u64 = 0u64
                while i < self.xs.size() {
                    t = t + self.xs.get(i)
                    i = i + 1u64
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
            }
            hd.add(10u64)
            hd.add(20u64)
            hd.add(30u64)
            # 60 from the Vec, plus the scalars that were never written.
            hd.sum() + hd.a + hd.h
        }
    "#;
    assert_consistent(src, "wb_prune_vec_field");
}

/// `dyn` dispatch is the shape the pass has to refuse: a thunk
/// allocates fresh locals purely to catch the writeback, so dropping a
/// slot would strand one undefined. The impl method is reachable both
/// directly and through the vtable here, which is exactly the case the
/// veto exists for.
#[test]
fn a_dyn_receiver_keeps_its_full_writeback() {
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
            val direct = w.step()      # a = 1
            val viaDyn = pump(&mut w)  # a = 2
            direct + viaDyn + w.a + w.l
        }
    "#;
    assert_consistent(src, "wb_prune_dyn_receiver");
}
