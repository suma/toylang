struct Inner {
    value: i64
}

struct Outer {
    inner: Inner
}

fn main() -> i64 {
    val outer = Outer { inner: Inner { value: 10i64 } }
    outer.inner.value
}