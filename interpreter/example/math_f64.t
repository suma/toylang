# Smoke test for the f64 math wrappers in the `math` module.
# Expected: floor(sqrt(16f64)) + floor(pow(2f64, 5f64)) = 4 + 32 = 36 → exit 36.
import math

fn main() -> u64 {
    val a: f64 = math::sqrt(16f64)
    val b: f64 = math::pow(2f64, 5f64)
    a as u64 + b as u64
}
