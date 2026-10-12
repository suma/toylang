# Iterator terminal operations and `Ordering` (STDLIB-ITER-TERMINAL /
# STDLIB-ORD-CMP).
#
# Every iterator inherits `count` / `last` / `nth` / `fold` / `any` /
# `all` / `position` / `find` from `trait Iterator<T>`; the ones whose
# element type allows it also have `min` / `max` (`OrdIterator`) and
# `sum` (`SumIterator`). Each consumes a copy of the iterator, so the
# binding keeps its place.
#
# The compiled lanes want every intermediate bound with `val` (a method
# call on a compound temporary is not lowered yet), hence the one
# binding per step below.

struct Score {
    points: u64,
    id: u64,
}

# Writing `lt` is enough: `cmp`, `le`, `gt`, `ge` (and so `<=`, `>`,
# `>=`) come from the trait.
impl Ord for Score {
    fn lt(&self, other: &Self) -> bool {
        self.points < other.points
    }
}

# A user iterator: name the trait and the terminal operations come
# with it; opt into `min` / `max` / `sum` with empty impls.
struct Countdown {
    n: u64,
}

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
    var v: Vec<u64> = Vec::new()
    v.push(5u64)
    v.push(2u64)
    v.push(9u64)
    v.push(4u64)

    val a = v.iter()
    val total: u64 = a.sum()
    val b = v.iter()
    val smallest = b.min() ?? 0u64
    val c = v.iter()
    val largest = c.max() ?? 0u64
    val d = v.iter()
    val big = d.any(fn(x: u64) -> bool { x > 8u64 })
    val e = v.iter()
    val at = e.position(fn(x: u64) -> bool { x == 9u64 }) ?? 99u64
    val f = v.iter()
    val evens = f.filter(fn(x: u64) -> bool { x % 2u64 == 0u64 })
    val even_count = evens.count()
    val g = v.iter()
    val squares = g.map(fn(x: u64) -> u64 { x * x })
    val sum_sq = squares.fold(0u64, fn(acc: u64, x: u64) -> u64 { acc + x })
    println("sum={total} min={smallest} max={largest} any>8={big} at={at}")
    println("evens={even_count} sum of squares={sum_sq}")

    # Ordering.
    val o = 3u64.cmp(7u64)
    println("3 vs 7: {o}")
    val p = Score { points: 10u64, id: 1u64 }
    val q = Score { points: 12u64, id: 2u64 }
    println("p <= q: {p <= q}, p > q: {p > q}")
    val top = max(p, q)
    println("max id: {top.id}")
    println("clamp: {clamp(15i64, 0i64, 10i64)}")

    # A sorted vector answers `binary_search` in O(log n).
    v.sort()
    val hit = v.binary_search(&5u64)
    val miss = v.binary_search(&6u64)
    val hit_at = match hit {
        Result::Ok(i) => i,
        Result::Err(_) => 99u64,
    }
    val miss_at = match miss {
        Result::Ok(_) => 99u64,
        Result::Err(i) => i,
    }
    println("5 found at {hit_at}, 6 would go at {miss_at}")

    val cd = Countdown { n: 5u64 }
    val cd_sum: u64 = cd.sum()
    println("countdown sum={cd_sum}")

    total + cd_sum
}
