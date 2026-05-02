# Demo: multi-segment module path. `import std.math` resolves to
# `modules/std/math.t`; the alias derives from the last segment
# (`math`), so call sites still write `math::abs(x)` even though
# the file lives under `std/`.
#
# Expected: 9 + 4 = 13 → exit 13.
import std.math

fn main() -> u64 {
    val a: i64 = math::abs(-9i64)
    val b: f64 = math::sqrt(16f64)
    a as u64 + b as u64
}
