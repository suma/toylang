//! STDLIB-ORD-CMP: `Ordering`, the methods `Ord` derives from `lt`
//! (`cmp` / `le` / `gt` / `ge`), the generic `min` / `max` / `clamp`,
//! and `Vec::binary_search`.

use super::harness::*;

/// `cmp` on the primitives and on `str` (which overrides it with one
/// extern call), and `Ordering`'s own methods.
#[test]
fn ordering_from_cmp() {
    let src = r#"
        fn main() -> u64 {
            val a = 3u64.cmp(5u64)
            println(a)
            val b = "b".cmp("a")
            println(b)
            val c = 7i64.cmp(7i64)
            println(c)
            println(c == Ordering::Equal)
            println(a != Ordering::Less)
            val r = a.reverse()
            println(r)
            val t = c.then(Ordering::Greater)
            println(t)
            println(t as i64)
            println(a.is_lt())
            println(b.is_le())
            0u64
        }
    "#;
    assert_renders(
        src,
        "ord_cmp_ordering",
        "Less\nGreater\nEqual\ntrue\nfalse\nGreater\nGreater\n1\ntrue\nfalse\n",
    );
}

/// A struct that writes only `lt` gets `cmp` and the `<=` / `>` / `>=`
/// operators from the defaults.
#[test]
fn a_struct_with_lt_gets_the_rest() {
    let src = r#"
        struct Pt { x: i64, y: i64 }
        impl Ord for Pt {
            fn lt(&self, other: &Self) -> bool {
                if self.x != other.x { self.x < other.x } else { self.y < other.y }
            }
        }
        fn main() -> u64 {
            val p = Pt { x: 1i64, y: 2i64 }
            val q = Pt { x: 1i64, y: 3i64 }
            val o = p.cmp(q)
            var bits = 0u64
            if p <= q { bits = bits + 1u64 }
            if p > q { bits = bits + 2u64 }
            if p >= p { bits = bits + 4u64 }
            if q.gt(p) { bits = bits + 8u64 }
            if o == Ordering::Less { bits = bits + 16u64 }
            bits
        }
    "#;
    assert_consistent(src, "ord_cmp_struct");
}

/// Generic `min` / `max` / `clamp`, including Rust's tie rule (`min`
/// keeps the first argument, `max` the second).
#[test]
fn generic_min_max_clamp() {
    let src = r#"
        struct K { k: u64, tag: u64 }
        impl Ord for K {
            fn lt(&self, other: &Self) -> bool { self.k < other.k }
        }
        fn main() -> u64 {
            val a = min(4u64, 2u64)
            val b = max(4i64, -2i64)
            val c = clamp(9u8, 1u8, 5u8)
            val d = clamp(-3i64, 0i64, 10i64)
            # Bound first: the compiled lanes infer `T` from a named
            # argument, not from a struct literal in the argument list.
            val k1 = K { k: 1u64, tag: 1u64 }
            val k2 = K { k: 1u64, tag: 2u64 }
            val lo = min(k1, k2)
            val k3 = K { k: 1u64, tag: 1u64 }
            val k4 = K { k: 1u64, tag: 2u64 }
            val hi = max(k3, k4)
            a * 10000u64 + (b as u64) * 1000u64 + (c as u64) * 100u64
                + (d as u64) * 10u64 + lo.tag * 3u64 + hi.tag
        }
    "#;
    assert_consistent(src, "ord_cmp_min_max");
}

/// `clamp` with its bounds the wrong way round stops rather than
/// picking one.
#[test]
fn clamp_refuses_reversed_bounds() {
    let src = r#"
        fn main() -> u64 {
            clamp(5u64, 9u64, 1u64)
        }
    "#;
    if skip_e2e() {
        return;
    }
    assert!(
        interpreter_error(src).contains("clamp: the low bound is above the high one"),
        "{}",
        interpreter_error(src)
    );
}

/// `binary_search` finds a present key (`Ok`) and the insertion point
/// of a missing one (`Err`), over primitives and over `String`.
#[test]
fn binary_search_reports_the_position() {
    let src = r#"
        fn show(r: Result<u64, u64>) -> str {
            match r {
                Result::Ok(i) => "Ok({i})",
                Result::Err(i) => "Err({i})",
            }
        }
        fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            v.push(1i64)
            v.push(3i64)
            v.push(5i64)
            v.push(7i64)
            val a = v.binary_search(5i64)
            println(show(a))
            val b = v.binary_search(0i64)
            println(show(b))
            val c = v.binary_search(8i64)
            println(show(c))
            val d = v.binary_search(4i64)
            println(show(d))
            var s: Vec<String> = Vec::new()
            s.push(String::from_str("apple"))
            s.push(String::from_str("fig"))
            s.push(String::from_str("pear"))
            val k = String::from_str("fig")
            val e = s.binary_search(k)
            println(show(e))
            val none: Vec<u64> = Vec::new()
            val f = none.binary_search(3u64)
            println(show(f))
            0u64
        }
    "#;
    assert_renders(src, "ord_cmp_binary_search", "Ok(2)\nErr(0)\nErr(4)\nErr(2)\nOk(1)\nErr(0)\n");
    if skip_e2e() {
        return;
    }
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
}

/// `Option::unwrap_or_default` -- what `SumIterator::sum` starts from.
#[test]
fn unwrap_or_default_supplies_the_zero() {
    let src = r#"
        fn main() -> u64 {
            val some: Option<u64> = Option::Some(7u64)
            val none: Option<u64> = Option::None
            some.unwrap_or_default() * 10u64 + none.unwrap_or_default()
        }
    "#;
    assert_consistent(src, "ord_cmp_unwrap_or_default");
}
