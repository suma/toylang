# Smoke test for the f64 math builtins (pow / sqrt).
# Expected: floor(sqrt(16f64)) + floor(pow(2f64, 5f64)) = 4 + 32 = 36 → exit 36.
fn main() -> u64 {
    val a: f64 = sqrt(16f64)
    val b: f64 = pow(2f64, 5f64)
    a as u64 + b as u64
}
