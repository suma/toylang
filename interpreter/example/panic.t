# `panic("msg")` aborts the run with the supplied message. The call is
# special: its "return type" is treated as Unknown so it can appear in
# value positions like the `then` branch of an `if`-expression — the
# surrounding context is what fixes the type.

const ERR_DIVISION_BY_ZERO: str = "division by zero"

fn divide(a: i64, b: i64) -> i64 {
    if b == 0i64 {
        panic(ERR_DIVISION_BY_ZERO)
    } else {
        a / b
    }
}

fn main() -> i64 {
    val ok: i64 = divide(20i64, 4i64)
    println(ok)                # 5
    # Uncomment to see panic in action:
    # divide(10i64, 0i64)
    ok
}
