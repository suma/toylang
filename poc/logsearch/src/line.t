# Splitting a byte buffer into lines.
#
# A line is a *window* into the caller's buffer -- an offset and a
# length -- never a copy. Nothing here allocates, which is what lets
# the ingest path stay allocation-free: on this runtime a freed byte
# is never handed back, so a `String` per line would turn every
# megabyte read into a megabyte of permanent memory (MEMORY.md).
#
# **The buffer arrives as a `Span<u8>`, not as a `&Vec<u8>`.** That is
# forced by the AOT backend: a compound *field* cannot be passed as a
# call argument (`self.buf` would fail with "call argument produced no
# value"), while a `Span` bound to a local can. The owner hands out a
# window; the scanner reads through it.
#
# The scan is deliberately scalar. Its vectorised twin is 26x faster
# (SIMD.md S1) and lands later behind this same interface, with this
# version kept as the reference the property tests compare against.

# One line's extent in the buffer it came from.
pub struct Line {
    start: u64,
    len: u64,
}

impl Line {
    pub fn end(&self) -> u64 { self.start + self.len }
    pub fn is_empty(&self) -> bool { self.len == 0u64 }
}

# A cursor over a buffer holding `len` valid bytes.
pub struct LineScan {
    pos: u64,
    len: u64,
}

impl LineScan {
    pub fn new(len: u64) -> Self {
        LineScan { pos: 0u64, len: len }
    }

    # Point the cursor back at the start of a buffer of `len` bytes.
    pub fn reset(&mut self, len: u64) {
        self.pos = 0u64
        self.len = len
    }

    pub fn done(&self) -> bool { self.pos >= self.len }
    pub fn position(&self) -> u64 { self.pos }

    # The next line, with its terminator removed: the `\n`, and a
    # `\r` in front of it. The last line counts even without a
    # newline, and an empty line is a line -- callers that want to
    # skip blanks say so themselves.
    pub fn next(&mut self, w: Span<u8>) -> Option<Line> {
        if self.pos >= self.len {
            return Option::None
        }
        val start = self.pos
        var i = start
        while i < self.len {
            val b: u8 = w.get(i)
            if b == 10u8 { break }
            i = i + 1u64
        }
        var end = i
        if end > start {
            val prev: u8 = w.get(end - 1u64)
            if prev == 13u8 { end = end - 1u64 }
        }
        self.pos = i + 1u64
        val out = Line { start: start, len: end - start }
        Option::Some(out)
    }
}
