# Design by Contract, end to end. The companion guide is
# `docs/design_by_contract.md`; this file is the part that runs.
#
#   cargo run -q -p interpreter -- interpreter/example/design_by_contract.t
#   cargo run -q -p interpreter -- --check interpreter/example/design_by_contract.t
#
# Every contract here is one `--check` agrees with. That is not a
# given: a plausible-looking postcondition is exactly the kind that
# turns out to be false on an input nobody thought about.

# `requires` states what the caller owes, `ensures` what the function
# owes back. Note the second precondition: ruling out a zero divisor
# is not enough, because `MIN / -1` has no representable result and
# traps as well.
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    requires !(a == -9223372036854775808i64 && b == -1i64)
    ensures result * b + (a % b) == a
{
    a / b
}

# Preconditions are also how an operation gets cheaper: the guard that
# would test `b != 0` at the division site is dropped, because the
# clause above already established it on entry.
fn average(total: u64, count: u64) -> u64
    requires count != 0u64
    ensures result <= total
{
    total / count
}

# `old(expr)` is the value `expr` had on entry — the only way to write
# a postcondition about a change rather than about a final state.
struct Counter { n: u64 }

impl Counter {
    fn bump(&mut self, by: u64) -> u64
        ensures result == old(self.n) + by
        ensures self.n == result
    {
        self.n = self.n + by
        self.n
    }
}

# The allocation counters are readable from contracts, so a function's
# memory behaviour can be part of its signature. This one promises to
# hand back everything it takes.
fn scratch(n: u64) -> u64
    ensures __builtin_cumulative_bytes() - old(__builtin_cumulative_bytes()) <= 256u64
    ensures __builtin_live_bytes() == old(__builtin_live_bytes())
{
    val p: ptr = __builtin_heap_alloc(128u64)
    __builtin_ptr_write(p, 0u64, n)
    val v: u64 = __builtin_ptr_read(p, 0u64)
    __builtin_heap_free(p)
    v
}

# A `test` block checks one case the author cares about; a contract
# checks every call the program makes. They are complements, not
# alternatives.
test "divide truncates toward zero" {
    assert_eq(divide(7i64, 2i64), 3i64)
    assert_eq(divide(-7i64, 2i64), -3i64)
}

test "bump reports the new value" {
    var c = Counter { n: 40u64 }
    assert_eq(c.bump(2u64), 42u64)
    assert_eq(c.n, 42u64)
}

fn main() -> u64 {
    println(divide(7i64, 2i64))
    println(average(10u64, 4u64))
    var c = Counter { n: 40u64 }
    println(c.bump(2u64))
    println(scratch(7u64))
    0u64
}
