# Design-by-Contract: `requires` (preconditions) and `ensures` (postconditions).
#
# - `requires` clauses are evaluated on function entry. A false predicate
#   is the *caller's* bug — they violated the contract.
# - `ensures` clauses run just before return, with `result` bound to the
#   return value. A false predicate is the *implementation's* bug.
# - Multiple clauses of either kind are AND-composed; each gets its own
#   diagnostic so violations point at a specific predicate.
#
# Both run only at runtime in this iteration (silent JIT fallback). The
# type checker still validates that each clause is bool-typed and that
# `result` references the declared return type.

# Plain function with both pre- and postconditions.
# The postcondition has to account for the remainder: integer division
# truncates, so `result * b == a` is false for 7 / 2. `--check` finds
# that in a second — a plausible-looking clause is exactly the kind
# worth running it against. The second precondition is the other trap
# a non-zero divisor does not rule out: `MIN / -1` has no
# representable result.
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    requires !(a == -9223372036854775808i64 && b == -1i64)
    ensures  result * b + (a % b) == a
{
    a / b
}

# Multiple `requires` are AND-composed; failures cite the clause index.
fn between(x: i64, lo: i64, hi: i64) -> i64
    requires lo <= hi
    requires lo <= x
    requires x <= hi
    ensures  result == x
{
    x
}

# Methods on structs. `self` is in scope for both clauses; `result` is
# the method's return value.
struct Counter {
    n: i64,
}

impl Counter {
    fn inc(self: Self) -> Self
        requires self.n >= 0i64
        ensures  result.n == self.n + 1i64
    {
        Counter { n: self.n + 1i64 }
    }
}

fn main() -> i64 {
    val q: i64 = divide(20i64, 4i64)            # 5
    val m: i64 = between(7i64, 0i64, 10i64)     # 7
    val c = Counter { n: 9i64 }
    val c2 = c.inc()                            # 10
    q + m + c2.n                                # 22
}
