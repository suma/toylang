# Smoke test for JIT Phase 2b: print / println of scalars + a small loop.
# Expected stdout (with INTERPRETER_JIT=1):
#   42
#   true
#   2
#   3
# Exit code: 1+2+3 = 6
fn sum_to(n: u64) -> u64 {
    var acc: u64 = 0u64
    var i: u64 = 1u64
    while i <= n {
        acc = acc + i
        i = i + 1u64
    }
    acc
}

fn main() -> u64 {
    println(42u64)
    println(true)
    for i in 2u64..4u64 {
        println(i)
    }
    sum_to(3u64)
}
