# NEVER-ALLOCATES: `never_allocates fn` — "this cannot allocate",
# checked at compile time.
#
#   cargo run -q -p interpreter -- interpreter/example/never_allocates.t
#
# The runtime counterpart is `ensures allocates(0u64)`, which measures
# one call and reports what it cost. This one follows every path out
# of the function and refuses the program if any of them reaches
# `__builtin_heap_alloc` / `__builtin_heap_realloc`, so it costs
# nothing at run time and cannot be wrong about a call that did not
# happen to allocate this time.

never_allocates fn triangle(n: u64) -> u64 {
    var total: u64 = 0u64
    var i: u64 = 1u64
    while i <= n {
        total = total + i
        i = i + 1u64
    }
    total
}

# Reporting is allowed: what the runtime spends holding a `str` is not
# the program's allocation, and the counters exclude it for the same
# reason.
never_allocates fn report(label: str, value: u64) -> u64 {
    println("{label} = {value}")
    value
}

# Recursion is fine — a cycle adds no reachable code of its own.
never_allocates fn gcd(a: u64, b: u64) -> u64 {
    if b == 0u64 {
        a
    } else {
        gcd(b, a % b)
    }
}

# Both halves of the pair, on one function: the static check says it
# cannot allocate, the clause says this call did not.
never_allocates fn checked(n: u64) -> u64
    ensures allocates(0u64)
{
    n * 2u64
}

fn main() -> u64 {
    val t: u64 = triangle(10u64)
    println(t)
    report("gcd", gcd(48u64, 18u64))
    println(checked(21u64))
    0u64
}
