# LSZ1 -- the compression the archives are written in.
#
# It is LZ77 in its plainest form: a 64 KiB window, a single hash
# table of most-recent positions, greedy matching, and LZ4-shaped
# tokens. There is no entropy coding stage, which is what keeps the
# decoder to about forty lines.
#
# **Why not gzip.** The language has no compression library
# (RUNTIME_GAPS.md G7) and a deflate-compatible writer needs Huffman
# tables and a bit-level layout -- more work than the log store it
# would serve. We write and read these archives ourselves, so the
# format only has to be honest, not standard. What that costs is
# `zcat`: the export to a standard format is a separate job, listed
# in OVERVIEW.md as out of scope for now.
#
# **The token.** One byte, two nibbles: literal length in the high
# four bits, match length minus 4 in the low four. Either nibble of
# 15 means "and a varint follows with the rest". After the literals
# comes a two-byte little-endian offset, then the match's extra
# length. The last sequence in a frame is literals only and carries
# no offset -- the decoder knows it is done because the frame header
# said how many bytes to produce.
#
# **The bounds are contracts, not comments.** Every function that
# indexes a window says what it needs with `requires`. A caller that
# breaks one stops at the call site with the clause named, instead of
# reading a byte that belongs to somebody else -- and the same clause
# lets the bounds check inside `Span::get` be dropped
# (CONTRACT-ELISION), so stating the rule costs nothing at run time.
#
# **The window does not cross a frame.** Each frame decompresses on
# its own, so a query can expand the two frames it needs out of a
# hundred (STORAGE_FORMAT.md §3).

pub fn hash_bits() -> u64 { 15u64 }
pub fn hash_size() -> u64 { 32768u64 }
pub fn min_match() -> u64 { 4u64 }
pub fn max_offset() -> u64 { 65535u64 }

# How many bytes agree at `cand` and `pos`, counted from zero.
#
# Counting from zero rather than from `min_match()` means the
# four-byte test and the extension are the same comparison: a caller
# reads "fewer than four" as "not a match" and never walks those
# bytes twice.
#
# The scalar reference. `match_len` below must agree with it byte
# for byte -- if the two ever disagree the compressed output
# differs, which is why the property test compares them directly
# rather than only checking that decompression works.
pub fn match_len_scalar(src: Span<u8>, cand: u64, pos: u64, end: u64) -> u64 {
    var m = 0u64
    while pos + m < end {
        val a: u8 = src.get(cand + m)
        val b: u8 = src.get(pos + m)
        if a != b { break }
        m = m + 1u64
    }
    m
}

# The same answer, sixteen bytes at a time.
#
# `__simd_all` asks the cheap question -- "did all sixteen
# agree?" -- and only a block that failed pays for
# `__simd_bitmask`, whose lowest set bit is the first lane that
# differed (SIMD.md §3 P1/P5). Log data makes this worth having:
# repeated lines produce matches hundreds of bytes long, and the
# scalar loop walks them one byte at a time.
pub unsafe fn match_len(src: Span<u8>, cand: u64, pos: u64, end: u64) -> u64 {
    val p = src.as_raw()
    var m = 0u64
    # A block compare may read up to 16 bytes past `pos + m`, so
    # it stops early enough that the scalar tail covers the rest.
    while pos + m + 16u64 <= end {
        val a: u8x16 = __simd_load(p, cand + m)
        val b: u8x16 = __simd_load(p, pos + m)
        val eq = a == b
        if __simd_all(eq) {
            m = m + 16u64
        } else {
            val bits = __simd_bitmask(eq)
            # `bitmask` sets a bit per *matching* lane, so the
            # first difference is the first zero: invert, then
            # count the zeros below it.
            val first = (~bits & 0xFFFFu64).trailing_zeros() as u64
            return m + first
        }
    }
    while pos + m < end {
        val a: u8 = src.get(cand + m)
        val b: u8 = src.get(pos + m)
        if a != b { break }
        m = m + 1u64
    }
    m
}

