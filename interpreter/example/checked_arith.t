# Overflow-aware integer arithmetic from `core/std/checked.t`
# (RUNTIME-TRAP). The operators wrap; these methods report the
# overflow instead — as an `Option` (`checked_*`) or by clamping to
# the type's bound (`saturating_*`).
#
# One trait, `Checked`, with an impl per integer width: `u8` / `u16` /
# `u32` / `u64` and `i8` / `i16` / `i32` / `i64`. The bounds each impl
# clamps to are its own type's, so the same call reads the same way at
# every width.
#
# Two shapes are load-bearing for the compiled backends: the receiver
# is a name rather than a literal, and an enum result is bound with
# `val` before it is matched.

fn main() -> u64 {
    val u_max: u64 = 18446744073709551615u64
    val five: u64 = 5u64
    val ten: u64 = 10u64

    val sum = u_max.checked_add(five)
    match sum {
        Option::Some(v) => println(v),
        Option::None => println("u64 add overflows"),
    }

    val diff = five.checked_sub(ten)
    match diff {
        Option::Some(v) => println(v),
        Option::None => println("u64 sub underflows"),
    }

    val product = ten.checked_mul(ten)
    match product {
        Option::Some(v) => println(v),
        Option::None => println("u64 mul overflows"),
    }

    val quotient = ten.checked_div(five)
    match quotient {
        Option::Some(v) => println(v),
        Option::None => println("division by zero"),
    }

    # Clamping instead of reporting: the classic `a - b` that must not
    # wrap past zero.
    val clamped = five.saturating_sub(ten)
    println(clamped)
    val piled = u_max.saturating_add(ten)
    println(piled)

    val i_min: i64 = -9223372036854775808i64
    val minus_one: i64 = -1i64
    val two: i64 = 2i64

    # `MIN / -1` has no representable result, so the operator traps;
    # `checked_div` answers `None` rather than stopping the program.
    val neg = i_min.checked_div(minus_one)
    match neg {
        Option::Some(v) => println(v),
        Option::None => println("i64 div overflows"),
    }

    val doubled = i_min.checked_mul(two)
    match doubled {
        Option::Some(v) => println(v),
        Option::None => println("i64 mul overflows"),
    }

    val floor = i_min.saturating_sub(two)
    println(floor)

    # The same calls at a narrow width (RUNTIME-TRAP-NARROW). The
    # operator `-` below zero traps here as it does for `u64`
    # (NARROW-UNSIGNED-SUB), so `checked_sub` is how to ask instead.
    val byte: u8 = 5u8
    val bigger: u8 = 10u8

    val byte_diff = byte.checked_sub(bigger)
    match byte_diff {
        Option::Some(v) => println(v),
        Option::None => println("u8 sub underflows"),
    }
    println(byte.saturating_sub(bigger))

    val byte_max: u8 = 255u8
    val byte_sum = byte_max.checked_add(byte)
    match byte_sum {
        Option::Some(v) => println(v),
        Option::None => println("u8 add overflows"),
    }
    println(byte_max.saturating_mul(byte))

    # `saturating_mul` is the one method the split traits never had
    # for a signed type. It clamps to whichever bound the true
    # product ran past, so `MIN * -1` lands on MAX rather than
    # wrapping back to MIN.
    val small_min: i16 = -32768i16
    val minus_one_16: i16 = -1i16
    val three_16: i16 = 3i16
    println(small_min.saturating_mul(minus_one_16))
    println(small_min.saturating_mul(three_16))

    val quad: i32 = 2147483647i32
    val quad_over = quad.checked_mul(three_16 as i32)
    match quad_over {
        Option::Some(v) => println(v),
        Option::None => println("i32 mul overflows"),
    }

    0u64
}
