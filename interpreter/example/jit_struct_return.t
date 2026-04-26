# Smoke test for JIT Phase 2d-3: struct values as function return type.
# `make_point` returns a `Point`; cranelift uses multi-return to deliver
# both fields, and the caller rebuilds a struct local from the results.
#
# Expected exit: (3+4) + (5+6) = 18
struct Point {
    x: i64,
    y: i64,
}

fn make_point(x: i64, y: i64) -> Point {
    Point { x: x, y: y }
}

fn sum_xy(p: Point) -> i64 {
    p.x + p.y
}

fn main() -> u64 {
    val a = make_point(3i64, 4i64)
    val b = make_point(5i64, 6i64)
    val total: i64 = sum_xy(a) + sum_xy(b)
    total as u64
}
