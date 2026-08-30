# RUNTIME-LIB P0-B: reading numbers back out of text.
#
# `__builtin_to_string` and string interpolation turn numbers into
# text; `parse::` is the direction that was missing, which is what
# closes the loop for anything read from `io::read_line()` /
# `io::arg(i)` / `io::read_file(path)`.
#
# The grammar is narrow on purpose (`docs/language.md` -> "Parsing
# numbers"): decimal only, no surrounding whitespace, no `inf`, and a
# value that does not fit is `Err(Overflow)` rather than a wrapped or
# infinite number.

fn read_u64(s: str) {
    val r = parse::to_u64(s)
    match r {
        Result::Ok(v) => { println("u64  [{s}] -> {v}") }
        Result::Err(e) => { println("u64  [{s}] -> {e}") }
    }
}

fn read_i64(s: str) {
    val r = parse::to_i64(s)
    match r {
        Result::Ok(v) => { println("i64  [{s}] -> {v}") }
        Result::Err(e) => { println("i64  [{s}] -> {e}") }
    }
}

fn read_f64(s: str) {
    val r = parse::to_f64(s)
    match r {
        Result::Ok(v) => { println("f64  [{s}] -> {v}") }
        Result::Err(e) => { println("f64  [{s}] -> {e}") }
    }
}

fn main() -> u64 {
    read_u64("42")
    read_u64("+7")
    read_u64("-1")                     # a negative number is not a u64
    read_u64(" 42")                    # not trimmed: call `.trim()` first
    read_u64("18446744073709551616")   # u64::MAX + 1

    read_i64("-9223372036854775808")   # i64::MIN survives the round trip
    read_i64("0x10")                   # decimal only

    read_f64("2.5e3")                  # exponents, which literals lack
    read_f64("0.1")
    read_f64("1e999")                  # too large: an error, not infinity

    # The round trip: whatever the language prints, it reads back.
    val n: u64 = 1234567u64
    val text: str = "{n}"
    val back = parse::to_u64(text)
    match back {
        Result::Ok(v) => { println("round trip: {v}") }
        Result::Err(e) => { println("round trip failed: {e}") }
    }

    # `?` works too, since the failure is an ordinary `Result`.
    val total = sum_of("10", "32")
    match total {
        Result::Ok(v) => { println("10 + 32 = {v}") }
        Result::Err(e) => { println("sum failed: {e}") }
    }
    0u64
}

fn sum_of(a: str, b: str) -> Result<u64, ParseError> {
    val x: u64 = parse::to_u64(a)?
    val y: u64 = parse::to_u64(b)?
    Result::Ok(x + y)
}
