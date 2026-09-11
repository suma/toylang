# Reading one log file into a buffer that is reused for the next one.
#
# The buffer is allocated once, at start-up, and every file after the
# first lands in the same bytes. That is not an optimisation: this
# runtime never reuses a freed address, so a `Vec` per file would add
# the whole log directory to the process's permanent footprint
# (MEMORY.md).
#
# Reading is all-or-nothing. It was the only shape the language
# offered when this was written; `fs::File` can stream now, and the
# storage layer does (`segfile.t`), but a log file still arrives in
# one piece here because a line that straddles two reads has to be
# carried, and nothing in this path needs that yet. A file larger
# than the buffer therefore fills
# it and stops; `was_truncated` is how the caller notices, and it is
# reported rather than hidden, because a log search that quietly drops
# the tail of a file is worse than one that says it cannot read it.
#
# **This struct is deliberately narrow: two fields, five leaves.** A
# `&mut self` method writes every leaf of its receiver back to the
# caller, and the AOT backend has room for eight values on the way
# out; a reader holding `Vec` (4) + length + flag + cursor (2) came to
# exactly eight and `load` -- which also returns a `Result` -- pushed
# it over into "Too many return values to fit in registers". Length
# and truncation are therefore *derived* rather than stored, and the
# line cursor is a single `u64`.

import std.io

pub struct LogReader {
    buf: Vec<u8>,
    pos: u64,
}

impl LogReader {
    # `cap` is the largest file this reader will read whole.
    pub fn with_capacity(cap: u64) -> Self {
        val b: Vec<u8> = Vec::with_capacity(cap)
        LogReader { buf: b, pos: 0u64 }
    }

    # Read `path` into the buffer, replacing what was there.
    pub fn load(&mut self, path: str) -> Result<u64, IoError> {
        self.pos = 0u64
        self.buf.set_size(0u64)
        val room = self.buf.capacity_span()
        var n: u64 = 0u64
        match room {
            Option::Some(window) => { n = io::read_file_into(path, window)? }
            Option::None => { return Result::Err(IoError::ReadError) }
        }
        self.buf.set_size(n)
        Result::Ok(n)
    }

    pub fn size(&self) -> u64 { self.buf.size() }

    # `read_file_into` fills its window and stops, so a file that
    # exactly fills the buffer is indistinguishable from one that
    # overflowed it. Both are reported as truncated: over-reporting a
    # boundary case is cheaper than losing a tail silently.
    pub fn was_truncated(&self) -> bool {
        self.buf.size() >= self.buf.capacity()
    }

    pub fn byte(&self, i: u64) -> u8 { self.buf.get(i) }

    # A window over everything that was read. The archive path
    # compresses out of this rather than copying the file again.
    pub fn span(&self) -> Option<Span<u8>> { self.buf.as_span() }

    # Start the line cursor over again.
    pub fn rewind(&mut self) { self.pos = 0u64 }

    # The next line of the loaded file.
    #
    # The cursor lives here as one `u64` and the scanner is rebuilt
    # around it per call: `LineScan` is two fields, so making one is
    # free, and this keeps the line-splitting rules in a single place
    # (line.t) instead of copied into the reader.
    #
    # The buffer reaches the scanner as a `Span` bound to a local --
    # the AOT backend cannot pass a compound *field* as an argument,
    # and a window is the right thing to hand out anyway.
    pub fn next_line(&mut self) -> Option<Line> {
        val n = self.buf.size()
        if self.pos >= n { return Option::None }
        val w = self.buf.as_span()
        match w {
            Option::Some(span) => {
                var sc = LineScan { pos: self.pos, len: n }
                val got = sc.next(span)
                self.pos = sc.position()
                got
            }
            Option::None => { Option::None }
        }
    }

    # A copy of `len` bytes from `start`, as a `String`.
    #
    # **This allocates**, and on this runtime that memory never comes
    # back. It exists for reports and samples -- printing one line out
    # of a million -- and must not appear in a scanning loop.
    pub fn text(&self, start: u64, len: u64) -> String {
        var out = String::with_capacity(len)
        var i: u64 = 0u64
        while i < len {
            val b: u8 = self.buf.get(start + i)
            out.push(b)
            i = i + 1u64
        }
        out
    }

    # Whether the bytes at `at` are `text`, which must be ASCII.
    # The format probes call this once per candidate shape per line,
    # so it must not allocate: `Vec::from_str(text)` here would cost
    # an allocation per *probe*, and this runtime never gives one
    # back. Reading the literal's bytes directly is a raw read, which
    # is the whole reason this is an `unsafe fn` -- callers stay safe.
    pub unsafe fn matches(&self, at: u64, text: str) -> bool {
        val n = text.len()
        if at + n > self.buf.size() { return false }
        val p = __builtin_str_to_ptr(text)
        var i: u64 = 0u64
        while i < n {
            val a: u8 = self.buf.get(at + i)
            val b: u8 = __builtin_ptr_read::<u8>(p, i)
            if a != b { return false }
            i = i + 1u64
        }
        true
    }
}
