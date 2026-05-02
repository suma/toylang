# Nested-tuple parameter `((i64, i64), i64)`. The interpreter
# handles this directly; the JIT currently rejects it at the
# parameter-resolution layer (todo #160) and the function falls
# back to the tree walker. Both modes must still return the same
# result. The expected exit code is 6 = 1 + 2 + 3.

fn add_nested(p: ((i64, i64), i64)) -> i64 {
    p.0.0 + p.0.1 + p.1
}

fn main() -> u64 {
    val r: i64 = add_nested(((1i64, 2i64), 3i64))
    r as u64
}
