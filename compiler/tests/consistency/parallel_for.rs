//! CONCURRENCY A1 / A2-b-2 — `parallel for`.
//!
//! The modifier says the iterations may run in any order, and since
//! A2-b-2 the compiled lanes do: the lowering outlines the body and
//! `toy_par_for` hands chunks of the range to threads. The IR VM and
//! the tree-walker still run it in order, which is a legal split of
//! the same range.
//!
//! That is the whole point of having shipped A1 first. The answer was
//! pinned while every lane was sequential, so these tests did not
//! change when the threads arrived — **a parallel loop that answers
//! differently is a bug in the parallelism, not a new semantics**
//! (`design-docs/CONCURRENCY.md` section 6).
//!
//! So these tests are not about parallelism. They are about the
//! answer the parallel form has to keep giving, whichever lane and
//! however many threads run it.

use super::harness::*;

#[test]
fn a_parallel_loop_answers_what_the_plain_loop_answers() {
    // The same body, written both ways, over a slot per index — the
    // shape the modifier exists for (CONCURRENCY.md section 1: the
    // work is per segment and the writes are disjoint by index).
    let src = r#"
        fn fill(out: &mut Vec<u64>, n: u64) {
            var i: u64 = 0u64
            while i < n {
                out.push(0u64)
                i = i + 1u64
            }
        }

        fn total(v: &Vec<u64>) -> u64 {
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < v.size() {
                sum = sum + v.get(i)
                i = i + 1u64
            }
            sum
        }

        fn main() -> u64 {
            var plain: Vec<u64> = Vec::new()
            fill(&mut plain, 16u64)
            for i in 0u64..16u64 {
                plain.set(i, i * i)
            }

            var par: Vec<u64> = Vec::new()
            fill(&mut par, 16u64)
            parallel for i in 0u64..16u64 {
                par.set(i, i * i)
            }

            println(total(&plain))
            println(total(&par))
            println(total(&plain) == total(&par))
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_matches_plain", "1240\n1240\ntrue\n");
}

#[test]
fn parallel_is_still_a_name() {
    // Contextual: only the identifier immediately before a `for` is
    // the modifier. A program that already had a `parallel` binding
    // keeps it.
    //
    // The body writes a slot per index rather than accumulating —
    // an accumulator is order-dependent and E0029 refuses it, which
    // is what the `an_accumulator_is_refused` test below pins.
    let src = r#"
        fn main() -> u64 {
            val parallel = 7u64
            var v: Vec<u64> = Vec::new()
            var k: u64 = 0u64
            while k < 3u64 {
                v.push(0u64)
                k = k + 1u64
            }
            val w = v.as_span()
            match w {
                Option::Some(s) => {
                    parallel for i in 0u64..3u64 {
                        s.set(i, i)
                    }
                }
                Option::None => { }
            }
            var acc: u64 = 0u64
            var j: u64 = 0u64
            while j < v.size() {
                acc = acc + v.get(j)
                j = j + 1u64
            }
            println(parallel)
            println(acc)
            0u64
        }
    "#;
    assert_renders(src, "parallel_as_a_name", "7\n3\n");
}

#[test]
fn a_parallel_loop_carries_its_range_semantics() {
    // Half-open, and an empty range runs nothing — the same rules the
    // plain `for` has. Counting happens in the slots rather than in a
    // running total, so the count survives being split.
    //
    // The empty range matters twice over now: the lowering hands
    // `toy_par_for` a *count*, and a backwards or empty one has to
    // arrive as 0 rather than as a wrapped `end - start`.
    let src = r#"
        fn tally(v: &Vec<u64>) -> u64 {
            var sum: u64 = 0u64
            var i: u64 = 0u64
            while i < v.size() {
                sum = sum + v.get(i)
                i = i + 1u64
            }
            sum
        }

        fn zeroed(n: u64) -> Vec<u64> {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < n {
                v.push(0u64)
                i = i + 1u64
            }
            v
        }

        fn main() -> u64 {
            var empty: Vec<u64> = zeroed(4u64)
            val ew = empty.as_span()
            match ew {
                Option::Some(s) => {
                    parallel for i in 3u64..3u64 {
                        s.set(i, 1u64)
                    }
                }
                Option::None => { }
            }
            println(tally(&empty))

            var four: Vec<u64> = zeroed(4u64)
            val fw = four.as_span()
            match fw {
                Option::Some(s) => {
                    parallel for i in 0u64..4u64 {
                        s.set(i, 1u64)
                    }
                }
                Option::None => { }
            }
            println(tally(&four))
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_range", "0\n4\n");
}

#[test]
fn a_window_in_a_struct_field_writes_through_to_its_buffer() {
    // CONCURRENCY A2-b groundwork. The plan for outlining a
    // `parallel for` body was to build an environment struct holding
    // what the body captures — and it was written as
    // `struct Env { out: &mut Vec<u64> }`, which **cannot exist**:
    // a struct field may not be a reference (REF-Stage-2 (e)).
    //
    // A window can. `Span<T>` is an ordinary struct, so it lives in
    // a field, and `set` writes through to the buffer it views —
    // which is exactly the shape the workloads need (a slot per
    // index). This pins that the substitute works, and works the
    // same on every lane, before anything is built on it.
    let src = r#"
        struct Env { out: Span<u64>, base: u64 }

        fn work(e: &Env, from: u64, until: u64) {
            var i: u64 = from
            while i < until {
                e.out.set(i, e.base + i * i)
                i = i + 1u64
            }
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::with_capacity(8u64)
            var k: u64 = 0u64
            while k < 8u64 {
                v.push(0u64)
                k = k + 1u64
            }
            val w = v.as_span()
            match w {
                Option::Some(sp) => {
                    val e = Env { out: sp, base: 100u64 }
                    # Two halves, as a split range would be.
                    work(&e, 0u64, 4u64)
                    work(&e, 4u64, 8u64)
                }
                Option::None => { }
            }
            var total: u64 = 0u64
            var j: u64 = 0u64
            while j < v.size() {
                total = total + v.get(j)
                j = j + 1u64
            }
            println(total)
            0u64
        }
    "#;
    assert_renders(src, "window_in_env_struct", "940\n");
}

#[test]
fn a_split_range_covers_every_index_exactly_once() {
    // A2-b-2's own claim. The lowering hands `toy_par_for` a count
    // and the runtime cuts it into chunks; every index has to be
    // visited once, whatever the cut. A slot per index says so
    // precisely — a total could add up while two chunks overlapped
    // and a third was skipped.
    //
    // 64 indices is more than the threads any test machine has, so
    // the range is really split rather than handed to one chunk.
    let src = r#"
        fn zeroed(n: u64) -> Vec<u64> {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < n {
                v.push(0u64)
                i = i + 1u64
            }
            v
        }

        fn main() -> u64 {
            var v: Vec<u64> = zeroed(64u64)
            val w = v.as_span()
            match w {
                Option::Some(s) => {
                    parallel for i in 0u64..64u64 {
                        s.set(i, s.get(i) + 1u64)
                    }
                }
                Option::None => { }
            }
            var once: u64 = 0u64
            var other: u64 = 0u64
            var i: u64 = 0u64
            while i < v.size() {
                if v.get(i) == 1u64 { once = once + 1u64 } else { other = other + 1u64 }
                i = i + 1u64
            }
            println(once)
            println(other)
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_covers_once", "64\n0\n");
}

#[test]
fn a_parallel_body_reads_what_it_captured() {
    // The environment carries scalars and structs, and the body sees
    // the values they held when the loop started. Both kinds at once:
    // `base` is a scalar, the window is a struct of two leaves, and
    // `scale` comes from a `&` parameter.
    let src = r#"
        fn fill(out: Span<u64>, n: u64, base: u64, scale: &u64) {
            parallel for i in 0u64..n {
                out.set(i, base + i * scale)
            }
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var k: u64 = 0u64
            while k < 8u64 {
                v.push(0u64)
                k = k + 1u64
            }
            val w = v.as_span()
            val scale: u64 = 10u64
            match w {
                Option::Some(s) => { fill(s, 8u64, 100u64, &scale) }
                Option::None => { }
            }
            var total: u64 = 0u64
            var j: u64 = 0u64
            while j < v.size() {
                total = total + v.get(j)
                j = j + 1u64
            }
            println(total)
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_captures", "1080\n");
}

#[test]
fn a_parallel_loop_over_a_signed_range_counts_the_same() {
    // The lowering passes a *count* and the body adds its base back,
    // so a range that starts below zero needs no special case — and
    // an unsigned `end - start` on it would be nonsense.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            var k: u64 = 0u64
            while k < 6u64 {
                v.push(0i64)
                k = k + 1u64
            }
            val w = v.as_span()
            match w {
                Option::Some(s) => {
                    parallel for i in -3i64..3i64 {
                        s.set((i + 3i64) as u64, i * i)
                    }
                }
                Option::None => { }
            }
            var total: i64 = 0i64
            var j: u64 = 0u64
            while j < v.size() {
                total = total + v.get(j)
                j = j + 1u64
            }
            println(total)
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_signed_range", "19\n");
}

#[test]
fn a_nested_parallel_loop_is_the_inner_one_run_plainly() {
    // Only the outer loop is split: the threads are already spent,
    // and nesting would oversubscribe them (CONCURRENCY.md section
    // 6). The answer is the same either way, which is what this
    // pins — the decision is about how many threads, not about what
    // the program means.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var k: u64 = 0u64
            while k < 12u64 {
                v.push(0u64)
                k = k + 1u64
            }
            val w = v.as_span()
            match w {
                Option::Some(s) => {
                    parallel for i in 0u64..4u64 {
                        parallel for j in 0u64..3u64 {
                            s.set(i * 3u64 + j, i + j)
                        }
                    }
                }
                Option::None => { }
            }
            var total: u64 = 0u64
            var m: u64 = 0u64
            while m < v.size() {
                total = total + v.get(m)
                m = m + 1u64
            }
            println(total)
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_nested", "30\n");
}

#[test]
fn a_parallel_loop_allocates_nothing() {
    // The environment is a slot in the caller's frame, and
    // `toy_par_for` keeps its jobs in its own — so a loop that was
    // allocation-free sequentially stays allocation-free, and
    // `ensures allocates(0)` still holds over one.
    //
    // It is a consistency test because the counters are the thing
    // most easily made lane-specific: a lane that modelled the
    // environment as a heap block would answer 24 here and nothing
    // else would change.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var k: u64 = 0u64
            while k < 8u64 {
                v.push(0u64)
                k = k + 1u64
            }
            val w = v.as_span()
            val before: u64 = __builtin_live_bytes()
            match w {
                Option::Some(s) => {
                    parallel for i in 0u64..8u64 {
                        s.set(i, i * 2u64)
                    }
                }
                Option::None => { }
            }
            println(__builtin_live_bytes() - before)
            println(v.get(7u64))
            0u64
        }
    "#;
    assert_renders(src, "parallel_for_allocation_free", "0\n14\n");
}
