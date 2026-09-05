package std.parse

# String -> number parsing (RUNTIME-LIB P0-B).
#
# The pair of `__builtin_to_string` and string interpolation: without
# it a program can print a number but cannot read one back, so
# `io::read_line()` / `io::arg(i)` / `io::read_file(path)` all dead-end
# at `str`.
#
#     val n = parse::to_u64("42")
#     match n {
#         Result::Ok(v) => v,
#         Result::Err(e) => { eprintln("bad number: {e}") 0u64 }
#     }
#
# The grammar is deliberately narrow and identical on every backend
# (see `docs/language.md` -> "Parsing numbers"):
#
#   - **no surrounding whitespace** — `" 42"` is `Err(Invalid)`. Call
#     `.trim()` first when the input comes from a line of text; a
#     parser that trims silently cannot be un-trimmed by a caller who
#     wanted the strictness.
#   - **decimal only** — no `0x` / `0b` prefix, no `_` separators
#     (those are source-literal syntax, not input syntax).
#   - **sign**: `+` and `-` for `to_i64` / `to_f64`; `to_u64` takes a
#     leading `+` but reads `-1` as `Err(Invalid)` rather than
#     wrapping.
#   - **no `inf` / `nan` spellings** for `to_f64`. A value too large
#     to represent is `Err(Overflow)`, not `Ok(infinity)`.
#
# `to_f64` is the one that crosses an extern boundary: the digits are
# validated here (so every backend accepts exactly the same strings)
# and only then handed to the host's correctly-rounded decimal ->
# binary conversion, which is not something to reimplement in toylang.
# The integer and bool parsers are pure toylang and need no runtime
# support at all.

extern fn __extern_parse_f64(s: str) -> f64 from "toylang_rt" as "toy_parse_f64"
extern fn __extern_parse_f64_status() -> u64 from "toylang_rt" as "toy_parse_f64_status"

# Why a string could not be read as a number. Exhaustively matchable,
# and rendered by `Display` so `println(e)` says what happened.
pub enum ParseError {
    Empty,    # the input had no characters at all
    Invalid,  # the input is not in the accepted grammar
    Overflow, # the value does not fit the target type
}

impl Display for ParseError {
    fn to_str(&self) -> str {
        match self {
            ParseError::Empty => "empty input",
            ParseError::Invalid => "invalid number",
            ParseError::Overflow => "out of range",
        }
    }
}

# Digits `[start, end)` of `b` as a `u64`, rejecting a non-digit byte
# and a value that does not fit. The caller has already dealt with any
# sign, so an empty range here means the input was a bare sign.
fn digits_to_u64(b: &String, start: u64, end: u64) -> Result<u64, ParseError> {
    if start >= end {
        return Result::Err(ParseError::Invalid)
    }
    var acc: u64 = 0u64
    var i: u64 = start
    while i < end {
        val c: u8 = b.get(i)
        # STDLIB-TEXT §6: the comparison chain this used to spell by
        # hand now has a name, and one place to be right.
        val d: Option<u32> = c.digit_value(10u32)
        val digit: u64 = match d {
            Option::Some(v) => v as u64,
            Option::None => { return Result::Err(ParseError::Invalid) }
        }
        val scaled = acc.checked_mul(10u64)
        match scaled {
            Option::Some(v) => { acc = v }
            Option::None => { return Result::Err(ParseError::Overflow) }
        }
        val summed = acc.checked_add(digit)
        match summed {
            Option::Some(v) => { acc = v }
            Option::None => { return Result::Err(ParseError::Overflow) }
        }
        i = i + 1u64
    }
    Result::Ok(acc)
}

# Read `s` as an unsigned decimal integer. A leading `+` is accepted;
# a leading `-` is `Err(Invalid)` even for `-0`, since a negative
# number is not a `u64` and wrapping it would hide the mistake.
pub fn to_u64(s: str) -> Result<u64, ParseError> {
    val b = String::from_str(s)
    val n: u64 = b.size()
    if n == 0u64 {
        return Result::Err(ParseError::Empty)
    }
    var start: u64 = 0u64
    if b.get(0u64) == '+' {
        start = 1u64
    }
    val r: Result<u64, ParseError> = digits_to_u64(&b, start, n)
    r
}

