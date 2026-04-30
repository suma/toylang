fn divide(a: i64, b: i64) -> i64 {
    val q: i64 = if b == 0i64 { panic("division by zero") } else { a / b }
    q
}

fn main() -> i64 {
    divide(10i64, 0i64)
}
