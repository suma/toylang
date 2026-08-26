# `const fn` — a function the compiler may run while compiling.
#
# The spelling is C++'s `constexpr`, not `consteval`: it says the
# function *can* be folded, never that it must be. What forces a fold
# is the position the call sits in — a `const` initialiser has to have
# a value before the program starts, while an ordinary call is folded
# only as an optimisation.
#
# The fold runs on the tree-walking interpreter, so a folded value is
# by construction the value the program would have computed. Wrapping
# `+` / `*` and truncated signed division come out the same either way.

const fn double(n: u64) -> u64 { n * 2u64 }

const fn factorial(n: u64) -> u64 {
    if n <= 1u64 { 1u64 } else { n * factorial(n - 1u64) }
}

# A `const fn` may call an unannotated function: the check follows the
# call graph rather than requiring every callee to carry the modifier.
fn add_one(n: u64) -> u64 { n + 1u64 }
const fn plus_one(n: u64) -> u64 { add_one(n) }

# Both initialisers below are literals by the time any backend sees
# them. Before `const fn` existed, a call here ran on the tree-walker
# and failed to compile on the other three engines.
const DOUBLED: u64 = double(21u64)        # 42
const CHAINED: u64 = double(DOUBLED)      # reads the earlier const
const FACT: u64 = factorial(10u64)        # 3628800

fn main() -> u64 {
    println(DOUBLED)                      # 42
    println(CHAINED)                      # 84
    println(FACT)                         # 3628800
    println(plus_one(1u64))               # 2 — folded at the call site
    DOUBLED
}
