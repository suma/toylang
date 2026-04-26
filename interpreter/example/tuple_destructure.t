# Tuple destructuring via `val` and `var`. The parser desugars
# `val (a, b) = expr` into a hidden temporary plus per-name bindings
# through `tmp.0`, `tmp.1`, …, so this works without any new AST node.
#
# Expected: 7 + 3 + 100 + 200 + 300 + 6 + 2 = 618 (exit 618 % 256 = 106).
fn pair_swap(p: (u64, u64)) -> (u64, u64) {
    (p.1, p.0)
}

fn main() -> u64 {
    val (a, b) = pair_swap((3u64, 7u64))
    val (x, y, z) = (100u64, 200u64, 300u64)
    var (m, n) = (1u64, 2u64)
    m = m + 5u64
    a + b + x + y + z + m + n
}
