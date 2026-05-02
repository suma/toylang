# Smoke test for Step C — JIT primitive method dispatch.
# `impl Negate for i64 { fn neg(self) -> Self }` makes `a.neg()`
# callable on a plain `i64` local. Both `main` and `i64::neg`
# should land on the JIT path; the program returns 7 (=
# `(-7).neg()`).

trait Negate {
    fn neg(self: Self) -> Self
}

impl Negate for i64 {
    fn neg(self: Self) -> Self {
        0i64 - self
    }
}

fn main() -> u64 {
    val a: i64 = 7i64
    val b: i64 = a.neg()       # -7
    val c: i64 = b.neg()       #  7
    c as u64
}
