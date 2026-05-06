# Phase JE-5 end-to-end: generic enum function boundary. A
# generic enum monomorph (`Opt<i64>`) flows across function
# args and returns through the JIT — `ParamTy::Enum` carries
# the per-monomorph payload_ty so each instantiation gets a
# distinct cranelift signature. Both modes return exit 42.

enum Opt<T> {
    None,
    Some(T),
}

fn unwrap_or_zero(o: Opt<i64>) -> i64 {
    match o {
        Opt::Some(x) => x,
        Opt::None => 0i64,
    }
}

fn double_opt(o: Opt<i64>) -> Opt<i64> {
    match o {
        Opt::Some(x) => Opt::Some(x + x),
        Opt::None => Opt::None,
    }
}

fn main() -> i64 {
    val s: Opt<i64> = Opt::Some(20i64)
    val n: Opt<i64> = Opt::None
    val d: Opt<i64> = double_opt(s)
    unwrap_or_zero(d) + unwrap_or_zero(n) + 2i64
}
