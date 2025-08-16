struct Inner {
    value: i64
}

struct Outer {
    inner: Inner,
    count: i64
}

fn main() -> i64 {
    val o =  Outer { 
            inner: Inner { value: 10i64 }, 
            count: 1i64 
        }
    o.inner.value + o.count
}
