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

/// `assert_consistent` plus the value itself: the lanes agreeing on a
/// wrong answer would pass the first alone.
fn assert_value(source: &str, stem: &str, expected: u64) {
    assert_consistent(source, stem);
    if skip_e2e() {
        return;
    }
    assert_eq!(interpreter_value(source), expected, "{stem}: value changed");
}

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

// RANGE-TYPE-ANNOTATION. `Range<u64>` read as a generic struct named a
// different type from the one a range literal has, so a parameter or
// return type written that way refused `0u64..3u64` with "expected
// Range<u64>, but got Range<u64>", and no range crossed a function
// boundary on any lane. The compiled lanes pass it as its two bounds.
#[test]
fn a_range_crosses_a_function_boundary() {
    let src = r#"
struct Win { lo: u64 }
impl Win {
    fn span(&self, r: Range<u64>) -> u64 {
        var s: u64 = 0u64
        for i in r { s = s + i * self.lo }
        s
    }
    fn around(&self, w: u64) -> Range<u64> {
        (self.lo - w)..(self.lo + w)
    }
}
fn upto(n: u64) -> Range<u64> {
    0u64..n
}
fn narrow(r: Range<u8>) -> u64 {
    (r.end - r.start) as u64
}
fn main() -> u64 {
    val r: Range<u64> = upto(4u64)
    var t: u64 = 0u64
    for i in r { t = t + i }
    val w = Win { lo: 10u64 }
    val a = w.around(2u64)
    val b = w.span(a)
    println("{r} {a}")
    t + b + narrow(3u8..9u8) + r.end * 1000u64
}
"#;
    assert_consistent(src, "range_crosses_boundary");
}

/// Every way a function hands a range back: both arms of an `if`
/// (they meet in one pair of return slots), an early `return`, a
/// parameter passed straight through, a signed element type, and a
/// call iterated directly.
#[test]
fn a_range_comes_back_from_every_kind_of_return() {
    let src = r#"
fn pick(c: bool, n: u64) -> Range<u64> {
    if c { 0u64..n } else { n..(n * 2u64) }
}
fn early(n: u64) -> Range<u64> {
    if n == 0u64 {
        return 5u64..6u64
    }
    1u64..n
}
fn same(r: Range<u64>) -> Range<u64> {
    r
}
fn shifted(r: Range<i64>, by: i64) -> Range<i64> {
    val lo = r.start + by
    val hi = r.end + by
    lo..hi
}
fn sum(r: Range<u64>) -> u64 {
    var s: u64 = 0u64
    for i in r { s = s + i }
    s
}
fn main() -> u64 {
    val a = pick(true, 3u64)
    val b = pick(false, 3u64)
    val c = early(0u64)
    val d = early(4u64)
    val e = same(a)
    val f = shifted(-2i64..1i64, 10i64)
    var g: u64 = 0u64
    for i in early(3u64) { g = g + i }
    val h = sum(same(b))
    sum(a) + sum(b) * 10u64 + sum(c) * 100u64 + sum(d) * 1000u64 + sum(e) * 10000u64
        + ((f.start + f.end) as u64) * 100000u64 + g * 1000000u64 + h * 10000000u64
}
"#;
    assert_value(src, "range_every_return", 124_936_623u64);
}

/// A generic function over `Range<T>`: the element type is inferred
/// from the argument and the pair is lowered at the substituted width.
#[test]
fn a_generic_function_takes_a_range() {
    let src = r#"
fn width<T>(r: Range<T>) -> T {
    r.end - r.start
}
fn main() -> u64 {
    val a: u64 = width(2u64..9u64)
    val b: i32 = width(-3i32..4i32)
    a + (b as u64)
}
"#;
    assert_value(src, "range_generic_param", 14u64);
}
