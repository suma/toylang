# Smoke test for JIT Phase 2f: generic function monomorphization. Each
# call site is specialized for the arg types, so `id<i64>`, `id<u64>`,
# and `add<u64>` become three distinct cranelift functions.
# Expected exit: 7 + 100 + 99 = 206
fn id<T>(x: T) -> T {
    x
}

fn add<T>(a: T, b: T) -> T {
    a + b
}

fn main() -> u64 {
    val a: i64 = id(7i64)
    val b: u64 = id(100u64)
    val c: u64 = add(50u64, 49u64)
    a as u64 + b + c
}
