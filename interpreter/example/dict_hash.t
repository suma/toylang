# `Dict<K, V>` after COLLECTIONS C1: lookup probes a slot table, and
# iteration still walks the entries in the order they were inserted.
#
# `str` keys are the case worth looking at: they hash through the
# runtime's FNV-1a rather than the identity hash the integer widths
# use, and they used to all land in one bucket.

fn main() -> u64 {
    var counts: Dict<str, u64> = Dict::new()
    counts.insert("alpha", 1u64)
    counts.insert("beta", 2u64)
    counts.insert("gamma", 3u64)

    # An update keeps the entry where it is.
    counts.insert("beta", 20u64)

    # A removal shifts the survivors down, so what is left is still in
    # insertion order.
    counts.remove("alpha")

    for kv in counts.iter() {
        println("{kv.0}={kv.1}")
    }
    println(counts.size())
    println(counts.contains_key("alpha"))
    println(counts.get_or("gamma", 0u64))

    # Enough integer keys to grow the slot table (it starts at 8 and
    # doubles past a 7/8 load factor), then read every one back.
    var squares: Dict<u64, u64> = Dict::new()
    var i: u64 = 0u64
    while i < 64u64 {
        squares.insert(i * 3u64, i * i)
        i = i + 1u64
    }
    var found: u64 = 0u64
    var j: u64 = 0u64
    while j < 64u64 {
        if squares.get_or(j * 3u64, 999u64) == j * j {
            found = found + 1u64
        }
        j = j + 1u64
    }
    println(found)
    println(squares.size())
    0u64
}
