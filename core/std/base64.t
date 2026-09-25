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
#
# **12 input bytes per pass** (SIMD.md strategy B). Like
# `hex::encode`, the output length is known up front, so the buffer
# is taken in one allocation and written through.
#
# The 3-byte-to-4-character regrouping is where this differs from
# hex, and it is why the vector path needs three shuffles:
#
#   1. `__simd_shuffle` spreads four 3-byte groups across four 32-bit
#      lanes, in the byte order that makes each lane read as
#      `(b0 << 16) | (b1 << 8) | b2`. The fourth byte of each lane
#      comes from a zero vector, which is what the second operand is
#      for.
#   2. `__simd_bitcast` reads that as `i32x4` so the four 6-bit
#      fields fall out with a shift and a mask each — the 16-bit lane
#      type the usual SSE formulation wants does not exist here, and
#      32-bit lanes reach the same place.
#   3. two more shuffles weave the four field vectors back into one
#      byte per 6-bit group, in order.
#
# The alphabet is then a chain of `__simd_select`s rather than a
# `__simd_swizzle`: the table has 64 entries, and a byte swizzle
# indexes 16. Five comparisons cost less than splitting the index.
#
# The stores land exactly: a pass needs `i + 16 <= n` (it loads 16
# bytes and consumes 12), and its 16 output characters go at
# `4i/3`, which stays inside the `ceil(n/3) * 4` the output needs.
pub fn encode(bytes: &Vec<u8>) -> String {
    val n: u64 = bytes.size()
    if n == 0u64 {
        val empty: String = String::new()
        return empty
    }
    val groups: u64 = (n + 2u64) / 3u64
    val out_len: u64 = groups * 4u64
    var out: String = String::with_capacity(out_len)
    val dst: Ptr<u8> = Ptr { addr: out.as_ptr() }
    val src: Ptr<u8> = Ptr { addr: bytes.as_ptr() }
    val zero: u8x16 = __simd_splat(0u8)
    val m63: i32x4 = __simd_splat(63i32)
    val lim25: u8x16 = __simd_splat(25u8)
    val lim51: u8x16 = __simd_splat(51u8)
    val lim62: u8x16 = __simd_splat(62u8)
    # `'A'`, `'a' - 26` and `'0' - 52`: the offset from a 6-bit value
    # to its character, per range.
    val off_upper: u8x16 = __simd_splat('A')
    val off_lower: u8x16 = __simd_splat(71u8)
    val off_digit: u8x16 = __simd_splat(4u8)
    val plus: u8x16 = __simd_splat('+')
    val slash: u8x16 = __simd_splat('/')
    var i: u64 = 0u64
    var j: u64 = 0u64
    while i + 16u64 <= n {
        val v: u8x16 = src.load16(i)
        val spread = __simd_shuffle(v, zero,
            [2u64, 1u64, 0u64, 16u64, 5u64, 4u64, 3u64, 16u64,
             8u64, 7u64, 6u64, 16u64, 11u64, 10u64, 9u64, 16u64])
        val w: i32x4 = __simd_bitcast(spread)
        val f0: u8x16 = __simd_bitcast((w >> 18u64) & m63)
        val f1: u8x16 = __simd_bitcast((w >> 12u64) & m63)
        val f2: u8x16 = __simd_bitcast((w >> 6u64) & m63)
        val f3: u8x16 = __simd_bitcast(w & m63)
        # Each field's value sits in byte `4g` of its vector, so the
        # weave picks bytes 0, 4, 8, 12 out of each.
        val pair01 = __simd_shuffle(f0, f1,
            [0u64, 16u64, 4u64, 20u64, 8u64, 24u64, 12u64, 28u64,
             0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
        val pair23 = __simd_shuffle(f2, f3,
            [0u64, 16u64, 4u64, 20u64, 8u64, 24u64, 12u64, 28u64,
             0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
        val idx = __simd_shuffle(pair01, pair23,
            [0u64, 1u64, 16u64, 17u64, 2u64, 3u64, 18u64, 19u64,
             4u64, 5u64, 20u64, 21u64, 6u64, 7u64, 22u64, 23u64])
        # Every lane computes every branch; the selects keep the one
        # its 6-bit value asked for. The arithmetic on the discarded
        # lanes wraps, which is defined and unobservable.
        var ch: u8x16 = idx + off_upper
        ch = __simd_select(idx > lim25, idx + off_lower, ch)
        ch = __simd_select(idx > lim51, idx - off_digit, ch)
        ch = __simd_select(idx == lim62, plus, ch)
        ch = __simd_select(idx > lim62, slash, ch)
        dst.store16(j, ch)
        i = i + 12u64
        j = j + 16u64
    }
    while i + 3u64 <= n {
        val b0: u8 = src.get(i)
        val b1: u8 = src.get(i + 1u64)
        val b2: u8 = src.get(i + 2u64)
        val group: u64 = (b0 as u64) * 65536u64 + (b1 as u64) * 256u64 + (b2 as u64)
        dst.set(j, base64::symbol(group / 262144u64))
        dst.set(j + 1u64, base64::symbol((group / 4096u64) % 64u64))
        dst.set(j + 2u64, base64::symbol((group / 64u64) % 64u64))
        dst.set(j + 3u64, base64::symbol(group % 64u64))
        i = i + 3u64
        j = j + 4u64
    }
    val rest: u64 = n - i
    val pad: u8 = '='
    if rest == 1u64 {
        val b0: u8 = src.get(i)
        val a: u64 = b0 as u64
        dst.set(j, base64::symbol(a / 4u64))
        dst.set(j + 1u64, base64::symbol((a % 4u64) * 16u64))
        dst.set(j + 2u64, pad)
        dst.set(j + 3u64, pad)
    } elif rest == 2u64 {
        val b0: u8 = src.get(i)
        val b1: u8 = src.get(i + 1u64)
        val a: u64 = b0 as u64
        val b: u64 = b1 as u64
        dst.set(j, base64::symbol(a / 4u64))
        dst.set(j + 1u64, base64::symbol((a % 4u64) * 16u64 + b / 16u64))
        dst.set(j + 2u64, base64::symbol((b % 16u64) * 4u64))
        dst.set(j + 3u64, pad)
    }
    out.set_size(out_len)
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
    if n >= 32u64 {
        # **16 input characters -> 12 output bytes per pass.**
        #
        # The bound is `i + 24 <= n`, not `i + 16 <= n`, for two
        # reasons that happen to coincide: padding only ever appears
        # in the final group, so leaving the last two groups to the
        # scalar loop keeps `=` out of the vector path entirely; and a
        # store writes all sixteen bytes while a pass only produces
        # twelve, so the four bytes past them must still be inside the
        # buffer.
        #
        # A bad character stops the vector loop rather than reporting
        # anything: the scalar loop below re-reads from the same `i`
        # and is the one that knows which of the four positions was
        # wrong, which is what `CodecError::Invalid` carries.
        val src: Ptr<u8> = Ptr { addr: text.as_ptr() }
        val dst: Ptr<u8> = Ptr { addr: out.as_ptr() }
        val ca: u8x16 = __simd_splat('A')
        val cz: u8x16 = __simd_splat('Z')
        val la: u8x16 = __simd_splat('a')
        val lz: u8x16 = __simd_splat('z')
        val d0: u8x16 = __simd_splat('0')
        val d9: u8x16 = __simd_splat('9')
        val cplus: u8x16 = __simd_splat('+')
        val cslash: u8x16 = __simd_splat('/')
        val n26: u8x16 = __simd_splat(26u8)
        val n52: u8x16 = __simd_splat(52u8)
        val n62: u8x16 = __simd_splat(62u8)
        val n63: u8x16 = __simd_splat(63u8)
        val zero: u8x16 = __simd_splat(0u8)
        var j: u64 = 0u64
        var scanning: bool = true
        while scanning && i + 24u64 <= n {
            val c: u8x16 = src.load16(i)
            val up = (c >= ca) & (c <= cz)
            val lw = (c >= la) & (c <= lz)
            val dg = (c >= d0) & (c <= d9)
            val pl = c == cplus
            val sl = c == cslash
            if __simd_all(((up | lw) | dg) | (pl | sl)) {
                var v: u8x16 = zero
                v = __simd_select(up, c - ca, v)
                v = __simd_select(lw, c - la + n26, v)
                v = __simd_select(dg, c - d0 + n52, v)
                v = __simd_select(pl, n62, v)
                v = __simd_select(sl, n63, v)
                # One vector per position within a group, so the
                # four 6-bit values of group `g` line up in lane `g`.
                val a0 = __simd_shuffle(v, v,
                    [0u64, 4u64, 8u64, 12u64, 0u64, 0u64, 0u64, 0u64,
                     0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
                val a1 = __simd_shuffle(v, v,
                    [1u64, 5u64, 9u64, 13u64, 0u64, 0u64, 0u64, 0u64,
                     0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
                val a2 = __simd_shuffle(v, v,
                    [2u64, 6u64, 10u64, 14u64, 0u64, 0u64, 0u64, 0u64,
                     0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
                val a3 = __simd_shuffle(v, v,
                    [3u64, 7u64, 11u64, 15u64, 0u64, 0u64, 0u64, 0u64,
                     0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
                val b0 = (a0 << 2u64) | (a1 >> 4u64)
                val b1 = (a1 << 4u64) | (a2 >> 2u64)
                val b2 = (a2 << 6u64) | a3
                val pair = __simd_shuffle(b0, b1,
                    [0u64, 16u64, 1u64, 17u64, 2u64, 18u64, 3u64, 19u64,
                     0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64])
                val bytes = __simd_shuffle(pair, b2,
                    [0u64, 1u64, 16u64, 2u64, 3u64, 17u64, 4u64, 5u64,
                     18u64, 6u64, 7u64, 19u64, 0u64, 0u64, 0u64, 0u64])
                dst.store16(j, bytes)
                i = i + 16u64
                j = j + 12u64
            } else {
                scanning = false
            }
        }
        out.set_size(j)
    }
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
