fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    ensures result * b == a
{
    a / b
}
fn main() -> i64 { divide(20i64, 0i64) }
