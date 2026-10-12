//! COMPOUND-CALL-EXPR-POS: a compound-returning call read through --
//! `mk().n`, `pair(1).0`, `mk().get()`, `v.get(i).len()` -- in any
//! position: a `val` RHS, an operand, a condition, an argument, a
//! `println` argument.
//!
//! The compiled lanes refused these ("field-access chains rooted at a
//! bare identifier", "method calls on a bare identifier or a
//! field-access chain"); the call now lands in fresh storage the chain
//! resolves through. Each call must still run exactly once, which is
//! what the `println` inside `mk` is for.

use super::harness::*;

#[test]
fn a_call_result_is_read_through_in_any_position() {
    let src = r#"
        struct In { x: u64, y: u64 }
        struct P { n: u64, inner: In, v: Vec<u64> }
        impl P {
            fn get(&self) -> u64 { self.n }
        }
        fn mk(n: u64) -> P {
            println("mk {n}")
            var v: Vec<u64> = Vec::new()
            v.push(n)
            P { n: n, inner: In { x: n + 1u64, y: n + 2u64 }, v: v }
        }
        fn triple(a: u64) -> (u64, u64, u64) {
            println("triple {a}")
            (a, a + 1u64, a + 2u64)
        }
        fn take(i: In) -> u64 { i.x * 10u64 + i.y }

        fn main() -> u64 {
            val a: u64 = mk(1u64).n
            val b = mk(2u64).inner.y
            println(mk(3u64).n)
            val c = take(mk(4u64).inner)
            var d = 0u64
            if mk(5u64).get() > 4u64 { d = 1u64 }
            val e = triple(6u64).1
            val f = mk(7u64).v.size()
            val i = mk(8u64).inner
            val j = mk(9u64).n + triple(10u64).2
            println("{a} {b} {c} {d} {e} {f} {i.x} {j}")
            a + b + c + d + e + f + j
        }
    "#;
    assert_stdout_consistent(src, "compound_call_receiver_positions");
}

/// The stdlib shapes that motivated it: a method on a constructed
/// `String`, and on an element borrowed out of a `Vec<String>`.
#[test]
fn a_method_on_a_returned_string_runs() {
    let src = r#"
        fn main() -> u64 {
            var w: Vec<String> = Vec::new()
            w.push(String::from_str("hello"))
            w.push(String::from_str("hi"))
            val a = String::from_str("abc").len()
            val b = w.get(0u64).len() + w.get(1u64).len()
            println("{a} {b}")
            a + b
        }
    "#;
    assert_stdout_consistent(src, "compound_call_receiver_string");
}

/// COMPOUND-TEMP-RECEIVER: a method called on the result of a call that
/// returns a compound owning nothing (`f().to_str()`, `mk().get()`) --
/// and `println(f())` for a type with `Display`, which the checker turns
/// into `println(f().to_str())`. The compiled lanes took a method only
/// on a name or a field chain; the receiver is now bound to a `val` in
/// a block of its own, as an owning one already was.
#[test]
fn a_method_on_a_non_owning_compound_temporary() {
    let src = r#"
        struct P { x: i64 }
        impl P { fn get(&self) -> i64 { self.x } }
        enum Color { Red, Green }
        fn f(a: i64, b: i64) -> Ordering { a.cmp(b) }
        fn mk(x: i64) -> P { P { x: x } }
        fn col(b: bool) -> Color { if b { Color::Red } else { Color::Green } }
        fn main() -> u64 {
            println(f(1i64, 2i64))
            println(col(false))
            println(mk(4i64))
            val s = f(2i64, 2i64).to_str()
            println(s)
            println(mk(5i64).get())
            println(3u64.cmp(1u64))
            0u64
        }
    "#;
    assert_renders(
        src,
        "compound_temp_receiver",
        "Less\nColor::Green\nP { x: 4 }\nEqual\n5\nGreater\n",
    );
}
