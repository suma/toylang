# Smoke test for tuple JIT support. Tuples are scalar-only at the JIT
# layer, so each element becomes its own SSA Variable; tuple parameters
# expand into one cranelift parameter per element and tuple returns
# expand into one cranelift return per element. Argument tuples must be
# passed as named locals so the JIT can find the per-element Variables.
#
# Out of scope: nested tuples (`((a, b), c)` shapes), tuple literals as
# function arguments, tuples whose elements are non-scalar.
#
# Expected: 20 + 10 + 3 = 33 → exit 33.
fn swap(p: (u64, u64)) -> (u64, u64) {
    (p.1, p.0)
}

fn add_pair(p: (i64, i64)) -> i64 {
    p.0 + p.1
}

fn main() -> u64 {
    val src = (10u64, 20u64)
    val (a, b) = swap(src)
    val ints = (1i64, 2i64)
    val s = add_pair(ints)
    a + b + s as u64
}
