# Tuple-literal expressions can be passed inline as call arguments
# (in addition to the existing identifier-of-tuple-local form).
# Expected: 1 + 2 + 30 + 40 = 73 → exit 73.
fn add_pair(p: (i64, i64)) -> i64 {
    p.0 + p.1
}

fn add_two_pairs(a: (u64, u64), b: (u64, u64)) -> u64 {
    a.0 + a.1 + b.0 + b.1
}

fn main() -> u64 {
    val s1 = add_pair((1i64, 2i64))
    val s2 = add_two_pairs((10u64, 20u64), (30u64, 40u64))
    s1 as u64 + s2 - 30u64
}
