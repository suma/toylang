fn buggy_abs(x: i64) -> i64
    ensures result >= 0i64
{
    -x
}
fn main() -> i64 { buggy_abs(5i64) }
