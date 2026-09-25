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
#
# **16 input bytes per pass** (SIMD.md strategy B). The output length
# is known before the first byte is read, so the buffer is taken in
# one allocation and written through rather than grown by `push` --
# on its own that is most of the win, exactly as it was for
# `String::to_upper`.
#
# The vector path is the reason `__simd_swizzle` and `__simd_shuffle`
# exist:
#
#   * the digit table is 16 bytes, so `__simd_swizzle(table, nibble)`
#     *is* the lookup -- one instruction for sixteen digits, with the
#     nibble as a runtime index;
#   * the two halves then have to interleave (`hi0 lo0 hi1 lo1 ...`),
#     which is a fixed permutation of the two vectors and so a
#     constant `__simd_shuffle` mask.
#
# The stores land exactly: `i + 16 <= n` gives `2i + 32 <= 2n`, so
# neither 16-byte store can reach past the output buffer, and the
# tail below writes the remaining bytes one at a time.
pub fn encode(bytes: &Vec<u8>) -> String {
    val n: u64 = bytes.size()
    if n == 0u64 {
        # Bound first: the compiled lanes only `return` a struct
        # through a bare identifier.
        val empty: String = String::new()
        return empty
    }
    val out_len: u64 = n * 2u64
    var out: String = String::with_capacity(out_len)
    val dst: Ptr<u8> = Ptr { addr: out.as_ptr() }
    val src: Ptr<u8> = Ptr { addr: bytes.as_ptr() }
    val digits: Ptr<u8> = Ptr { addr: __builtin_str_to_ptr("0123456789abcdef") }
    val table: u8x16 = digits.load16(0u64)
    val low: u8x16 = __simd_splat(0x0Fu8)
    var i: u64 = 0u64
    while i + 16u64 <= n {
        val v: u8x16 = src.load16(i)
        val hi_ch = __simd_swizzle(table, v >> 4u64)
        val lo_ch = __simd_swizzle(table, v & low)
        val first: u8x16 = __simd_shuffle(hi_ch, lo_ch,
            [0u64, 16u64, 1u64, 17u64, 2u64, 18u64, 3u64, 19u64,
             4u64, 20u64, 5u64, 21u64, 6u64, 22u64, 7u64, 23u64])
        val second: u8x16 = __simd_shuffle(hi_ch, lo_ch,
            [8u64, 24u64, 9u64, 25u64, 10u64, 26u64, 11u64, 27u64,
             12u64, 28u64, 13u64, 29u64, 14u64, 30u64, 15u64, 31u64])
        dst.store16(i * 2u64, first)
        dst.store16(i * 2u64 + 16u64, second)
        i = i + 16u64
    }
    while i < n {
        val byte: u8 = src.get(i)
        val b: u64 = byte as u64
        val hi_digit: u8 = hex::nibble(b / 16u64)
        val lo_digit: u8 = hex::nibble(b % 16u64)
        dst.set(i * 2u64, hi_digit)
        dst.set(i * 2u64 + 1u64, lo_digit)
        i = i + 1u64
    }
    out.set_size(out_len)
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
    if n >= 32u64 {
        # **32 input digits per pass**, giving one full 16-byte store.
        # Sixteen digits would only half-fill a vector, and a store
        # writes all sixteen bytes.
        #
        # The digit value is branch-free. `c | 0x20` folds `A-F` onto
        # `a-f`, so one comparison pair covers both cases, and the
        # select picks between `c - '0'` and `lower - 'a' + 10`. Both
        # arms are computed on every lane; the wrong one wraps, which
        # is defined and then discarded.
        #
        # Validity is checked for the whole chunk at once. On any bad
        # digit the vector loop simply stops and the scalar loop below
        # re-reads from the same `i` -- it is the one that knows *which*
        # digit was wrong, and reporting that index is the contract.
        val src: Ptr<u8> = Ptr { addr: text.as_ptr() }
        val dst: Ptr<u8> = Ptr { addr: out.as_ptr() }
        val case_bit: u8x16 = __simd_splat(0x20u8)
        val d0: u8x16 = __simd_splat('0')
        val d9: u8x16 = __simd_splat('9')
        val la: u8x16 = __simd_splat('a')
        val lf: u8x16 = __simd_splat('f')
        val ten: u8x16 = __simd_splat(10u8)
        var scanning: bool = true
        while scanning && i + 32u64 <= n {
            val c0: u8x16 = src.load16(i)
            val c1: u8x16 = src.load16(i + 16u64)
            val low0 = c0 | case_bit
            val low1 = c1 | case_bit
            val ok0 = ((c0 >= d0) & (c0 <= d9)) | ((low0 >= la) & (low0 <= lf))
            val ok1 = ((c1 >= d0) & (c1 <= d9)) | ((low1 >= la) & (low1 <= lf))
            if __simd_all(ok0 & ok1) {
                val n0 = __simd_select((c0 >= d0) & (c0 <= d9), c0 - d0, low0 - la + ten)
                val n1 = __simd_select((c1 >= d0) & (c1 <= d9), c1 - d0, low1 - la + ten)
                # Even digits are the high nibbles, odd ones the low.
                val hi = __simd_shuffle(n0, n1,
                    [0u64, 2u64, 4u64, 6u64, 8u64, 10u64, 12u64, 14u64,
                     16u64, 18u64, 20u64, 22u64, 24u64, 26u64, 28u64, 30u64])
                val lo = __simd_shuffle(n0, n1,
                    [1u64, 3u64, 5u64, 7u64, 9u64, 11u64, 13u64, 15u64,
                     17u64, 19u64, 21u64, 23u64, 25u64, 27u64, 29u64, 31u64])
                val packed: u8x16 = (hi << 4u64) | lo
                dst.store16(i / 2u64, packed)
                i = i + 32u64
            } else {
                scanning = false
            }
        }
        # The bytes the vector loop wrote are live; the tail appends
        # from here with the ordinary `push`.
        out.set_size(i / 2u64)
    }
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
