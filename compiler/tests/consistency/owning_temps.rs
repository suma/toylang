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
