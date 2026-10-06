//! CONCURRENCY B1 — `spawn { body }` and `Task<T>`.
//!
//! Every lane runs the body where the `spawn` is written, which is one
//! of the orders real threads can take (`design-docs/CONCURRENCY.md`
//! section 7, point 2). These tests pin that answer before any thread
//! exists, the way `parallel_for.rs` pinned A1's: when B2 moves the
//! body onto a thread, an answer that changes is a bug in the
//! threading, not a new semantics.

use super::harness::*;

#[test]
fn a_task_hands_back_its_value() {
    let src = r#"
        fn work(n: u64) -> u64 {
            var acc: u64 = 0u64
            for i in 0u64..n {
                acc = acc + i
            }
            acc
        }

        fn main() -> u64 {
            val t: Task<u64> = spawn { work(10u64) }
            println(t.is_done())
            println(t.join())
            0u64
        }
    "#;
    assert_renders(src, "spawn_value", "true\n45\n");
}

#[test]
fn an_owned_capture_moves_into_the_body_and_dies_with_it() {
    // The string is the body's once it is captured: the parent no
    // longer drops it, and the body does on its way out, so it is gone
    // by the time the spawn expression has a value. What remains is
    // the task's own result slot (one `u64`).
    let src = r#"
        fn main() -> u64 {
            val buf = String::from_str("payload bytes")
            val before: u64 = __builtin_live_bytes()
            val t: Task<u64> = spawn { buf.len() }
            println(before - __builtin_live_bytes())
            println(t.join())
            0u64
        }
    "#;
    assert_renders(src, "spawn_moves_capture", "5\n13\n");
}

#[test]
fn a_scalar_capture_is_a_copy() {
    let src = r#"
        fn main() -> u64 {
            val base: u64 = 40u64
            val t: Task<u64> = spawn { base + 2u64 }
            println(base)
            println(t.join())
            0u64
        }
    "#;
    assert_renders(src, "spawn_copies_scalar", "40\n42\n");
}

#[test]
fn an_unjoined_task_drops_its_value() {
    // Nothing joins `t`; its drop waits for the body and drops the
    // value, so the program ends holding nothing.
    let src = r#"
        fn make() -> u64 {
            val t: Task<u64> = spawn { 7u64 }
            1u64
        }

        fn main() -> u64 {
            val before: u64 = __builtin_live_bytes()
            val r = make()
            println(__builtin_live_bytes() - before)
            println(r)
            0u64
        }
    "#;
    assert_renders(src, "spawn_unjoined", "0\n1\n");
}

#[test]
fn a_body_produces_a_compound_value() {
    // A `Result` from a call, and an owned `String` built in
    // the body and handed back through `join`.
    let src = r#"
        fn main() -> u64 {
            val digits = String::from_str("1234")
            val t: Task<Result<u64, ParseError>> = spawn {
                val parsed = parse::to_u64(digits.to_str())
                parsed
            }
            val r = t.join()
            match r {
                Result::Ok(n) => println(n + 1u64),
                Result::Err(e) => println("bad"),
            }
            val u: Task<String> = spawn {
                var s = String::from_str("made ")
                s.push_str("inside")
                s
            }
            val made = u.join()
            println(made)
            0u64
        }
    "#;
    assert_renders(src, "spawn_compound", "1235\nmade inside\n");
}

#[test]
fn a_capture_moved_into_a_body_can_be_handed_back() {
    // The body owns `v` and returns it, so it is not dropped at the
    // body's end: `join` gives the same vector back.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            val t: Task<Vec<u64>> = spawn {
                var w = v
                w.push(2u64)
                w
            }
            val back = t.join()
            println(back.size())
            println(back.get(1u64))
            0u64
        }
    "#;
    assert_renders(src, "spawn_hand_back", "2\n2\n");
}
