//! A generic parameter with no bound, and the module-qualified call.
//!
//! `fn shuffle<T>(v: &mut Vec<T>)` is the first stdlib free function
//! that is generic, and writing it turned up three separate holes --
//! one in each layer -- that only a parameter with **no** bound and a
//! **qualified** call reach:
//!
//! 1. the type checker treated "declared with a bound" as the whole of
//!    "declared", so `T` read as a name nobody introduced;
//! 2. the qualified-call path did not push the scope that
//!    `visit_generic_call` pops, so the *caller's* bindings went with
//!    it and every later mention of the argument was
//!    `[E0003] Identifier not found`;
//! 3. lowering looked a qualified callee up in the function index by
//!    name, where a generic template never appears -- it is minted per
//!    instantiation -- so the call was rejected as unsupported.

use super::harness::*;

#[test]
fn an_unbounded_parameter_is_in_scope_in_its_own_body() {
    // `v.get(i)` returns `T`. Nothing bounds `T`, and nothing needs
    // to: the value is only moved to another slot of the same vector.
    let src = r#"
        fn swap_ends<T>(v: &mut Vec<T>) {
            if v.size() < 2u64 { return }
            val first: T = v.get(0u64)
            val last: T = v.get(v.size() - 1u64)
            v.set(0u64, last)
            v.set(v.size() - 1u64, first)
        }

        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            swap_ends(&mut v)
            println(v.get(0u64))
            println(v.get(2u64))
            var s: Vec<str> = Vec::new()
            s.push("a")
            s.push("b")
            swap_ends(&mut s)
            println(s.get(0u64))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "unbounded_generic_param");
}

#[test]
fn a_qualified_generic_call_leaves_the_callers_bindings_alone() {
    // The argument is mentioned again *after* the call: that is the
    // whole test. The scope the qualified path failed to push was the
    // caller's, so `v` stopped existing at the next line.
    let src = r#"
        fn main() -> u64 {
            io::random_seed(3u64)
            var v: Vec<u64> = Vec::new()
            v.push(10u64)
            v.push(20u64)
            v.push(30u64)
            random::shuffle(&mut v)
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < v.size() {
                total = total + v.get(i)
                i = i + 1u64
            }
            total
        }
    "#;
    // 60, and the same 60 on every backend.
    assert_consistent(src, "qualified_generic_call_scope");
}
