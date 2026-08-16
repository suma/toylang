# Stdlib iteration (STDLIB-ITER): `for x in coll.iter()` works on the
# three standard collections. The parser desugars
# `for x in EXPR { body }` to a `while` + `match __iter.next()`
# loop against any struct exposing `fn next(&mut self) -> Option<T>`
# (structural / duck-typed; see `iter_demo.t` for the protocol).
#
#   - `v.iter()` on a `Vec<T>` yields each element as a copy
#   - `d.iter()` on a `Dict<K, V>` yields `(key, value)` tuples in
#     insertion order
#   - `s.iter()` on a `String` yields one `u8` per byte
#
# Range-based `for i in 0..N` keeps its dedicated integer fast path.
#
# Run: cargo run -q -p interpreter -- example/std_iter.t
# Expected exit code: 32

fn main() -> i64 {
    var v: Vec<i64> = Vec::new()
    v.push(1i64)
    v.push(2i64)
    v.push(3i64)

    var d: Dict<i64, i64> = Dict::new()
    d.insert(10i64, 100i64)
    d.insert(20i64, 200i64)

    val s = String::from_str("abc")

    var total = 0i64
    for x in v.iter() {
        total = total + x
    }
    for kv in d.iter() {
        val (k, v2) = kv
        total = total + (v2 / k)
    }
    for b in s.iter() {
        total = total + (b as i64) - 96i64
    }
    # 6 (vec) + 20 (dict) + 6 (string bytes 1+2+3)
    total
}
