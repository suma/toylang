# Value-method form: numeric values can call `.abs()` / `.sqrt()`
# directly without going through the math module qualifier.
# Expected: abs(-7) + sqrt(16) = 7 + 4 = 11 → exit 11.
fn main() -> u64 {
    val x: i64 = -7i64
    val r: f64 = 16f64
    x.abs() as u64 + r.sqrt() as u64
}
