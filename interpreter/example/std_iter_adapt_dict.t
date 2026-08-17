# Stdlib iterator adapters on `DictIter<K, V>` (STDLIB-ITER-ADAPT):
# `map` / `filter` in `core/std/dict.t`. The adapters take the key and
# value as separate scalar arguments (`f: fn (K, V) -> U`) because an
# AOT closure cannot receive a tuple parameter. The iterator state is
# kept flat inside the adapter with `count` packed into `index`'s high
# 32 bits, staying within the backend's 8-return register budget.
#
# Run: cargo run -q -p interpreter -- example/std_iter_adapt_dict.t
# Expected exit code: 1160 (660 map + 500 filter)

fn main() -> u64 {
    var d: Dict<u64, u64> = Dict::new()
    d.insert(10u64, 100u64)
    d.insert(20u64, 200u64)
    d.insert(30u64, 300u64)

    # map: k + v over each pair
    var it = d.iter()
    var m = it.map(fn(k: u64, v: u64) -> u64 { k + v })
    var t1: u64 = 0u64
    for x in m { t1 = t1 + x }

    # filter: pairs whose value exceeds 150
    var it2 = d.iter()
    var f = it2.filter(fn(k: u64, v: u64) -> bool { v > 150u64 })
    var t2: u64 = 0u64
    for kv in f { t2 = t2 + kv.1 }

    # 110 + 220 + 330 (map) + 200 + 300 (filter) = 1160
    t1 + t2
}