# Modulo operator (`%`) and compound assignment (`+= -= *= /= %=`).
#
# Modulo follows truncated-remainder semantics, matching Rust / C:
#   ( 7) %  3 ==  1
#   (-7) %  3 == -1
#   ( 7) % -3 ==  1
#
# Compound assignment is parser-level desugaring: `x += rhs` lowers to
# `x = x + rhs`. Identifier and field-access LHS are supported.

struct Counter {
    n: i64,
}

fn main() -> i64 {
    # Basic modulo
    val a: i64 = 17i64 % 5i64       # 2
    val b: i64 = (100u64 % 7u64) as i64  # 100 % 7 = 2

    # Compound on identifier
    var x: i64 = 10i64
    x += 5i64    # 15
    x -= 2i64    # 13
    x *= 3i64    # 39
    x /= 2i64    # 19
    x %= 7i64    # 5

    # Compound on struct field
    var c = Counter { n: 0i64 }
    c.n += 10i64
    c.n *= 3i64

    a + b + x + c.n
}
