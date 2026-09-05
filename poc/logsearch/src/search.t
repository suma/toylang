# Finding a short byte string inside a long one.
#
# This is the query engine's inner loop: a `contains` filter has no
# index behind it (QUERY.md §3), so every candidate record's bytes are
# walked. The scalar version is kept as the reference the SIMD one is
# checked against.
#
# The vector version filters on **two** bytes -- the needle's first
# and its last -- rather than one. That is not a micro-optimisation:
# a single-byte filter collapses to the naive scan whenever that byte
# is common, which for log text it usually is. Filtering on the pair
# makes candidates rare for the same input, and the candidates that
# do survive are taken from `__simd_bitmask` one bit at a time instead
# of by re-walking the window (SIMD.md §5: 1.3x vs 4.0x on a dense
# input, 15x on a sparse one).

# Whether `hay[at .. at+n]` equals the needle.
fn equal_at(hay: Span<u8>, at: u64, needle: Span<u8>, n: u64) -> bool
    requires at + n <= hay.len()
    requires n <= needle.len()
{
    var i: u64 = 0u64
    while i < n {
        val a: u8 = hay.get(at + i)
        val b: u8 = needle.get(i)
        if a != b { return false }
        i = i + 1u64
    }
    true
}

# The scalar reference: first-byte scan, then verify.
pub fn find_scalar(hay: Span<u8>, from: u64, len: u64, needle: Span<u8>, n: u64) -> bool
    requires from + len <= hay.len()
    requires n <= needle.len()
{
    if n == 0u64 { return true }
    if n > len { return false }
    val first: u8 = needle.get(0u64)
    val last_start = from + len - n
    var i = from
    while i <= last_start {
        val c: u8 = hay.get(i)
        if c == first {
            if equal_at(hay, i, needle, n) { return true }
        }
        i = i + 1u64
    }
    false
}

# The same answer, filtering sixteen positions at a time.
pub unsafe fn find(hay: Span<u8>, from: u64, len: u64, needle: Span<u8>, n: u64) -> bool
    requires from + len <= hay.len()
    requires n <= needle.len()
{
    if n == 0u64 { return true }
    if n > len { return false }
    # Below a window there is nothing to vectorise.
    if len < 32u64 { return find_scalar(hay, from, len, needle, n) }

    val p = hay.as_raw()
    val fb: u8 = needle.get(0u64)
    val lb: u8 = needle.get(n - 1u64)
    val fv: u8x16 = __simd_splat(fb)
    val lv: u8x16 = __simd_splat(lb)
    val last_start = from + len - n

    var i = from
    while i + 16u64 <= last_start {
        val a: u8x16 = __simd_load(p, i)
        val b: u8x16 = __simd_load(p, i + n - 1u64)
        val ma = a == fv
        val mb = b == lv
        val hit = ma & mb
        if __simd_any(hit) {
            var bits = __simd_bitmask(hit)
            while bits != 0u64 {
                val k = bits.trailing_zeros() as u64
                if equal_at(hay, i + k, needle, n) { return true }
                bits = bits & (bits - 1u64)
            }
        }
        i = i + 16u64
    }
    while i <= last_start {
        val c: u8 = hay.get(i)
        if c == fb {
            if equal_at(hay, i, needle, n) { return true }
        }
        i = i + 1u64
    }
    false
}

# Whether the two windows hold the same bytes.
pub fn equals(hay: Span<u8>, at: u64, len: u64, want: Span<u8>, n: u64) -> bool
    requires at + len <= hay.len()
    requires n <= want.len()
{
    if len != n { return false }
    equal_at(hay, at, want, n)
}
