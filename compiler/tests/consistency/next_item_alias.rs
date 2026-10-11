//! NEXT-ITEM-ALIAS: what an iterator's `next` hands back is an element
//! the container keeps -- a shallow copy, the way `get` answers -- so
//! nothing that receives it may drop it.
//!
//! Before, three receivers did:
//!
//! - `val m = it.next()` took a drop on every lane, so the element's
//!   buffer was freed at the end of the scope and again by the `Vec`;
//! - `match it.next() { Some(s) => .. }` with an arm that runs to its
//!   end gave the arm a drop target over the payload (compiled lanes);
//! - `for s in v.iter()` freed each item on the tree-walker only, which
//!   then reported fewer live bytes than the compiled lanes.
//!
//! A heap block's second free is idempotent, so the first two printed
//! the right answer; the heap check is what shows them.

use super::harness::*;

const SRC: &str = r#"
    fn main() -> u64 {
        var w: Vec<String> = Vec::new()
        w.push(String::from_str("hello"))
        w.push(String::from_str("hi"))
        val before = __builtin_live_bytes()
        for s in w.iter() {
            println(s.len())
        }
        var it = w.iter()
        val m = it.next()
        match m {
            Option::Some(s) => println(s.len()),
            Option::None => println("none"),
        }
        match it.next() {
            Option::Some(s) => println(s.len()),
            Option::None => println("none"),
        }
        println(before - __builtin_live_bytes())
        println(w.get(0u64).len() + w.get(1u64).len())
        0u64
    }
"#;

/// The items are not freed by the loop, the binding or the arm: the
/// live total is unchanged and the elements still read.
#[test]
fn an_iterator_item_is_not_dropped() {
    assert_renders(SRC, "next_item_alias", "5\n2\n5\n2\n0\n7\n");
}

/// No lane frees an element twice.
#[test]
fn an_iterator_item_is_freed_once() {
    if skip_e2e() {
        return;
    }
    assert_eq!(tree_walker_heap_check_report(SRC), "heap check: 0 double frees (0 distinct)\n");
    let (code, stderr) = compiled_heap_poison_run(SRC, "next_item_alias_poison");
    assert_eq!(code, 0, "instrumented binary stopped:\n{stderr}");
}
