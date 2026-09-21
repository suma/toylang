//! CONCURRENCY A1 — `parallel for`.
//!
//! The modifier says the iterations may run in any order, and later
//! at the same time. **Every lane runs it sequentially today**, which
//! is a correct implementation of that promise and the reason the
//! semantics can be pinned before any thread exists: once the answer
//! is fixed here, parallelising it is an optimisation that cannot
//! change it (`design-docs/CONCURRENCY.md` section 6).
//!
//! So these tests are not about parallelism. They are about the
//! answer the parallel form has to keep giving.

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
    let src = r#"
        fn main() -> u64 {
            val parallel = 7u64
            var acc: u64 = 0u64
            parallel for i in 0u64..3u64 {
                acc = acc + i
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
    // plain `for` has, because it *is* the plain `for`.
    let src = r#"
        fn main() -> u64 {
            var seen: u64 = 0u64
            parallel for i in 3u64..3u64 {
                seen = seen + 1u64
            }
            println(seen)
            var last: u64 = 0u64
            parallel for i in 0u64..4u64 {
                last = last + 1u64
            }
            println(last)
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
