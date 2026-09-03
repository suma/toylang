# Bytes as base64 text (STDLIB-SERIALIZE §6).
#
# RFC 4648's standard alphabet (`A-Z a-z 0-9 + /`) with `=` padding,
# and nothing else:
#
# * **The URL-safe alphabet (`-_`) is not here.** It is a different
#   encoding with the same name, and guessing which one an input uses
#   means accepting text that was meant to be rejected. When a URL or
#   a JWT needs it, it goes in as its own pair of functions.
# * **Padding is required.** Unpadded base64 cannot be told from
#   truncated base64, which is the failure this decoder exists to
#   report.
# * **No whitespace**, so MIME's 76-column wrapping is not accepted.
#   A decoder that skips newlines cannot tell a formatted file from a
#   corrupted one.
# * **The unused bits of the last group must be zero.** `QQ==` is
#   `A`; `QR==` encodes nothing -- two spellings of one byte -- and
#   is `Invalid`.
#
# Names are qualified with `base64::` for the reason given in
# `hex.t`: a bare name in a stdlib body can resolve to a user
# function (STDLIB-FN-SHADOWED-BY-USER-FN).

# The character for a 6-bit group.
fn symbol(v: u64) -> u8 {
    if v < 26u64 {
        ('A' + v) as u8
    } elif v < 52u64 {
        ('a' + (v - 26u64)) as u8
    } elif v < 62u64 {
        ('0' + (v - 52u64)) as u8
    } elif v == 62u64 {
        # `as u8` on these two only because a char literal does not
        # take its type from a return position or from a sibling `if`
        # arm (CHAR-LITERAL-NUM covers annotations, arguments,
        # comparisons and arithmetic). The three arms above need the
        # cast anyway.
        '+' as u8
    } else {
        '/' as u8
    }
}

# The 6-bit value of a character, or 64 for anything outside the
# alphabet (including `=`, which the caller handles by position).
fn value(b: u8) -> u64 {
    val c: u64 = b as u64
    if c >= 'A' && c <= 'Z' {
        c - 'A'
    } elif c >= 'a' && c <= 'z' {
        c - 'a' + 26u64
    } elif c >= '0' && c <= '9' {
        c - '0' + 52u64
    } elif c == '+' {
        62u64
    } elif c == '/' {
        63u64
    } else {
        64u64
    }
}

# Four characters per three bytes, padded to a multiple of four. An
# empty input gives an empty string.
pub fn encode(bytes: &Vec<u8>) -> String {
    val n: u64 = bytes.size()
    var out: String = String::new()
    var i: u64 = 0u64
    while i + 3u64 <= n {
        val a: u64 = bytes.get(i) as u64
        val b: u64 = bytes.get(i + 1u64) as u64
        val c: u64 = bytes.get(i + 2u64) as u64
        val group: u64 = a * 65536u64 + b * 256u64 + c
        out.push(base64::symbol(group / 262144u64))
        out.push(base64::symbol((group / 4096u64) % 64u64))
        out.push(base64::symbol((group / 64u64) % 64u64))
        out.push(base64::symbol(group % 64u64))
        i = i + 3u64
    }
    val rest: u64 = n - i
    if rest == 1u64 {
        val a: u64 = bytes.get(i) as u64
        out.push(base64::symbol(a / 4u64))
        out.push(base64::symbol((a % 4u64) * 16u64))
        out.push('=')
        out.push('=')
    } elif rest == 2u64 {
        val a: u64 = bytes.get(i) as u64
        val b: u64 = bytes.get(i + 1u64) as u64
        out.push(base64::symbol(a / 4u64))
        out.push(base64::symbol((a % 4u64) * 16u64 + b / 16u64))
        out.push(base64::symbol((b % 16u64) * 4u64))
        out.push('=')
    }
    out
}

# The bytes of `s`, or where it stopped being base64.
#
# An empty string decodes to an empty `Vec`. Any other length that is
# not a multiple of four is `BadLength` -- see the header on why
# unpadded input is not accepted.
pub fn decode(s: str) -> Result<Vec<u8>, CodecError> {
    val text: String = String::from_str(s)
    val n: u64 = text.size()
    if n % 4u64 != 0u64 {
        return Result::Err(CodecError::BadLength)
    }
    var out: Vec<u8> = Vec::with_capacity((n / 4u64) * 3u64)
    var i: u64 = 0u64
    while i < n {
        val c0: u8 = text.get(i)
        val c1: u8 = text.get(i + 1u64)
        val c2: u8 = text.get(i + 2u64)
        val c3: u8 = text.get(i + 3u64)
        val v0: u64 = base64::value(c0)
        val v1: u64 = base64::value(c1)
        if v0 == 64u64 {
            return Result::Err(CodecError::Invalid(i))
        }
        if v1 == 64u64 {
            return Result::Err(CodecError::Invalid(i + 1u64))
        }
        # Padding is only ever in the final group, and only in the
        # last two positions.
        val last: bool = i + 4u64 == n
        if c2 == '=' {
            if !last || c3 != '=' {
                return Result::Err(CodecError::Invalid(i + 2u64))
            }
            if v1 % 16u64 != 0u64 {
                return Result::Err(CodecError::Invalid(i + 1u64))
            }
            out.push((v0 * 4u64 + v1 / 16u64) as u8)
        } else {
            val v2: u64 = base64::value(c2)
            if v2 == 64u64 {
                return Result::Err(CodecError::Invalid(i + 2u64))
            }
            out.push((v0 * 4u64 + v1 / 16u64) as u8)
            if c3 == '=' {
                if !last {
                    return Result::Err(CodecError::Invalid(i + 3u64))
                }
                if v2 % 4u64 != 0u64 {
                    return Result::Err(CodecError::Invalid(i + 2u64))
                }
                out.push(((v1 % 16u64) * 16u64 + v2 / 4u64) as u8)
            } else {
                val v3: u64 = base64::value(c3)
                if v3 == 64u64 {
                    return Result::Err(CodecError::Invalid(i + 3u64))
                }
                out.push(((v1 % 16u64) * 16u64 + v2 / 4u64) as u8)
                out.push(((v2 % 4u64) * 64u64 + v3) as u8)
            }
        }
        i = i + 4u64
    }
    Result::Ok(out)
}
