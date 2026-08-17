# Stdlib iterator adapters (STDLIB-ITER-ADAPT): `map` / `filter` /
# `enumerate` / `zip` / `collect` on a `VecIter<T>`, defined in
# `core/std/collections/vec.t`. Each adapter is an ordinary struct
# exposing `fn next(&mut self) -> Option<T>`, so the parser's for-loop
# desugaring treats it like any other iterator. `collect` consumes the
# iterator by value and builds a `Vec`; the caller's iterator keeps its
# state (compound alias semantics).
#
# Run: cargo run -q -p interpreter -- example/std_iter_adapt.t
# Expected exit code: 750 (150 map + 6 filter + 515 map+collect +
# 40 enumerate + 30 zip + 9 filter-collect)

fn main() -> u64 {
    var v: Vec<u64> = Vec::new()
    v.push(1u64)
    v.push(2u64)
    v.push(3u64)
    v.push(4u64)
    v.push(5u64)

    # map: x * 10
    var it = v.iter()
    var m = it.map(fn(x: u64) -> u64 { x * 10u64 })
    var t1: u64 = 0u64
    for x in m { t1 = t1 + x }

    # filter: even elements only
    var it2 = v.iter()
    var f = it2.filter(fn(x: u64) -> bool { x % 2u64 == 0u64 })
    var t2: u64 = 0u64
    for x in f { t2 = t2 + x }

    # collect: map + collect into a Vec, then sum it back out
    var it3 = v.iter()
    var m2 = it3.map(fn(x: u64) -> u64 { x + 100u64 })
    var c1 = m2.collect()
    var t3: u64 = 0u64
    var i3: u64 = 0u64
    while i3 < c1.size() {
        t3 = t3 + c1.get(i3)
        i3 = i3 + 1u64
    }

    # enumerate: (index, value) pairs
    var it4 = v.iter()
    var e = it4.enumerate()
    var t4: u64 = 0u64
    for kv in e { t4 = t4 + kv.0 * kv.1 }

    # zip: pairs of elements from two iterators
    var it5 = v.iter()
    var it6 = v.iter()
    var z = it5.zip(it6)
    var t5: u64 = 0u64
    for p in z { t5 = t5 + p.0 + p.1 }

    # filter + collect
    var it9 = v.iter()
    var f2 = it9.filter(fn(x: u64) -> bool { x % 2u64 == 1u64 })
    var c3 = f2.collect()
    var t7: u64 = 0u64
    var i7: u64 = 0u64
    while i7 < c3.size() {
        t7 = t7 + c3.get(i7)
        i7 = i7 + 1u64
    }

    # 150 (map) + 6 (filter even) + 515 (map+collect) + 40 (enumerate)
    # + 30 (zip) + 9 (filter odd collect) = 750
    t1 + t2 + t3 + t4 + t5 + t7
}