# Smoke test for JIT Phase 2d: struct field access. Each scalar field
# becomes its own SSA Variable, so reads and writes go through the SSA
# layer rather than memory. Out of scope: struct copies, struct as
# function param/return, methods, nested struct fields.
#
# Expected exit: (1+9) + (2+8) = 20
struct Point {
    x: i64,
    y: i64,
}

fn main() -> u64 {
    var p = Point { x: 1i64, y: 2i64 }
    p.x = p.x + 9i64
    p.y = p.y + 8i64
    val total: i64 = p.x + p.y
    total as u64
}
