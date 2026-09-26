# Range, top-level name and `n @ pat` patterns over scalar scrutinees.
# The interpreter's JIT compiles these functions natively
# (INTERPRETER_JIT=1 -v lists them) instead of falling back.

fn bucket(n: i64) -> i64 {
    match n {
        -10i64..0i64 => 1i64,
        0i64 => 2i64,
        small @ 1i64..10i64 => small * 10i64,
        other => other + 1000i64,
    }
}

fn digits(n: u64) -> u64 {
    match n % 100u64 {
        0u64..10u64 => 1u64,
        x @ 10u64..100u64 => x / 10u64,
        _ => 0u64,
    }
}

fn main() -> u64 {
    val a = bucket(-3i64) + bucket(0i64) + bucket(7i64) + bucket(42i64)
    val b = digits(5u64) + digits(1234u64) + digits(99u64)
    # 1 + 2 + 70 + 1042 = 1115, 1 + 3 + 9 = 13
    println((a as u64) + b)
    0u64
}
