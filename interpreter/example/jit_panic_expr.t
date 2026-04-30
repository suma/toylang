# Expression-position panic in the JIT. The then-branch diverges and
# its eligibility type is Never, which unifies with the else-branch's
# i64 to give the if-expression an i64 value type. Codegen emits
# `trap UserCode(1)` on the panicking path, so the cont block has
# only the else-branch as a predecessor.
#
# `divide(10i64, 2i64)` returns 5i64. Replacing the second arg with
# 0i64 traps with "division by zero" and exits 1.
fn divide(a: i64, b: i64) -> i64 {
    val q: i64 = if b == 0i64 { panic("division by zero") } else { a / b }
    q
}

fn main() -> i64 {
    divide(10i64, 2i64)
}
