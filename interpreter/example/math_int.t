# Smoke test for the integer math builtins.
# Expected: abs(-7) + min(3, 5) + max(10u64, 4u64) = 7 + 3 + 10 = 20 → exit 20.
fn main() -> u64 {
    val a: i64 = abs(-7i64)
    val b: i64 = min(3i64, 5i64)
    val c: u64 = max(10u64, 4u64)
    a as u64 + b as u64 + c
}
