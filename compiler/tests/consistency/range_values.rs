//! RANGE-FOR: a range **value** in every lane.
//!
//! `val r = a..b` used to exist only in the tree-walker, and even there
//! nothing could consume one: `for i in r` stopped at "method `next`
//! not found", because the parser reads a bare name after `in` as an
//! iterator. The checker now rewrites that loop into
//! `for i in r.start..r.end` once it knows `r` is a range, and the
//! compiled lanes hold a range as its two bounds. Iterating does not
//! consume the range: the bounds are read when the loop starts.

use super::harness::*;

/// Iterating twice, reading the bounds, printing and interpolating a
/// range, a signed range with `continue`, and a parenthesised range
/// literal after `in` (which takes the same desugar through a
/// synthetic `var`).
#[test]
fn a_range_value_iterates_and_prints() {
    let src = r#"
fn main() -> u64 {
    val r = 2u64..6u64
    var t: u64 = 0u64
    for i in r { t = t + i }
    for i in r { t = t + i }
    println("{r.start} {r.end} {t}")
    println(r)
    var s: i64 = 0i64
    val q = -3i64..2i64
    for i in q {
        if i == 0i64 { continue }
        s = s + i
    }
    for i in (1u64..4u64) { t = t + i }
    println("{s} {t}")
    t
}
"#;
    assert_consistent(src, "range_value_iterates");
}

/// The shapes around the loop: a labelled `break` out of two loops
/// over the same range, empty and reversed ranges, a `var` range
/// reassigned before and *inside* its own loop (the loop keeps the
/// bounds it started with), a copy that does not follow the original,
/// a shadowing name in the body, and an ordinary iterator still taking
/// the protocol path.
#[test]
fn a_range_value_around_its_loop() {
    let src = r#"
struct Counter { n: u64, limit: u64 }

impl Counter {
    fn next(&mut self) -> Option<u64> {
        if self.n >= self.limit { return Option::None }
        self.n = self.n + 1u64
        Option::Some(self.n)
    }
}

fn main() -> u64 {
    var acc: u64 = 0u64
    val r = 0u64..10u64
    # labelled break out of a nested loop over the same range
    @outer: for i in r {
        for j in r {
            if i * j > 20u64 { break @outer }
            if j % 2u64 == 0u64 { continue }
            acc = acc + j
        }
    }
    println("nested {acc}")

    # empty and reversed ranges run zero times
    val e = 5u64..5u64
    val back = 7u64..3u64
    var hits: u64 = 0u64
    for i in e { hits = hits + 1u64 }
    for i in back { hits = hits + 1u64 }
    println("empty {hits}")

    # reassign a var range, copy one, and reassign inside the loop
    var w = 1u64..3u64
    val copy = w
    w = (10u64..12u64)
    var sum: u64 = 0u64
    for i in w {
        w = (100u64..200u64)
        sum = sum + i
    }
    for i in copy { sum = sum + i }
    println("reassigned {sum} {w} {copy}")

    # shadowing: the inner name is not a range
    val s = 2u64..4u64
    var t: u64 = 0u64
    for i in s {
        val s = 1000u64
        t = t + i + s
    }
    println("shadow {t}")

    # an ordinary iterator still takes the protocol path
    var c = Counter { n: 0u64, limit: 3u64 }
    var it_sum: u64 = 0u64
    for v in c { it_sum = it_sum + v }
    println("iter {it_sum}")

    acc + hits + sum + t + it_sum
}
"#;
    assert_consistent(src, "range_value_around_loop");
}

/// Narrow widths, as a `for` header already allowed. A range value
/// used to be `i64` / `u64` only, and `0u8..3u8` was refused as "not
/// matching integer types, got u8..u8".
#[test]
fn a_narrow_range_value() {
    let src = r#"
fn main() -> u64 {
    val r = 250u8..255u8
    var t: u64 = 0u64
    for i in r { t = t + (i as u64) }
    val n = -2i32..2i32
    var s: i64 = 0i64
    for i in n { s = s + (i as i64) }
    println("{r} {n} {s}")
    t
}
"#;
    assert_consistent(src, "range_value_narrow");
}
