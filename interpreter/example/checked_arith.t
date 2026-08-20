# Overflow-aware integer arithmetic from `core/std/checked.t`
# (RUNTIME-TRAP). The operators wrap; these methods report the
# overflow instead — as an `Option` (`checked_*`) or by clamping to
# the type's bound (`saturating_*`).
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

    0u64
}
