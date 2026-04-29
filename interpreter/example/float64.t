# f64 (IEEE 754 double-precision) literals and arithmetic.
#
# Float literals require an explicit `f64` suffix so they remain
# unambiguous against tuple-access syntax like `outer.0.1`. Both
# `3.14f64` and `42f64` are accepted.
#
# Supported operators on f64: + - * / % comparisons (`==`, `!=`,
# `<`, `<=`, `>`, `>=`), and unary minus. Comparisons follow IEEE
# 754 — NaN compares false against everything (including itself).
#
# `as` converts between f64 and i64/u64. f64 → integer truncates
# toward zero and saturates on out-of-range; NaN becomes 0.

fn main() -> u64 {
    # Basic arithmetic
    val a: f64 = 3.0f64
    val b: f64 = 2.0f64
    println(a + b)        # 5.0
    println(a * b)        # 6.0
    println(a - b)        # 1.0
    println(a / b)        # 1.5
    println(7.0f64 % 3.0f64)  # 1.0

    # Unary minus
    val neg: f64 = -2.5f64
    println(neg)          # -2.5

    # Casts: integer → f64 → integer
    val i: i64 = 5i64
    val f: f64 = i as f64
    println(f)            # 5.0
    val back: i64 = (f * 2.0f64) as i64
    println(back)         # 10

    # Comparisons produce bool
    if a > b {
        println(1u64)
    } else {
        println(0u64)
    }

    0u64
}
