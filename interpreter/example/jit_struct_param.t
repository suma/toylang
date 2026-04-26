# Smoke test for JIT Phase 2d-2: struct values as function parameters.
# Each scalar field expands into its own cranelift parameter, so passing
# a `Point` to `sum_xy` flows two i64 values through the call boundary.
#
# Expected exit: (5+12) + (3+4) = 24
struct Point {
    x: i64,
    y: i64,
}

fn sum_xy(p: Point) -> i64 {
    p.x + p.y
}

fn main() -> u64 {
    val a = Point { x: 5i64, y: 12i64 }
    val b = Point { x: 3i64, y: 4i64 }
    val total: i64 = sum_xy(a) + sum_xy(b)
    total as u64
}
