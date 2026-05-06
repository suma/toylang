# Phase JE-3 end-to-end: a single-generic-param enum
# (`Opt<T>` shape) now compiles via the JIT when each
# instantiation's `T` is a JIT scalar. Constructor inference
# pulls T from the tuple-variant arg; unit constructors get T
# from the val/var annotation. Match payload binding picks up
# the resolved per-monomorph payload type. Both modes return
# exit 42.

enum Opt<T> {
    None,
    Some(T),
}

fn main() -> i64 {
    val a: Opt<i64> = Opt::Some(40i64)
    val b: Opt<i64> = Opt::None
    val sum_a: i64 = match a {
        Opt::Some(x) => x,
        Opt::None => 0i64,
    }
    val sum_b: i64 = match b {
        Opt::Some(x) => x,
        Opt::None => 2i64,
    }
    sum_a + sum_b
}
