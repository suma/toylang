# Bytes as hexadecimal text (STDLIB-SERIALIZE §5).
#
# **Output is lower case; input accepts either case.** Everywhere
# else this stdlib refuses to guess (`parse::to_u64` will not trim a
# space, and the JSON reader takes no extensions), but the case of a
# hex digit carries no ambiguity at all -- `FF` and `ff` cannot mean
# two different things -- and every tool that emits hex picks its own
# case.
#
# Nothing else is accepted: no whitespace, no `0x` prefix, no
# separators. Those are the ones where being generous costs
# something, since a decoder that skips whitespace cannot tell a
# truncated file from a formatted one.
#
# Every call this module makes to its own functions is qualified with
# `hex::`, because a bare name in a stdlib body resolves to a user
# function of the same name (STDLIB-FN-SHADOWED-BY-USER-FN), and
# `digit` is a name a program is likely to use.

# The digit for a nibble, lower case.
fn nibble(v: u64) -> u8 {
    if v < 10u64 {
        ('0' + v) as u8
    } else {
        ('a' + (v - 10u64)) as u8
    }
}

# The value of a hex digit, or 16 for anything else -- 16 rather than
# an `Option<u8>` because this is the inner loop of the decoder and
# the caller checks it immediately.
fn digit(b: u8) -> u64 {
    val c: u64 = b as u64
    if c >= '0' && c <= '9' {
        c - '0'
    } elif c >= 'a' && c <= 'f' {
        c - 'a' + 10u64
    } elif c >= 'A' && c <= 'F' {
        c - 'A' + 10u64
    } else {
        16u64
    }
}

# Two lower-case digits per byte, most significant first. An empty
# input gives an empty string.
pub fn encode(bytes: &Vec<u8>) -> String {
    val n: u64 = bytes.size()
    var out: String = String::new()
    var i: u64 = 0u64
    while i < n {
        val b: u64 = bytes.get(i) as u64
        out.push(hex::nibble(b / 16u64))
        out.push(hex::nibble(b % 16u64))
        i = i + 1u64
    }
    out
}

# The bytes of `s`, or where it stopped being hexadecimal.
#
# An empty string decodes to an empty `Vec` -- that is a length of
# zero bytes, not a failure. An odd number of digits is `BadLength`:
# the last digit is half of a byte and there is no way to know which
# half is missing.
pub fn decode(s: str) -> Result<Vec<u8>, CodecError> {
    val text: String = String::from_str(s)
    val n: u64 = text.size()
    if n % 2u64 != 0u64 {
        return Result::Err(CodecError::BadLength)
    }
    var out: Vec<u8> = Vec::with_capacity(n / 2u64)
    var i: u64 = 0u64
    while i < n {
        val hi: u64 = hex::digit(text.get(i))
        if hi == 16u64 {
            return Result::Err(CodecError::Invalid(i))
        }
        val lo: u64 = hex::digit(text.get(i + 1u64))
        if lo == 16u64 {
            return Result::Err(CodecError::Invalid(i + 1u64))
        }
        out.push((hi * 16u64 + lo) as u8)
        i = i + 2u64
    }
    Result::Ok(out)
}
