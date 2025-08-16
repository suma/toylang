struct Inner {
    value: i64
}

fn main() -> i64 {
    val inner = Inner { value: 10i64 }
    inner.value
}