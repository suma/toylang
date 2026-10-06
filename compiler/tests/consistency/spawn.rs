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
            println(t.join())
            println(t.is_done())
            0u64
        }
    "#;
    assert_renders(src, "spawn_value", "45\ntrue\n");
}

#[test]
fn an_owned_capture_moves_into_the_body_and_dies_with_it() {
    // The string is the body's once it is captured: the parent no
    // longer drops it, and the body does on its way out, so it is gone
    // once the task has been joined. What remains is the task's own
    // result slot (one `u64`), which lives as long as the task.
    //
    // Measured after the join: before it, the body may or may not have
    // run yet, and a program that looks then is asking when.
    let src = r#"
        fn main() -> u64 {
            val buf = String::from_str("payload bytes")
            val before: u64 = __builtin_live_bytes()
            val t: Task<u64> = spawn { buf.len() }
            println(t.join())
            println(before - __builtin_live_bytes())
            0u64
        }
    "#;
    assert_renders(src, "spawn_moves_capture", "13\n5\n");
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

#[test]
fn several_tasks_run_at_once_and_join_in_any_order() {
    // On the compiled lanes these four bodies run on four threads,
    // each with an owned vector of its own; on the sequential ones,
    // one after another at their spawns. Joined out of spawn order,
    // the answer is the same.
    let src = r#"
        fn total(v: &Vec<u64>) -> u64 {
            var acc: u64 = 0u64
            for i in 0u64..v.size() {
                acc = acc + v.get(i)
            }
            acc
        }

        fn filled(n: u64, k: u64) -> Vec<u64> {
            var v: Vec<u64> = Vec::new()
            for i in 0u64..n {
                v.push(i * k)
            }
            v
        }

        fn main() -> u64 {
            val a = filled(1000u64, 1u64)
            val b = filled(1000u64, 2u64)
            val c = filled(1000u64, 3u64)
            val d = filled(1000u64, 4u64)
            val ta: Task<u64> = spawn { total(&a) }
            val tb: Task<u64> = spawn { total(&b) }
            val tc: Task<u64> = spawn { total(&c) }
            val td: Task<u64> = spawn { total(&d) }
            val rd = td.join()
            val rb = tb.join()
            val ra = ta.join()
            val rc = tc.join()
            println(ra + rb + rc + rd)
            0u64
        }
    "#;
    assert_renders(src, "spawn_four", "4995000\n");
}

#[test]
fn an_event_loop_waits_for_a_task_beside_its_sockets() {
    // CONCURRENCY B3: the task's descriptor goes into a `Poller` like
    // a socket's. On a compiled lane it turns readable when the
    // thread finishes; on a sequential one the body has already run,
    // so the first `wait` reports it. How many turns the loop took is
    // the one thing that differs, so it is not printed.
    let src = r#"
        fn work(seed: u64) -> u64 {
            var acc: u64 = seed
            for i in 0u64..200000u64 {
                acc = acc ^ (acc * 31u64 + i)
            }
            acc
        }

        fn main() -> u64 {
            val made = Poller::new()
            var p = match made {
                Result::Ok(p) => p,
                Result::Err(e) => panic("no poller"),
            }
            val t: Task<u64> = spawn { work(7u64) }
            val fd = t.as_fd()
            println(fd == t.as_fd())
            val reg = p.register(fd, 42u64, interest_read())
            var waiting = true
            while waiting {
                val got = p.wait(1000i64)
                val n = got ?? 0u64
                for i in 0u64..n {
                    val ev = p.event(i)
                    if ev.token() == 42u64 && ev.is_readable() {
                        waiting = false
                    }
                }
            }
            val dereg = p.deregister(fd)
            println(t.is_done())
            println(t.join() == work(7u64))
            0u64
        }
    "#;
    assert_renders(src, "spawn_poll", "true\ntrue\ntrue\n");
}

#[test]
fn a_body_may_end_in_a_call_that_produces_a_compound_value() {
    // The most natural spawn is `spawn { write(buf) }` with a struct
    // coming back. The compiled lanes cannot return a compound value
    // straight from a call in tail position, so the outlining binds the
    // tail to a `val` first; written by hand, it used to be a lowering
    // error.
    let src = r#"
        struct Out { n: u64, buf: Vec<u64> }

        fn write_out(v: Vec<u64>) -> Out {
            val n = v.size()
            Out { n: n, buf: v }
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(4u64)
            v.push(5u64)
            val t: Task<Out> = spawn { write_out(v) }
            val o = t.join()
            println(o.n)
            println(o.buf.get(1u64))
            0u64
        }
    "#;
    assert_renders(src, "spawn_compound_tail", "2\n5\n");
}
