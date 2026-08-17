# Stdlib iterator adapters on `StringIter` (STDLIB-ITER-ADAPT): `map` /
# `filter` / `enumerate` / `collect` in `core/std/string.t`. The
# iterator yields one `u8` per byte; the adapters follow the same
# design as the `VecIter` ones — ordinary structs with
# `fn next(&mut self) -> Option<T>`, `collect` taking the iterator by
# value.
#
# Run: cargo run -q -p interpreter -- example/std_iter_adapt_string.t
# Expected exit code: 812 (52 map + 2 filter + 10 enumerate +
# 532 map-collect + 216 filter-collect)

fn main() -> u64 {
    val s = String::from_str("hello")

    # map: byte value minus 96 (so 'h' -> 8 etc.)
    var it = s.iter()
    var m = it.map(fn(b: u8) -> u64 { (b as u64) - 96u64 })
    var t1: u64 = 0u64
    for x in m { t1 = t1 + x }

    # filter: vowels only (e=101, o=111)
    var it2 = s.iter()
    var f = it2.filter(fn(b: u8) -> bool { b == 101u8 || b == 111u8 })
    var t2: u64 = 0u64
    for b in f { t2 = t2 + 1u64 }

    # enumerate
    var it3 = s.iter()
    var e = it3.enumerate()
    var t3: u64 = 0u64
    for kv in e { t3 = t3 + kv.0 }

    # collect (map)
    var it4 = s.iter()
    var m2 = it4.map(fn(b: u8) -> u8 { b })
    var c = m2.collect()
    var t4: u64 = 0u64
    var i: u64 = 0u64
    while i < c.size() {
        t4 = t4 + (c.get(i) as u64)
        i = i + 1u64
    }

    # collect (filter): both 'l's
    var it5 = s.iter()
    var f2 = it5.filter(fn(b: u8) -> bool { b == 108u8 })
    var c2 = f2.collect()
    var t5: u64 = 0u64
    var j: u64 = 0u64
    while j < c2.size() {
        t5 = t5 + (c2.get(j) as u64)
        j = j + 1u64
    }

    t1 + t2 + t3 + t4 + t5
}