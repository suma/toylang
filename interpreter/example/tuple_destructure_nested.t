# Nested tuple destructuring on `val` / `var`. The parser desugars each
# inner tuple pattern through its own hidden `__tuple_tmp_N` temporary,
# so any depth of nesting works.
#
# `val ((a, b), c) = ((1, 2), 10)` lowers roughly to:
#   val __tuple_tmp_0 = ((1, 2), 10)
#   val __tuple_tmp_1 = __tuple_tmp_0.0
#   val a = __tuple_tmp_1.0
#   val b = __tuple_tmp_1.1
#   val c = __tuple_tmp_0.1
#
# Result: 1 + 2 + 10 + 1 + 20 + 300 + 4000 = 4334 (exit = 4334 % 256 = 238).
fn make() -> ((i64, i64), (i64, i64)) {
    ((1i64, 2i64), (3i64, 4i64))
}

fn main() -> i64 {
    val ((a, b), c) = ((1i64, 2i64), 10i64)
    val ((p, q), (r, s)) = make()
    a + b + c
        + p + q * 10i64 + r * 100i64 + s * 1000i64
}
