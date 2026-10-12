//! STDLIB-ITER-TERMINAL: the terminal operations every iterator inherits
//! from `trait Iterator<T>` (`count` / `last` / `nth` / `fold` / `any` /
//! `all` / `position` / `find`), and the bounded ones an iterator opts
//! into (`OrdIterator::min` / `max`, `SumIterator::sum`).
//!
//! They are default bodies of a *generic* trait, which is what took
//! frontend work: the inherited signature is restated in each impl's
//! terms (`MapIter<T, U>` yields `U`, not `T`), a method's own type
//! parameter (`fold<Acc>`) is parsed instead of skipped, and an empty
//! `impl<T: Ord> OrdIterator<T> for VecIter<T> {}` carries its bound
//! into the body it inherits.

use super::harness::*;

/// Every operation over a `Vec<u64>`, through the base iterator.
#[test]
fn terminal_operations_over_a_vec() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(2u64)
            v.push(9u64)
            v.push(2u64)
            val a = v.iter()
            println(a.count())
            val b = v.iter()
            val l = b.last()
            println(l ?? 0u64)
            val c = v.iter()
            val n2 = c.nth(2u64)
            println(n2 ?? 0u64)
            val d = v.iter()
            val n9 = d.nth(9u64)
            println(n9 ?? 77u64)
            val e = v.iter()
            println(e.fold(100u64, fn(acc: u64, x: u64) -> u64 { acc - x }))
            val f = v.iter()
            println(f.any(fn(x: u64) -> bool { x > 8u64 }))
            val g = v.iter()
            println(g.all(fn(x: u64) -> bool { x > 2u64 }))
            val h = v.iter()
            val pos = h.position(fn(x: u64) -> bool { x == 2u64 })
            println(pos ?? 99u64)
            val i = v.iter()
            val fd = i.find(fn(x: u64) -> bool { x > 5u64 })
            println(fd ?? 0u64)
            val j = v.iter()
            val mn = j.min()
            println(mn ?? 0u64)
            val k = v.iter()
            val mx = k.max()
            println(mx ?? 0u64)
            val m = v.iter()
            val s: u64 = m.sum()
            println(s)
            0u64
        }
    "#;
    assert_renders(src, "iter_terminal_vec", "4\n2\n9\n77\n82\ntrue\nfalse\n1\n9\n2\n9\n18\n");
}

/// An adapter's element type is its own (`MapIter<T, U>` yields `U`):
/// the inherited signatures have to say so, or `min` on a map from
/// `u64` to `i64` would compare `u64`s.
#[test]
fn terminal_operations_through_adapters() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(2u64)
            v.push(9u64)
            val a = v.iter()
            val m = a.map(fn(x: u64) -> i64 { 3i64 - (x as i64) })
            val mn = m.min()
            println(mn ?? 0i64)
            val b = v.iter()
            val f = b.filter(fn(x: u64) -> bool { x != 2u64 })
            val s: u64 = f.sum()
            println(s)
            val c = v.iter()
            val m2 = c.map(fn(x: u64) -> f64 { (x as f64) * 0.5f64 })
            val fs: f64 = m2.sum()
            println(fs)
            val d = v.iter()
            val m3 = d.map(fn(x: u64) -> u64 { x * 10u64 })
            println(m3.fold(0u64, fn(acc: u64, x: u64) -> u64 { acc + x }))
            0u64
        }
    "#;
    assert_renders(src, "iter_terminal_adapters", "-6\n14\n8.0\n160\n");
}

/// The empty stream: `sum` is the zero, `min` / `max` / `last` are
/// `None`, `all` is vacuously true and `any` false.
#[test]
fn terminal_operations_over_nothing() {
    let src = r#"
        fn main() -> u64 {
            val e: Vec<f64> = Vec::new()
            val a = e.iter()
            val s: f64 = a.sum()
            println(s)
            val b = e.iter()
            val mx = b.max()
            println(mx ?? -1f64)
            val c = e.iter()
            println(c.all(fn(x: f64) -> bool { x > 0f64 }))
            val d = e.iter()
            println(d.any(fn(x: f64) -> bool { x > 0f64 }))
            val f = e.iter()
            val l = f.last()
            println(l ?? -2f64)
            val g = e.iter()
            println(g.count())
            0u64
        }
    "#;
    assert_renders(src, "iter_terminal_empty", "0.0\n-1.0\ntrue\nfalse\n-2.0\n0\n");
}