# The byte order the hash reads its four bytes in, gathered for four
# *overlapping* windows at once: bytes 0..3, 1..4, 2..5 and 3..6 of one
# sixteen-byte load. `__simd_swizzle` indexes with values, so this is a
# table, built once and carried into the loop.
pub unsafe fn window_index() -> u8x16 {
    var v: u8x16 = __simd_splat(0u8)
    v = __simd_insert(v, 0u64, 0u8)    v = __simd_insert(v, 1u64, 1u8)
    v = __simd_insert(v, 2u64, 2u8)    v = __simd_insert(v, 3u64, 3u8)
    v = __simd_insert(v, 4u64, 1u8)    v = __simd_insert(v, 5u64, 2u8)
    v = __simd_insert(v, 6u64, 3u8)    v = __simd_insert(v, 7u64, 4u8)
    v = __simd_insert(v, 8u64, 2u8)    v = __simd_insert(v, 9u64, 3u8)
    v = __simd_insert(v, 10u64, 4u8)   v = __simd_insert(v, 11u64, 5u8)
    v = __simd_insert(v, 12u64, 3u8)   v = __simd_insert(v, 13u64, 4u8)
    v = __simd_insert(v, 14u64, 5u8)   v = __simd_insert(v, 15u64, 6u8)
    v
}

pub fn hash_multiplier() -> i32 { 2654435761u64 as i32 }

# The hashes of the four positions starting at `at`, in one pass.
#
# The scalar version reads four bytes per position -- sixteen loads
# and four multiplies for these four answers. This does one load, one
# swizzle, one multiply and one shift.
#
# **The shift is arithmetic, not logical**, because `i32x4` lanes are
# signed and the language has one `>>`. That is harmless here: the
# hash keeps the low fifteen bits, and sign extension only ever
# reaches bits at or above fifteen, which the mask removes.
pub unsafe fn hash4x(src: Span<u8>, at: u64, idx: u8x16, factor: i32x4, low15: i32x4) -> i32x4
    requires at + 16u64 <= src.len()
{
    val p = src.as_raw()
    val block: u8x16 = __simd_load(p, at)
    val gathered = __simd_swizzle(block, idx)
    val words: i32x4 = __simd_bitcast(gathered)
    val prod = words * factor
    # Each step is bound on purpose. A line ending in an identifier
    # and a next line starting with `(` are read as one call across
    # the newline -- `sh & low15` written as `(prod >> 17u64) & low15`
    # would be parsed as `factor(prod >> 17u64)`.
    val sh = prod >> 17u64
    val h = sh & low15
    h
}

pub struct Lsz {
    table: Vec<u32>,
}

impl Lsz {
    pub fn new() -> Self {
        var t: Vec<u32> = Vec::with_capacity(hash_size())
        var i: u64 = 0u64
        while i < hash_size() {
            t.push(0u32)
            i = i + 1u64
        }
        Lsz { table: t }
    }

    # Forget every position. Called between frames, because a match
    # may not reach across one.
    pub fn reset(&mut self) {
        var i: u64 = 0u64
        while i < hash_size() {
            self.table.set(i, 0u32)
            i = i + 1u64
        }
    }

    fn hash4(&self, src: Span<u8>, at: u64) -> u64
        requires at + 4u64 <= src.len()
    {
        val b0: u8 = src.get(at)
        val b1: u8 = src.get(at + 1u64)
        val b2: u8 = src.get(at + 2u64)
        val b3: u8 = src.get(at + 3u64)
        val v = (b0 as u64) | ((b1 as u64) << 8u64) | ((b2 as u64) << 16u64) | ((b3 as u64) << 24u64)
        # Knuth's multiplicative hash, kept inside 32 bits: `*` wraps
        # rather than trapping (RUNTIME-TRAP), which is what we want.
        val m = (v * 2654435761u64) & 0xFFFFFFFFu64
        (m >> (32u64 - hash_bits())) & (hash_size() - 1u64)
    }

