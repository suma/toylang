# Phase JE-4 end-to-end: multi-generic-param enum (`Res<T, E>`,
# the `Result<T, E>` shape) compiles via the JIT when both type
# args resolve to a uniform scalar at the monomorph (the JIT's
# single-payload-slot representation requires all variants to
# agree). Both modes return exit 42.

enum Res<T, E> {
    Ok(T),
    Err(E),
}

fn main() -> i64 {
    val a: Res<i64, i64> = Res::Ok(40i64)
    val b: Res<i64, i64> = Res::Err(2i64)
    val sum_a: i64 = match a {
        Res::Ok(x) => x,
        Res::Err(_) => 0i64,
    }
    val sum_b: i64 = match b {
        Res::Ok(_) => 0i64,
        Res::Err(e) => e,
    }
    sum_a + sum_b
}
