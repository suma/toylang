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


/// COMPOUND-CALL-EXPR-POS: a chain of compound-returning methods
/// (`mk().twin().get()`) and an operator's compound result read through
/// (`(p + p).x`). Each step is bound in a block of its own, all under
/// the same synthetic name -- which also pins that a `val` whose
/// annotated block declares that name again keeps its own value (the
/// lowering used to bind the outer name before the block, and the
/// block's exit left it meaning the inner one: 3 for 6).
#[test]
fn a_chain_of_compound_results_is_read_through() {
    let src = r#"
        struct P { x: i64 }
        impl P {
            fn twin(&self) -> P { P { x: self.x * 2i64 } }
            fn get(&self) -> i64 { self.x }
            fn add(&self, o: &Self) -> P { P { x: self.x + o.x } }
            fn neg(&self) -> P { P { x: 0i64 - self.x } }
        }
        fn mk(x: i64) -> P { P { x: x } }
        fn main() -> u64 {
            println(mk(3i64).twin().get())
            val p = mk(5i64)
            println(p.twin().twin().get())
            val t: P = { val t = mk(3i64)  t.twin() }
            println(t.x)
            println((p + p).x)
            println((p + mk(1i64)).get())
            println((-p).x)
            0u64
        }
    "#;
    assert_renders(src, "compound_chain", "6\n20\n6\n10\n6\n-5\n");
}

/// COMPOUND-CALL-EXPR-POS: a compound is written whole into an array
/// element (`arr[i] = mk()`, a binding, a literal, under `soa` too,
/// a tuple, an enum), built from calls in an array literal, written
/// into a struct field (`o.p = mk()`, `o.p = o.p.swap()` reading the
/// old value), and called as a statement for its effect alone.
#[test]
fn a_compound_is_written_into_any_place() {
    let src = r#"
        struct P { x: i64, y: u8 }
        impl P { fn swap(&self) -> P { P { x: self.y as i64, y: self.x as u8 } } }
        struct O { p: P, n: u64 }
        enum E { A(i64), B }
        fn mk(x: i64) -> P { P { x: x, y: 3u8 } }
        fn me(x: i64) -> E { E::A(x) }
        fn pr(a: u64) -> (u64, i64) { (a, -1i64) }
        fn noisy() -> P { println("made")  mk(0i64) }
        fn main() -> u64 {
            var arr: [P; 3] = [mk(0i64), mk(0i64), mk(0i64)]
            arr[1u64] = mk(7i64)
            val q = P { x: 8i64, y: 9u8 }
            arr[2u64] = q
            arr[0u64] = P { x: 5i64, y: 6u8 }
            val i = 2u64
            arr[i - 1u64] = mk(11i64)
            println(arr[0u64].x + arr[1u64].x + arr[2u64].x)
            println(arr[2u64].y)
            var ss: soa [P; 2] = [mk(0i64), mk(0i64)]
            ss[1u64] = mk(4i64)
            println(ss[1u64].x)
            var ts: [(u64, i64); 2] = [(0u64, 0i64), (0u64, 0i64)]
            ts[1u64] = pr(6u64)
            val t = ts[1u64]
            println(t.1)
            var es: [E; 2] = [E::B, E::B]
            es[0u64] = me(4i64)
            val e0 = es[0u64]
            match e0 {
                E::A(v) => println(v),
                E::B => println("b"),
            }
            var o = O { p: mk(1i64), n: 3u64 }
            o.p = mk(2i64)
            o.p = o.p.swap()
            println(o.p.x)
            println(o.p.y)
            noisy()
            pr(1u64)
            if o.n > 1u64 { noisy() } else { noisy() }
            0u64
        }
    "#;
    assert_renders(
        src,
        "compound_into_place",
        "24\n9\n4\n-1\n4\n3\n2\nmade\nmade\n",
    );
}

/// COMPOUND-CALL-EXPR-POS: a tuple's compound element written whole
/// (`t.0 = mk()`, `u.0 = (7, 8)`), a struct's tuple / enum field
/// (`o.pair = pr()`, `o.e = E::A(9)`), a method on a tuple element
/// (`t.0.swap()`), and a `match` over an enum read out of a place
/// (`es[0]`, `es[i]`, `h.e`) rather than a binding.
#[test]
fn a_compound_place_is_written_and_matched() {
    let src = r#"
        struct P { x: i64, y: i64 }
        impl P { fn swap(&self) -> P { P { x: self.y, y: self.x } } }
        enum E { A(i64), B }
        struct O { pair: (u64, i64), e: E, n: u64 }
        struct H { e: E }
        fn mk() -> P { P { x: 1i64, y: 2i64 } }
        fn pr(a: u64) -> (u64, i64) { (a, -1i64) }
        fn main() -> u64 {
            var t = (P { x: 0i64, y: 0i64 }, 1i64)
            t.0 = mk()
            println(t.0.x)
            t.0 = t.0.swap()
            println(t.0.x)
            var o = O { pair: (0u64, 0i64), e: E::B, n: 1u64 }
            o.pair = pr(5u64)
            println(o.pair.0)
            o.e = E::A(9i64)
            val e = o.e
            match e {
                E::A(v) => println(v),
                E::B => println("b"),
            }
            var u = ((1u64, 2u64), 3u64)
            u.0 = (7u64, 8u64)
            println(u.0.1)
            val es: [E; 2] = [E::A(4i64), E::B]
            var s = 0i64
            match es[0u64] {
                E::A(v) => { s = v }
                E::B => {}
            }
            val i = 1u64
            val r = match es[i] {
                E::A(v) => v,
                E::B => 100i64,
            }
            val h = H { e: E::A(7i64) }
            val q = match h.e {
                E::A(v) => v,
                E::B => 0i64,
            }
            println(s + r + q)
            0u64
        }
    "#;
    assert_renders(src, "compound_place_match", "1\n2\n5\n9\n8\n111\n");
}
