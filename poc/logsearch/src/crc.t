# CRC-32 (the IEEE polynomial, reflected -- what gzip, PNG and zip
# all agree on), used to notice a frame that came back wrong.
#
# The table is built once and lives in the struct. It cannot be a
# `const`: compile-time evaluation folds scalars only, so a 256-entry
# table has to be computed at start-up.
#
# The bounds each function needs are stated as `requires`. That is
# not decoration: a precondition is what lets the compiler drop the
# bounds check inside `Span::get` (CONTRACT-ELISION), so the safe
# spelling and the fast one are the same spelling here.
#
# **This is the one part of the archive path that cannot be
# vectorised** -- CRC needs carry-less multiply, which the language
# does not have (SIMD.md §6, RUNTIME_GAPS.md G7). A byte-at-a-time
# table lookup is the fastest shape available.

pub fn crc_seed() -> u64 { 0xFFFFFFFFu64 }

pub struct Crc32 {
    table: Vec<u32>,
}

impl Crc32 {
    pub fn new() -> Self {
        var t: Vec<u32> = Vec::with_capacity(256u64)
        var i: u64 = 0u64
        while i < 256u64 {
            var c: u64 = i
            var k: u64 = 0u64
            while k < 8u64 {
                if (c & 1u64) != 0u64 {
                    c = 0xEDB88320u64 ^ (c >> 1u64)
                } else {
                    c = c >> 1u64
                }
                k = k + 1u64
            }
            t.push(c as u32)
            i = i + 1u64
        }
        Crc32 { table: t }
    }

    # Fold `len` bytes of `src` into a running value. Start from
    # `crc_seed()` and finish with `finish`.
    pub fn update(&self, crc: u64, src: Span<u8>, from: u64, len: u64) -> u64
        requires from + len <= src.len()
        requires crc <= 0xFFFFFFFFu64
    {
        var c = crc
        var i: u64 = 0u64
        while i < len {
            val b: u8 = src.get(from + i)
            val idx = (c ^ (b as u64)) & 0xFFu64
            val e: u32 = self.table.get(idx)
            c = (e as u64) ^ (c >> 8u64)
            i = i + 1u64
        }
        c
    }

    pub fn finish(&self, crc: u64) -> u64
        requires crc <= 0xFFFFFFFFu64
    { (crc ^ 0xFFFFFFFFu64) & 0xFFFFFFFFu64 }

    # The whole answer for one window, seed to finish.
    pub fn of(&self, src: Span<u8>, from: u64, len: u64) -> u64
        requires from + len <= src.len()
    {
        val c = self.update(crc_seed(), src, from, len)
        self.finish(c)
    }
}
