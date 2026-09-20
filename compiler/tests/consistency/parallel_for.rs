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
