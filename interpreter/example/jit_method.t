# Smoke test for JIT Phase 2d-4: struct method dispatch.
# `p.dist_squared()` lowers to a regular cranelift call where the
# receiver expands into per-field arguments alongside any extra args.
#
# Expected exit: (3*3 + 4*4) + (5*5 + 12*12) = 25 + 169 = 194
struct Point {
    x: i64,
    y: i64,
}

impl Point {
    fn dist_squared(self: Self) -> i64 {
        self.x * self.x + self.y * self.y
    }
}

fn main() -> u64 {
    val a = Point { x: 3i64, y: 4i64 }
    val b = Point { x: 5i64, y: 12i64 }
    val total: i64 = a.dist_squared() + b.dist_squared()
    total as u64
}