    # Compress `len` bytes of `src` starting at `from` into `out`.
    # Answers how many bytes were appended.
    pub fn encode(&mut self, src: Span<u8>, from: u64, len: u64, out: &mut ByteWriter) -> u64
        requires from + len <= src.len()
    {
        val before = out.len()
        self.reset()
        val end = from + len
        val idx = window_index()
        val factor: i32x4 = __simd_splat(hash_multiplier())
        val low15: i32x4 = __simd_splat(32767i32)
        var pos = from
        var anchor = from
        while pos + min_match() <= end {
            # Four hashes at a time while there is a full window to
            # load; the tail falls back to the scalar hash, which is
            # also the reference the batch is checked against.
            var h0: u64 = 0u64
            var h1: u64 = 0u64
            var h2: u64 = 0u64
            var h3: u64 = 0u64
            var batch: u64 = 1u64
            if pos + 16u64 <= end {
                val hv = hash4x(src, pos, idx, factor, low15)
                # The lane index must be a literal, so the four are
                # spelled out rather than looped over.
                h0 = __simd_extract(hv, 0u64) as u64
                h1 = __simd_extract(hv, 1u64) as u64
                h2 = __simd_extract(hv, 2u64) as u64
                h3 = __simd_extract(hv, 3u64) as u64
                batch = 4u64
            } else {
                h0 = self.hash4(src, pos)
            }

            var k: u64 = 0u64
            var advanced = false
            while k < batch && !advanced {
                val at = pos + k
                if at + min_match() > end {
                    k = batch
                } else {
                    var h: u64 = h0
                    if k == 1u64 { h = h1 }
                    if k == 2u64 { h = h2 }
                    if k == 3u64 { h = h3 }

                    val slot: u32 = self.table.get(h)
                    self.table.set(h, (at + 1u64) as u32)

                    var matched = 0u64
                    if slot != 0u32 {
                        val cand = (slot as u64) - 1u64
                        val dist = at - cand
                        if cand >= from && dist <= max_offset() && dist > 0u64 {
                            # The four-byte check and the extension are
                            # the same comparison, so they are one call:
                            # `match_len` answers 0..3 for "not a match".
                            matched = match_len(src, cand, at, end)
                            if matched < min_match() { matched = 0u64 }
                        }
                    }

                    if matched >= min_match() {
                        val dist = at - ((slot as u64) - 1u64)
                        val litlen = at - anchor
                        val extra = matched - min_match()

                        var token: u64 = 0u64
                        if litlen >= 15u64 { token = 15u64 << 4u64 } else { token = litlen << 4u64 }
                        if extra >= 15u64 { token = token | 15u64 } else { token = token | extra }
                        out.put_u8(token as u8)
                        if litlen >= 15u64 { out.put_varint(litlen - 15u64) }
                        out.reserve(litlen)
                        out.put_span_fast(src, anchor, litlen)
                        out.put_u16(dist)
                        if extra >= 15u64 { out.put_varint(extra - 15u64) }

                        pos = at + matched
                        anchor = pos
                        advanced = true
                    } else {
                        k = k + 1u64
                    }
                }
            }
            if !advanced { pos = pos + k }
        }

        # The tail is always literals, and carries no offset.
        val litlen = end - anchor
        if litlen > 0u64 {
            var token: u64 = 0u64
            if litlen >= 15u64 { token = 15u64 << 4u64 } else { token = litlen << 4u64 }
            out.put_u8(token as u8)
            if litlen >= 15u64 { out.put_varint(litlen - 15u64) }
            out.reserve(litlen)
            out.put_span_fast(src, anchor, litlen)
        }
        out.len() - before
    }
}

# Expand `clen` bytes at `from` into `out`, producing exactly
# `raw_len` bytes. Answers false when the stream ran out early, which
# is how a truncated or corrupt frame is caught before it becomes
# wrong records.
pub fn decode_frame(src: Span<u8>, from: u64, clen: u64, raw_len: u64, out: &mut ByteWriter) -> bool
    requires from + clen <= src.len()
{
    val base = out.len()
    var rd = ByteReader::new(from + clen)
    rd.seek(from)
    var produced: u64 = 0u64
    var ok = true
    while produced < raw_len && ok {
        if rd.remaining() == 0u64 {
            ok = false
        } else {
            val token = rd.take_u8(src) as u64
            var litlen = token >> 4u64
            if litlen == 15u64 {
                val more = rd.take_varint(src)
                litlen = litlen + more
            }
            if litlen > 0u64 {
                if rd.remaining() < litlen {
                    ok = false
                } else {
                    out.reserve(litlen)
                    out.put_span_fast(src, rd.position(), litlen)
                    rd.seek(rd.position() + litlen)
                    produced = produced + litlen
                }
            }
            if ok && produced < raw_len {
                if rd.remaining() < 2u64 {
                    ok = false
                } else {
                    val dist = rd.take_u16(src)
                    var mlen = (token & 15u64) + min_match()
                    if (token & 15u64) == 15u64 {
                        val more = rd.take_varint(src)
                        mlen = mlen + more
                    }
                    if dist == 0u64 || dist > produced {
                        ok = false
                    } else {
                        val from_at = base + produced - dist
                        if dist >= 16u64 {
                            # Far enough back that a block copy never
                            # reads a byte this call has not written.
                            out.reserve(mlen)
                            out.append_from_self(from_at, mlen)
                        } else {
                            # A close match is a run: `aaaa...` is
                            # encoded as distance 1, and every byte
                            # read here is one this loop just wrote.
                            # Byte at a time is not slow here, it is
                            # the definition.
                            var k: u64 = 0u64
                            while k < mlen {
                                val b: u8 = out.byte_at(from_at + k)
                                out.put_u8(b)
                                k = k + 1u64
                            }
                        }
                        produced = produced + mlen
                    }
                }
            }
        }
    }
    if produced != raw_len { ok = false }
    ok
}