/// The string iterators are concrete impls (`Iterator<u8>`,
/// `Iterator<char>`), so the substituted signature is a primitive.
#[test]
fn terminal_operations_over_a_string() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("hello")
            val a = s.iter()
            val mx = a.max()
            println(mx ?? 0u8)
            val b = s.chars()
            println(b.count())
            val c = s.iter()
            val p = c.position(fn(x: u8) -> bool { x == 'l' })
            println(p ?? 99u64)
            val d = s.iter()
            val total: u8 = d.sum()
            println(total)
            0u64
        }
    "#;
    // 104 + 101 + 108 + 108 + 111 = 532, which wraps to 20 in a `u8`
    // the way `+` does.
    assert_renders(src, "iter_terminal_string", "111\n5\n2\n20\n");
}

/// A user iterator gets the defaults by naming the trait, and opts into
/// `min` / `sum` with empty impls. The `SumIterator` one is checked
/// before `core/std/default.t`'s `impl Default for u64` (user statements
/// come first): conformance is recorded in the signature pre-pass now,
/// so `u64: Default` already holds when the inherited body asks.
#[test]
fn a_user_iterator_inherits_the_terminal_operations() {
    let src = r#"
        struct Countdown { n: u64 }
        impl Iterator<u64> for Countdown {
            fn next(&mut self) -> Option<u64> {
                if self.n == 0u64 {
                    Option::None
                } else {
                    self.n = self.n - 1u64
                    Option::Some(self.n)
                }
            }
        }
        impl OrdIterator<u64> for Countdown {}
        impl SumIterator<u64> for Countdown {}

        fn main() -> u64 {
            val a = Countdown { n: 4u64 }
            val b = Countdown { n: 4u64 }
            val c = Countdown { n: 4u64 }
            val s: u64 = b.sum()
            val m = c.min()
            a.count() * 100u64 + s * 10u64 + (m ?? 9u64)
        }
    "#;
    assert_consistent(src, "iter_terminal_user");
}

/// The operations consume a copy: the caller's binding keeps the
/// position it had.
#[test]
fn a_terminal_operation_leaves_the_binding_where_it_was() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(2u64)
            v.push(9u64)
            var it = v.iter()
            val first = it.next()
            val a = it.count()
            val b = it.count()
            (first ?? 0u64) * 100u64 + a * 10u64 + b
        }
    "#;
    assert_consistent(src, "iter_terminal_position");
}

/// An element the operation hands back is the container's (the same
/// alias `next` returns), so binding it must not free it: no lane frees
/// a `String` twice, and nothing leaks.
#[test]
fn an_element_returned_by_a_terminal_operation_is_freed_once() {
    let src = r#"
        fn main() -> u64 {
            var s: Vec<String> = Vec::new()
            s.push(String::from_str("pear"))
            s.push(String::from_str("apple"))
            s.push(String::from_str("fig"))
            val a = s.iter()
            val mn = a.min()
            match mn {
                Option::Some(x) => println(x),
                Option::None => println("none"),
            }
            val b = s.iter()
            val mx = b.max()
            match mx {
                Option::Some(x) => println(x),
                Option::None => println("none"),
            }
            val c = s.iter()
            val l = c.last()
            match l {
                Option::Some(x) => println(x),
                Option::None => println("none"),
            }
            val d = s.iter()
            val n = d.nth(1u64)
            match n {
                Option::Some(x) => println(x),
                Option::None => println("none"),
            }
            println(s.size())
            0u64
        }
    "#;
    assert_renders(src, "iter_terminal_owned", "apple\npear\nfig\napple\n3\n");
    if skip_e2e() {
        return;
    }
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    let (code, stderr) = compiled_heap_poison_run(src, "iter_terminal_owned_poison");
    assert_eq!(code, 0, "instrumented binary stopped:\n{stderr}");
}

/// IR VM: a compound result written into a local whose address is taken
/// later in the function has to land in the local's heap cell. It went
/// to the raw slot, so `x.lt(b)` -- which borrows the `match` payload
/// `b` of `var best = it.next()` -- read the value from before the call,
/// and `min` answered 0 on the VM alone.
#[test]
fn a_compound_result_reaches_an_address_taken_local() {
    let src = r#"
        fn smallest(v: &Vec<u64>) -> Option<u64> {
            var it = v.iter()
            var best = it.next()
            for x in it {
                match best {
                    Option::Some(b) => {
                        if x.lt(b) {
                            best = Option::Some(x)
                        }
                    }
                    Option::None => {}
                }
            }
            best
        }
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(5u64)
            v.push(2u64)
            v.push(9u64)
            val m = smallest(&v)
            m ?? 99u64
        }
    "#;
    assert_consistent(src, "iter_terminal_vm_cell");
}
