# Random numbers (STDLIB-NUMERIC N6).
#
# **Everything here is built on `io::random()`**, which is one extern
# and a reproducible xorshift64*. Nothing below crosses the boundary
# again, so the same seed produces the same sequence on every backend
# -- which is the only reason any of this can be tested at all. A
# `random_normal` implemented in libm would differ between lanes.
#
# **Not for cryptography.** The generator is a PRNG seeded from the
# clock and the process id, and `io::random_seed(s)` makes it fully
# predictable on purpose. An API that called itself secure would be
# claiming something this language cannot yet be responsible for.

pub fn random_u64() -> u64 { io::random() }

# A `u64` in `[lo, hi)`, with no bias.
#
# `random() % n` is biased unless `n` divides 2^64 -- the low residues
# get one more chance than the high ones. Drawing again when the value
# lands in the ragged tail removes it exactly, at the cost of an
# occasional extra draw (under one in 2^32 for any range worth using).
pub fn random_range(lo: u64, hi: u64) -> u64 {
    assert(lo < hi, "random_range: the range is empty")
    val span: u64 = hi - lo
    # A power of two divides 2^64 evenly, so the mask is unbiased and
    # never rejects.
    if span.is_power_of_two() {
        return lo + (io::random() & (span - 1u64))
    }
    # The largest multiple of `span` that fits; anything at or above it
    # is the tail that would skew the result.
    val limit: u64 = limits::u64_max() - (limits::u64_max() % span)
    var v: u64 = io::random()
    while v >= limit {
        v = io::random()
    }
    lo + (v % span)
}

# The signed version of the same half-open range. Computed as an
# offset from `lo` so a span wider than `i64` still fits.
#
# The span is `(hi - lo) as u64`, not `(hi as u64) - (lo as u64)`:
# **`u64` subtraction traps on underflow** while signed subtraction
# wraps, and for a range that straddles zero the unsigned spelling
# underflows every time. Two's complement makes the wrapped signed
# difference the right span.
pub fn random_i64_range(lo: i64, hi: i64) -> i64 {
    assert(lo < hi, "random_i64_range: the range is empty")
    val span: u64 = (hi - lo) as u64
    val off: u64 = random_range(0u64, span)
    ((lo as u64) + off) as i64
}

# A float in `[0, 1)`, with all 53 bits of the mantissa. Dividing by
# 2^53 is exact, so no value is more likely than its neighbour.
pub fn random_f64() -> f64 {
    val bits: u64 = io::random() >> 11u64
    (bits as f64) / 9007199254740992f64
}

pub fn random_bool() -> bool { (io::random() & 1u64) == 1u64 }

# A sample from the standard normal distribution (mean 0, variance 1),
# by the Box-Muller transform.
#
# In toylang rather than an extern for the reason at the top: a libm
# implementation would give different values on different lanes. The
# `u` draw excludes 0, which `log` cannot take.
pub fn random_normal() -> f64 {
    var u: f64 = random_f64()
    while u == 0f64 {
        u = random_f64()
    }
    val v: f64 = random_f64()
    val r: f64 = math::sqrt(-2f64 * math::log(u))
    val theta: f64 = 6.283185307179586f64 * v
    r * math::cos(theta)
}

# Shuffle a vector in place, uniformly (Fisher-Yates).
#
# Every one of the `n!` orderings is equally likely, which the naive
# "swap each element with a random position anywhere" loop is not --
# that one produces `n^n` equally likely swap sequences over `n!`
# orderings, and the two do not divide. Drawing from `[0, i]` and
# walking down is what makes the counts match.
#
# The draw goes through `random_range`, so the rejection there keeps
# the positions unbiased too; `random() % (i + 1)` would tilt every
# shuffle toward the low indices.
pub fn shuffle<T>(v: &mut Vec<T>) {
    val n: u64 = v.size()
    # `n - 1` would underflow on an empty vector, and `u64`
    # subtraction traps rather than wrapping (RUNTIME-TRAP).
    if n < 2u64 { return }
    var i: u64 = n - 1u64
    while i > 0u64 {
        val j: u64 = random_range(0u64, i + 1u64)
        if j != i {
            val a: T = v.get(i)
            val b: T = v.get(j)
            v.set(i, b)
            v.set(j, a)
        }
        i = i - 1u64
    }
}