# Read `s` as a signed decimal integer. `-9223372036854775808` is
# accepted: the magnitude is read as a `u64` and only then compared
# against the bound for its sign, so the one value whose positive form
# does not fit is not lost.
pub fn to_i64(s: str) -> Result<i64, ParseError> {
    val b = String::from_str(s)
    val n: u64 = b.size()
    if n == 0u64 {
        return Result::Err(ParseError::Empty)
    }
    val first: u8 = b.get(0u64)
    var start: u64 = 0u64
    var negative: bool = false
    if first == '-' {
        negative = true
        start = 1u64
    } elif first == '+' {
        start = 1u64
    }
    val magnitude: Result<u64, ParseError> = digits_to_u64(&b, start, n)
    match magnitude {
        Result::Ok(m) => {
            if negative {
                # 2^63 is `i64::MIN` once the u64 bit pattern is read
                # as signed, which is exactly the value wanted.
                if m > 9223372036854775808u64 {
                    Result::Err(ParseError::Overflow)
                } else {
                    val v: i64 = 0i64 - (m as i64)
                    Result::Ok(v)
                }
            } else {
                if m > 9223372036854775807u64 {
                    Result::Err(ParseError::Overflow)
                } else {
                    Result::Ok(m as i64)
                }
            }
        }
        Result::Err(e) => Result::Err(e),
    }
}

# Whether `s[start..end]` is one or more digits.
fn all_digits(b: &String, start: u64, end: u64) -> bool {
    if start >= end {
        return false
    }
    var i: u64 = start
    while i < end {
        val c: u8 = b.get(i)
        if c.is_ascii_digit() == false {
            return false
        }
        i = i + 1u64
    }
    true
}

# Whether `s` matches `[+-]? ( digits ( '.' digits? )? | '.' digits )
# ( [eE] [+-]? digits )?`. Checked here rather than left to the host
# so that every backend accepts the same set of strings — a C
# `strtod` would also take `inf`, `nan`, hex floats and leading
# whitespace, none of which this language's grammar has.
fn is_decimal(b: &String, n: u64) -> bool {
    var i: u64 = 0u64
    if n == 0u64 {
        return false
    }
    val first: u8 = b.get(0u64)
    if first == '+' || first == '-' {
        i = 1u64
    }
    # Mantissa: digits, then optionally `.` and more digits.
    var int_end: u64 = i
    while int_end < n {
        val c: u8 = b.get(int_end)
        if c.is_ascii_digit() == false {
            break
        }
        int_end = int_end + 1u64
    }
    val int_digits: u64 = int_end - i
    var cursor: u64 = int_end
    var frac_digits: u64 = 0u64
    if cursor < n && b.get(cursor) == '.' {
        cursor = cursor + 1u64
        var frac_end: u64 = cursor
        while frac_end < n {
            val c: u8 = b.get(frac_end)
            if c.is_ascii_digit() == false {
                break
            }
            frac_end = frac_end + 1u64
        }
        frac_digits = frac_end - cursor
        cursor = frac_end
    }
    if int_digits == 0u64 && frac_digits == 0u64 {
        return false
    }
    # Exponent: `e` / `E`, an optional sign, then at least one digit.
    if cursor < n {
        val e: u8 = b.get(cursor)
        if e != 'e' && e != 'E' {
            return false
        }
        cursor = cursor + 1u64
        if cursor < n {
            val sign: u8 = b.get(cursor)
            if sign == '+' || sign == '-' {
                cursor = cursor + 1u64
            }
        }
        if !all_digits(&b, cursor, n) {
            return false
        }
        cursor = n
    }
    cursor == n
}

# Read `s` as a decimal floating-point number. The conversion of an
# accepted string is the host's (correctly rounded on every backend);
# a value too large to represent is `Err(Overflow)` rather than an
# infinity, so a caller cannot mistake "too big" for "very big".
pub fn to_f64(s: str) -> Result<f64, ParseError> {
    val b = String::from_str(s)
    val n: u64 = b.size()
    if n == 0u64 {
        return Result::Err(ParseError::Empty)
    }
    if !is_decimal(&b, n) {
        return Result::Err(ParseError::Invalid)
    }
    val v: f64 = __extern_parse_f64(s)
    val status: u64 = __extern_parse_f64_status()
    if status == 0u64 {
        Result::Ok(v)
    } elif status == 2u64 {
        Result::Err(ParseError::Overflow)
    } else {
        Result::Err(ParseError::Invalid)
    }
}

# Read `s` as a boolean. Exactly `true` or `false` — no case folding,
# no `1` / `0` / `yes`, because a parser that guesses is one a caller
# cannot make strict again.
pub fn to_bool(s: str) -> Result<bool, ParseError> {
    val b = String::from_str(s)
    if b.size() == 0u64 {
        return Result::Err(ParseError::Empty)
    }
    if s == "true" {
        Result::Ok(true)
    } elif s == "false" {
        Result::Ok(false)
    } else {
        Result::Err(ParseError::Invalid)
    }
}
