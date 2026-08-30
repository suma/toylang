# SIMD (SIMD.md Phase 2): 128-bit vectors as a type, with the
# ordinary operators working lane-wise.
#
# Five lane types exist: f64x2 / f32x4 / i32x4 / i64x2 / u8x16. The
# width stops at 128 bits because SSE2 (x86-64) and NEON (aarch64)
# both have it unconditionally, so nothing here is host-dependent.
#
# `__simd_splat` / `__simd_load` have no lane-type suffix -- they take
# their type from the annotation at the call site, or from the vector
# on the other side of an operator.

fn dot(xs: ptr, ys: ptr, pairs: u64) -> f64 {
    var total: f64 = 0f64
    var i: u64 = 0u64
    while i < pairs {
        val a: f64x2 = __simd_load(xs, i * 2u64)
        val b: f64x2 = __simd_load(ys, i * 2u64)
        # Lane-wise multiply, then a horizontal fold. The fold runs
        # lane 0 -> n in order, which is what makes the answer the
        # same on every backend.
        total = total + __simd_reduce_add(a * b)
        i = i + 1u64
    }
    total
}

fn main() -> u64 {
    # --- lane-wise operators ---------------------------------------
    val a: f64x2 = __simd_splat(1.5f64)
    val b: f64x2 = __simd_splat(2.0f64)
    println(a * b + a)

    # A comparison answers per lane, so it produces a mask (all-ones /
    # all-zeros), not one bool. `__simd_all` asks the whole-vector
    # question.
    val mask = a < b
    println(mask)
    println(__simd_all(mask))

    # Branch-free choice between two vectors.
    println(__simd_select(mask, a, b))

    # --- integer lanes ---------------------------------------------
    # Integer lanes wrap and never trap: the scalar `u64 -` panics on
    # underflow, but a per-lane guard would defeat the point.
    val m: i32x4 = __simd_splat(2147483647i32)
    println(m + __simd_splat(1i32))

    # `<<` / `>>` shift every lane by the same scalar amount.
    val u: u8x16 = __simd_splat(200u8)
    println(u >> 2u64)
    println(u & __simd_splat(15u8))

    # --- memory ------------------------------------------------------
    # `__simd_load(p, i)` addresses by *element*: lane k lives at byte
    # offset (i + k) * lane_bytes. Note the contrast with
    # `__builtin_ptr_read`, whose offset is a byte count.
    val xs = __builtin_heap_alloc(64u64)
    val ys = __builtin_heap_alloc(64u64)
    var k: u64 = 0u64
    while k < 4u64 {
        __builtin_ptr_write(xs, k * 8u64, (k + 1u64) as f64)
        __builtin_ptr_write(ys, k * 8u64, 2f64)
        k = k + 1u64
    }
    println(dot(xs, ys, 2u64))
    __builtin_heap_free(xs)
    __builtin_heap_free(ys)

    # --- lane addressing -------------------------------------------
    val v = __simd_insert(a, 1u64, 9f64)
    val lane: f64 = __simd_extract(v, 1u64)
    println(lane)
    println("as text: {v}")
    0u64
}
