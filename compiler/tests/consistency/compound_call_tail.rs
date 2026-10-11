//! COMPOUND-CALL-EXPR-POS: a compound-returning call as a function's
//! result -- the body's tail, a block's tail behind it, or `return`.
//!
//! The compiled lanes refused all of these with
//!
//! ```text
//! compiler MVP cannot use a struct-returning call (`mk`) in expression
//! position; bind the result with `val`
//! ```
//!
//! so `fn mk() -> String { String::from_str("x") }` could not be
//! written. The same call already worked as a `val` RHS, as an argument
//! and as the tail of an `if` arm; the body tail now takes the arm's
//! route (store into fresh leaf locals, return those). Enum returns
//! always took it, which is why only struct and tuple failed.

use super::harness::*;

/// The three call shapes that can produce a struct, each as a body
/// tail, plus one behind a block's tail.
#[test]
fn a_struct_returning_call_is_a_function_result() {
    let src = r#"
        struct P { x: i64, y: i64 }

        impl P {
            fn new(x: i64, y: i64) -> P { P { x: x, y: y } }
            fn origin() -> P { P::new(0i64, 0i64) }
            fn swapped(&self) -> P { P::new(self.y, self.x) }
            fn twice_swapped(&self) -> P { self.swapped().swapped() }
        }

        fn mk(a: i64) -> P { P { x: a, y: a + 1i64 } }
        fn via_free(a: i64) -> P { mk(a) }
        fn via_block(a: i64) -> P {
            val b = a * 2i64
            { mk(b) }
        }

        fn main() -> i64 {
            val o = P::origin()
            val f = via_free(3i64)
            val b = via_block(5i64)
            val s = f.swapped()
            o.x + o.y + f.x + b.y + s.x
        }
    "#;
    // 0 + 0 + 3 + 11 + 4.
    assert_eq!(interpreter_value(src) & 0xff, 18);
    assert_consistent(src, "compound_call_tail_struct");
}

#[test]
fn a_tuple_returning_call_is_a_function_result() {
    let src = r#"
        fn pair(a: i64) -> (i64, i64) { (a, a + 1i64) }
        fn mixed(a: i64) -> (i64, bool, u64) { (a, true, 2u64) }
        fn via_pair(a: i64) -> (i64, i64) { pair(a * 10i64) }
        fn via_mixed(a: i64) -> (i64, bool, u64) { mixed(a) }

        fn main() -> i64 {
            val p = via_pair(2i64)
            val m = via_mixed(4i64)
            if m.1 { p.0 + p.1 + m.0 * (m.2 as i64) } else { 0i64 }
        }
    "#;
    // 20 + 21 + 4 * 2.
    assert_eq!(interpreter_value(src) & 0xff, 49);
    assert_consistent(src, "compound_call_tail_tuple");
}

/// `return <call>` (and `return <literal>` for a struct, which was
/// refused the same way), including an early one that skips the tail.
#[test]
fn an_explicit_return_takes_a_compound_call() {
    let src = r#"
        struct P { x: i64, y: i64 }

        fn mk(a: i64) -> P { P { x: a, y: a + 1i64 } }
        fn pair(a: i64) -> (i64, i64) { (a, a * 2i64) }

        fn pick(a: i64) -> P {
            if a > 10i64 { return mk(a) }
            if a < 0i64 { return P { x: 0i64, y: 0i64 } }
            mk(a + 100i64)
        }

        fn split(a: i64) -> (i64, i64) {
            if a == 0i64 { return pair(7i64) }
            return pair(a)
        }

        fn main() -> i64 {
            val a = pick(20i64)
            val b = pick(-1i64)
            val c = pick(1i64)
            val s = split(0i64)
            val t = split(3i64)
            a.x + b.y + (c.x - 100i64) + s.1 + t.0
        }
    "#;
    // 20 + 0 + 1 + 14 + 3.
    assert_eq!(interpreter_value(src) & 0xff, 38);
    assert_consistent(src, "compound_call_tail_return");
}

/// Ownership: the returned value is the caller's, and what the callee
/// bound around the call is still dropped -- once, in the same order on
/// every lane. A `String` built by the stdlib exercises the case that
/// motivated the fix.
#[test]
fn a_returned_call_drops_like_a_bound_one() {
    let src = r#"
        struct H { n: u64 }
        impl Drop for H { fn drop(&mut self) { println("drop {self.n}") } }

        fn mk(n: u64) -> H { H { n: n } }
        fn wrap(n: u64) -> H {
            val keep = H { n: 100u64 }
            mk(n + keep.n)
        }
        fn early(n: u64) -> H {
            val tmp = mk(7u64)
            if n > 0u64 { return mk(n) }
            mk(tmp.n)
        }
        fn label(n: u64) -> String {
            val a = String::from_str("x")
            val b = String::from_str("y")
            if n > 0u64 { return a.concat(b) }
            b.concat(a)
        }

        fn main() -> u64 {
            val a = wrap(1u64)
            val b = early(2u64)
            val c = early(0u64)
            val l1 = label(1u64)
            val l0 = label(0u64)
            println(l1)
            println(l0)
            println(__builtin_live_bytes())
            a.n + b.n + c.n
        }
    "#;
    assert_stdout_consistent(src, "compound_call_tail_drop");
}
