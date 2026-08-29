# `f32` — the single-precision float (SIMD-F32).
#
# f32 exists so `f32x4` can be the SIMD mainstay (SIMD.md's undecided
# point 1). Arithmetic is IEEE-754 single precision; mixing f32 with
# f64 (or with any integer) is a type error — cross-width moves go
# through an explicit `as`. Note `0.1f32 + 0.2f32 == 0.3f32` is true
# at single precision (the f64 experiment famously fails this).

fn main() -> u64 {
    val a: f32 = 1.5f32
    val b: f32 = 2.25f32

    # Arithmetic, unary minus, comparisons — all f32-native.
    val c = a * b + 0.5f32
    val d = -a
    val lt = a < b

    # Single-precision rounding: f32 keeps 0.1 + 0.2 == 0.3.
    val prec = 0.1f32 + 0.2f32 == 0.3f32

    # Cast matrix: f32 ↔ f64 promote / demote, f32 → int saturates.
    val wide: f64 = c as f64
    val back: f32 = wide as f32
    val i: u64 = 3.9f32 as u64

    # `const` initialisers fold at compile time.
    # (See `compiler/tests/consistency/float32.rs` for that pin.)

    println(c)
    println(d)
    println(lt)
    println(prec)
    println(wide)
    println(back)
    println(i)

    if c == 3.875f32 && lt && prec && back == 3.875f32 && i == 3u64 {
        7u64
    } else {
        0u64
    }
}
