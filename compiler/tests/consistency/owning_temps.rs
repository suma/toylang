//! OWNING-TEMP-DROP: a call read through without a binding --
//! `mk().n`, `String::from_str("a").len()` -- is dropped at the end of
//! the read, on every lane. Before, nobody owned it, so its `Drop`
//! never ran and its buffer leaked (the tree-walker included, which is
//! why the lanes agreed on the leak).
//!
//! `bind_owning_temporaries` rewrites the read to `{ val __temp = call
//!  __temp.read }`, so the ordinary ownership rules decide the rest:
//! a consuming method moves the temporary, a `&mut self` one mutates
//! it before it drops.

use super::harness::*;

const OWNER: &str = r#"
    struct H { n: u64, name: String }
    impl Drop for H { fn drop(&mut self) { println("drop {self.n}") } }
    impl H {
        fn get(&self) -> u64 { self.n }
        fn finish(self: Self) -> u64 { self.n * 10u64 }
        fn bump(&mut self) { self.n = self.n + 1u64 }
    }
    fn mk(n: u64) -> H { H { n: n, name: String::from_str("h") } }
"#;

/// Each temporary drops once, right after the read that used it.
#[test]
fn a_temporary_that_owns_drops_after_the_read() {
    let src = format!(
        r#"{OWNER}
        fn main() -> u64 {{
            val a = mk(1u64).n
            println("read a")
            val b = mk(2u64).get()
            println("read b")
            val c = mk(3u64).finish()
            println("read c")
            mk(4u64).bump()
            println("bumped")
            val d = mk(6u64).name.len()
            if mk(7u64).get() > 3u64 {{ println("big") }}
            val r = mk(8u64).get()
            println("{{a}} {{b}} {{c}} {{d}} {{r}}")
            a + b + c + d + r
        }}
    "#
    );
    // `finish` consumes the temporary, so its own body is what drops
    // it ("drop 3"); `bump` runs before the drop, so 4 drops as 5.
    assert_renders(
        &src,
        "owning_temps_drop_order",
        "drop 1\nread a\ndrop 2\nread b\ndrop 3\nread c\ndrop 5\nbumped\n\
         drop 6\ndrop 7\nbig\ndrop 8\n1 2 30 1 8\n",
    );
}

/// `get` hands back the element the container keeps; dropping it as a
/// temporary would free the container's own buffer. Reading it twice
/// is what would show that.
#[test]
fn an_element_read_through_get_is_not_dropped() {
    let src = r#"
        fn main() -> u64 {
            var w: Vec<String> = Vec::new()
            w.push(String::from_str("hello"))
            val first = w.get(0u64).len()
            val again = w.get(0u64).len()
            println("{first} {again}")
            first + again
        }
    "#;
    assert_stdout_consistent(src, "owning_temps_get_is_borrowed");
}

/// What the temporaries allocated is freed: the live total comes back
/// to where it was.
#[test]
fn a_dropped_temporary_frees_its_buffer() {
    let src = r#"
        fn main() -> u64 {
            val before = __builtin_live_bytes()
            val n = String::from_str("abcdef").len()
            val m = String::from_str("xyz").len() + String::from_str("w").len()
            val after = __builtin_live_bytes()
            println("{n} {m} {after - before}")
            n + m
        }
    "#;
    assert_renders(src, "owning_temps_frees", "6 4 0\n");
}

/// A read whose result owns something of its own: the call is bound by
/// a statement in front, and drops with the enclosing block. Before,
/// the compiled lanes refused `mk().renamed()` and the temporary leaked
/// on the tree-walker.
#[test]
fn a_temporary_under_an_owning_result_drops_with_the_block() {
    let src = format!(
        r#"{OWNER}
        impl H {{
            fn renamed(&self) -> H {{ H {{ n: self.n + 100u64, name: String::from_str("r") }} }}
        }}
        fn work() -> u64 {{
            val r = mk(1u64).renamed()
            println("bound {{r.n}}")
            var t = mk(2u64).renamed().renamed()
            println("t {{t.n}}")
            val k = mk(3u64).renamed().name.len()
            val tail = String::from_str("r")
            val c = String::from_str("ab").concat(tail)
            println(c)
            r.n + t.n + k
        }}
        fn main() -> u64 {{
            val before = __builtin_live_bytes()
            val n = work()
            println("{{n}} {{__builtin_live_bytes() - before}}")
            n
        }}
    "#
    );
    assert_renders(
        &src,
        "owning_temps_owning_result",
        "bound 101\nt 202\ndrop 103\ndrop 3\nabr\ndrop 202\ndrop 102\ndrop 2\ndrop 101\ndrop 1\n304 0\n",
    );
}

/// A fresh owning call passed as an argument is bound in front of the
/// statement, so a callee that only lends it (`&H`, a by-value
/// parameter it reads, `concat(&String)`) leaves it to the caller's
/// block, and one that keeps it (`push`) takes it as a move. Before,
/// a lent temporary had no owner and leaked on every lane. An argument
/// behind something that runs first (`note(..)`) stays where it is, so
/// the order of effects never changes.
#[test]
fn a_temporary_passed_as_an_argument_drops_with_the_block() {
    let src = r#"
        struct H { n: u64 }
        impl Drop for H { fn drop(&mut self) { println("drop {self.n}") } }
        fn mk(n: u64) -> H {
            println("mk {n}")
            H { n: n }
        }
        fn note(n: u64) -> u64 {
            println("note {n}")
            n
        }
        fn peek(h: &H) -> u64 { h.n }
        fn look(h: H) -> u64 { h.n }
        fn both(a: u64, h: &H) -> u64 { a + h.n }
        fn work() -> u64 {
            val s = String::from_str("ab")
            val c = s.concat(String::from_str("cd"))
            println(c)
            val a = peek(&mk(1u64))
            val b = look(mk(2u64))
            var v: Vec<H> = Vec::new()
            v.push(mk(3u64))
            for i in 4u64..6u64 {
                v.push(mk(i))
            }
            val d = both(note(9u64), &mk(6u64))
            println("work {a} {b} {d} {v.size()}")
            a + b
        }
        fn main() -> u64 {
            val before = __builtin_live_bytes()
            val n = work()
            println("{n} {__builtin_live_bytes() - before}")
            n
        }
    "#;
    assert_renders(
        src,
        "owning_temps_argument",
        "abcd\nmk 1\nmk 2\nmk 3\nmk 4\nmk 5\nnote 9\nmk 6\nwork 1 2 15 3\n\
         drop 3\ndrop 4\ndrop 5\ndrop 2\ndrop 1\n3 0\n",
    );
}
