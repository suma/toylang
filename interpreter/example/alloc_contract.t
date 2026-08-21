# Allocation contracts (ALLOC-CONTRACT).
#
# `old(expr)` in an `ensures` clause is the value `expr` had on entry
# to the function. Combined with the allocation counters — which are
# readable from contracts and mean exactly what `--profile=mem`
# reports — that turns a function's memory behaviour into something it
# can promise in its signature, and every backend enforces the promise
# at run time.
#
# Run it under `--profile=mem` to see the same numbers the contracts
# are asserting on.

# "Allocates nothing." Not a comment that rots — a clause that stops
# the program the day someone adds a heap call to the body.
fn triangle(n: u64) -> u64
    ensures __builtin_live_bytes() == old(__builtin_live_bytes())
{
    var total: u64 = 0u64
    var i: u64 = 1u64
    while i <= n {
        total = total + i
        i = i + 1u64
    }
    total
}

# "Requests at most 256 bytes, and hands every one of them back."
# The first clause bounds the peak request, the second says the
# function is memory-neutral by the time it returns.
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

# `old` is not only about memory — it is the general "value on entry",
# which is what a postcondition about mutation needs.
struct Counter { n: u64 }

impl Counter {
    fn bump(&mut self, by: u64) -> u64
        ensures result == old(self.n) + by
    {
        self.n = self.n + by
        self.n
    }
}

fn main() -> u64 {
    println(triangle(10u64))
    println(scratch(7u64))
    var c = Counter { n: 40u64 }
    println(c.bump(2u64))
    0u64
}
