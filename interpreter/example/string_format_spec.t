# STR-INTERP-FMT: format specs in string interpolation.
#
# `"{value:spec}"` renders `value` under `spec` instead of the plain
# display form. The grammar is a subset of Rust's:
#
#     spec := [align] ['0'] [width] ['.' precision] [type]
#     align := '<' | '>' | '^'
#     type  := 'x' | 'X' | 'b' | 'o'
#
# The spec is part of the literal, so it is parsed and packed at
# compile time — a malformed one is a parse error, never a runtime
# surprise. Specs apply to primitives only; a struct / tuple / enum
# with a spec is a type error (give the type a `to_str` method and
# interpolate that instead).

fn main() -> i64 {
    # Precision is the reason this feature exists: without it there is
    # no way to choose how many decimals an f64 shows.
    val pi: f64 = 3.14159265f64
    println("pi ~ {pi:.2}")
    println("pi ~ {pi:.5}")
    println("pi ~ {pi}")

    # Width pads; alignment picks the side. Numbers default to
    # right-aligned, text to left-aligned (same rule as Rust).
    val n: u64 = 42u64
    println("[{n:6}] [{n:<6}] [{n:^6}]")
    println("[{n:06}]")

    # Zero-padding inserts after the sign, so the digits stay readable.
    val neg: i64 = -42i64
    println("[{neg:6}] [{neg:06}]")

    # Radix. A negative value renders its two's-complement pattern at
    # its own width, so a narrow int does not show 16 hex digits.
    println("{n:x} {n:X} {n:b} {n:o}")
    val narrow: i32 = -1i32
    println("i32 -1 = {narrow:x}")

    # Text and bools take width / alignment.
    val label: str = "ok"
    println("[{label:6}] [{label:>6}] [{label:^6}]")
    val flag: bool = true
    println("[{flag:>7}]")

    # Combined, and alongside plain segments in the same literal.
    val ratio: f64 = 0.5f64
    println("ratio={ratio:8.3} count={n:<4} done={flag}")

    # `{{` / `}}` still lex to literal braces next to a spec.
    println("{{{n:x}}}")

    0i64
}
