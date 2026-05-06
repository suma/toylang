# Phase JE-6 end-to-end: enum receiver method dispatch.
# `impl<T> Opt<T>` methods (is_some / unwrap_or) lower through
# the same MonoTarget::Method path used for struct methods, with
# the receiver's per-monomorph payload_ty driving the generic
# substitution. Both modes return exit 42.

enum Opt<T> {
    None,
    Some(T),
}

impl<T> Opt<T> {
    fn is_some(self: Self) -> bool {
        match self {
            Opt::Some(_) => true,
            Opt::None => false,
        }
    }
    fn unwrap_or(self: Self, default: T) -> T {
        match self {
            Opt::Some(x) => x,
            Opt::None => default,
        }
    }
}

fn main() -> i64 {
    val a: Opt<i64> = Opt::Some(40i64)
    val b: Opt<i64> = Opt::None
    val sum_a: i64 = a.unwrap_or(0i64)
    val sum_b: i64 = b.unwrap_or(2i64)
    val flag_a: bool = a.is_some()
    val flag_b: bool = b.is_some()
    if flag_a {
        if flag_b { 99i64 } else { sum_a + sum_b }
    } else {
        0i64
    }
}
