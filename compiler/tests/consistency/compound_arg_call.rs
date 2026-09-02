//! COMPOUND-ARG-CALL: a compound-returning call in argument position.
//!
//! A compound never travels as one SSA value — it lives in one local
//! per leaf — so a call producing one needs locals to write into.
//! On a `val` RHS those are the new binding's, which is why
//! `val p = mk(3i64)` always worked while `take(mk(3i64))` was
//!
//! ```text
//! compiler MVP cannot use a struct-returning call (`mk`) in expression
//! position; bind the result with `val`
//! ```
//!
//! The enum half of this landed on 2026-09-01 (ENUM-ARG-NEST) and left
//! an asymmetry worth removing on its own: `Option::Some(mk())` was
//! fine while `take(mk())` was not, for no reason the reader could
//! see. An argument slot allocates its own leaf locals here and points
//! the existing `CallStruct` / `CallTuple` / `CallEnum` dests at them.
//!
//! Ownership moves into the callee, as it does for a `val`-bound
//! compound passed by value, so the fresh storage registers no drop of
//! its own. `a_call_argument_frees_the_same_as_a_binding` is what
//! holds that — it compares the two spellings rather than asserting a
//! count, because what matters is that the shorter one did not change
//! anything.

use super::harness::*;

/// The three call shapes that can produce a struct: a free function,
/// an associated function, and a method.
#[test]
fn a_struct_returning_call_is_an_argument() {
    let src = r#"
        struct P { x: i64, y: i64 }

        impl P {
            fn origin() -> P { P { x: 0i64, y: 0i64 } }
            fn twin(&self) -> P { P { x: self.x, y: self.x } }
        }

        fn mk(a: i64) -> P { P { x: a, y: a + 1i64 } }
        fn take(p: P) -> i64 { p.x + p.y }

        fn main() -> i64 {
            val o = P { x: 5i64, y: 9i64 }
            take(P::origin()) + take(o.twin()) + take(mk(2i64))
        }
    "#;
    // 0 + (5 + 5) + (2 + 3).
    assert_eq!(interpreter_value(src) & 0xff, 15);
    assert_consistent(src, "compound_arg_struct_call");
}

/// The tuple counterpart, including a nested tuple so the leaf walk
/// has to recurse on both sides of the call.
#[test]
fn a_tuple_returning_call_is_an_argument() {
    let src = r#"
        fn pair(a: i64) -> (i64, i64) { (a, a + 1i64) }
        fn mixed(a: i64) -> (i64, bool, u64) { (a, true, 2u64) }

        fn sum(t: (i64, i64)) -> i64 { t.0 + t.1 }

        fn weigh(t: (i64, bool, u64)) -> i64 {
            if t.1 { t.0 * (t.2 as i64) } else { 0i64 }
        }

        fn main() -> i64 {
            sum(pair(3i64)) + weigh(mixed(4i64))
        }
    "#;
    // (3 + 4) + (4 * 2). A nested tuple return (`((i64, i64), bool)`)
    // would be a different gap — the lowering has no return type for
    // it yet — so the mixed-width flat tuple is what exercises the
    // leaf walk here.
    assert_eq!(interpreter_value(src) & 0xff, 15);
    assert_consistent(src, "compound_arg_tuple_call");
}

/// Nested: the argument of the inner call is itself a
/// compound-returning call, and the outer one takes the result. Each
/// slot allocates its own locals, so the two must not share.
#[test]
fn compound_returning_calls_nest_in_argument_position() {
    let src = r#"
        struct P { x: i64, y: i64 }

        fn mk(a: i64) -> P { P { x: a, y: a + 1i64 } }
        fn shift(p: P, by: i64) -> P { P { x: p.x + by, y: p.y + by } }
        fn take(p: P) -> i64 { p.x + p.y }

        fn main() -> i64 {
            take(shift(mk(1i64), 10i64))
        }
    "#;
    // mk(1) is (1, 2); shifted by 10 that is (11, 12).
    assert_eq!(interpreter_value(src) & 0xff, 23);
    assert_consistent(src, "compound_arg_nested_call");
}

/// A generic constructor in argument position: nothing in
/// `Vec::new()` says what `T` is, so the parameter slot is the only
/// thing that can pick the instantiation.
#[test]
fn a_generic_constructor_takes_its_instance_from_the_slot() {
    let src = r#"
        fn count(v: Vec<u64>) -> u64 { v.size() }

        fn main() -> u64 {
            count(Vec::new()) + 7u64
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 7);
    assert_consistent(src, "compound_arg_generic_ctor");
}

/// The value the call produced is owned by the callee, exactly as a
/// `val`-bound one passed by value is. Rather than assert a count —
/// which would pin whatever the by-value move rule happens to do
/// today — this compares the two spellings: writing the call inline
/// must free neither more nor less often than binding it first.
#[test]
fn a_call_argument_frees_the_same_as_a_binding() {
    let src = r#"
        struct Owned { p: ptr }

        impl Owned {
            fn make() -> Owned { Owned { p: __builtin_heap_alloc(32u64) } }
        }

        impl Drop for Owned {
            fn drop(&mut self) { __builtin_heap_free(self.p) }
        }

        fn consume(o: Owned) -> u64 { 1u64 }

        fn main() -> u64 {
            val a0 = __builtin_free_count()
            val v = Owned::make()
            consume(v)
            val via_binding = __builtin_free_count() - a0

            val a1 = __builtin_free_count()
            consume(Owned::make())
            val inline = __builtin_free_count() - a1

            if via_binding == inline { 1u64 } else { 0u64 }
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 1);
    assert_consistent(src, "compound_arg_call_drop_parity");
}
