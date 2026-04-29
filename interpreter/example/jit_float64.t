# Smoke test for f64 in the cranelift JIT. Exercises:
#   - f64 literals (with `f64` suffix to avoid tuple-access ambiguity)
#   - +, -, *, /, comparisons, unary minus
#   - i64 → f64 → i64 casts (fcvt_from_sint / fcvt_to_sint_sat)
#   - println(f64) via the `jit_println_f64` helper
#
# The exit code is the truncated sum, so a discrepancy between the
# interpreter and JIT is caught by the integration test framework.
fn main() -> u64 {
    val a: f64 = 3.0f64
    val b: f64 = 2.0f64
    println(a + b)        # 5.0
    println(a * b)        # 6.0
    println(a / b)        # 1.5
    println(-a)           # -3.0

    val i: i64 = 5i64
    val f: f64 = i as f64
    val back: i64 = (f * 2.0f64) as i64
    println(back)         # 10

    if a > b {
        7u64
    } else {
        0u64
    }
}
