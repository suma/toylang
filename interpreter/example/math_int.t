# Smoke test for the integer math wrappers in `std math` (== the
# `math` module shipped under `interpreter/modules/math/math.t`).
# Expected: abs(-7) + min(3, 5) + max(10u64, 4u64) = 7 + 3 + 10 = 20 → exit 20.

fn main() -> u64 {
    val a: i64 = math::abs(-7i64)
    val b: i64 = math::min_i64(3i64, 5i64)
    val c: u64 = math::max_u64(10u64, 4u64)
    a as u64 + b as u64 + c
}
