# Writing and reading the little-endian byte soup that the on-disk
# formats are made of (STORAGE_FORMAT.md).
#
# `ByteWriter` owns one growable buffer; a segment build fills it,
# hands it to `io::write_file_bytes`, and then `clear`s it for the
# next segment. It is the only place in the write path that grows,
# and it grows to a high-water mark and stays there -- which is the
# behaviour this runtime needs, since a freed byte never comes back
# (MEMORY.md).
#
# `ByteReader` is a cursor over a `Span<u8>`: a window, never an
# owner. Reads are bounds-checked by `Span` itself.

pub struct ByteWriter {
    buf: Vec<u8>,
}

impl ByteWriter {
    pub fn with_capacity(n: u64) -> Self {
        val b: Vec<u8> = Vec::with_capacity(n)
        ByteWriter { buf: b }
    }

    pub fn len(&self) -> u64 { self.buf.size() }
    pub fn is_empty(&self) -> bool { self.buf.size() == 0u64 }
    pub fn clear(&mut self) { self.buf.clear() }

    # Drop everything after `n` bytes. The frame writer uses this to
    # take back a compression attempt that did not pay for itself.
    pub fn truncate(&mut self, n: u64)
        requires n <= self.len()
    {
        if n < self.buf.size() { self.buf.set_size(n) }
    }
    pub fn span(&self) -> Option<Span<u8>> { self.buf.as_span() }
    pub fn byte_at(&self, i: u64) -> u8 { self.buf.get(i) }

    pub fn put_u8(&mut self, v: u8) { self.buf.push(v) }

    pub fn put_u16(&mut self, v: u64)
        requires v <= 0xFFFFu64
    {
        self.buf.push((v & 0xFFu64) as u8)
        self.buf.push(((v >> 8u64) & 0xFFu64) as u8)
    }

    pub fn put_u32(&mut self, v: u64)
        requires v <= 0xFFFFFFFFu64
    {
        var i: u64 = 0u64
        while i < 4u64 {
            self.buf.push(((v >> (i * 8u64)) & 0xFFu64) as u8)
            i = i + 1u64
        }
    }

    pub fn put_u64(&mut self, v: u64) {
        var i: u64 = 0u64
        while i < 8u64 {
            self.buf.push(((v >> (i * 8u64)) & 0xFFu64) as u8)
            i = i + 1u64
        }
    }

    pub fn put_i64(&mut self, v: i64) { self.put_u64(v as u64) }

    # LEB128: seven bits at a time, high bit set while more follow.
    pub fn put_varint(&mut self, v: u64) {
        var x = v
        var more = true
        while more {
            val low = (x & 0x7Fu64) as u8
            x = x >> 7u64
            if x == 0u64 {
                self.buf.push(low)
                more = false
            } else {
                self.buf.push(low | 0x80u8)
            }
        }
    }

    # Four ASCII bytes, written in the order they are spelled.
    pub unsafe fn put_magic(&mut self, tag: str) {
        val p = __builtin_str_to_ptr(tag)
        var i: u64 = 0u64
        while i < tag.len() {
            val b: u8 = __builtin_ptr_read::<u8>(p, i)
            self.buf.push(b)
            i = i + 1u64
        }
    }

    # `len` bytes of `src` starting at `from`.
    pub fn put_span(&mut self, src: Span<u8>, from: u64, len: u64)
        requires from + len <= src.len()
    {
        var i: u64 = 0u64
        while i < len {
            val b: u8 = src.get(from + i)
            self.buf.push(b)
            i = i + 1u64
        }
    }

    pub fn capacity(&self) -> u64 { self.buf.capacity() }

    # A window over the whole allocation, not just the bytes written
    # so far: where a file read lands. `set_len` then says how much of
    # it is real. The pair exists because `read_at` fills a window it
    # is given, and a window over `span()` would be zero bytes long
    # after `clear`.
    pub fn room(&self) -> Option<Span<u8>> { self.buf.capacity_span() }

    pub fn set_len(&mut self, n: u64)
        requires n <= self.capacity()
    {
        self.buf.set_size(n)
    }

    # Make sure `n` more bytes will fit without the buffer moving.
    # The block writers below store through the raw pointer, so the
    # room has to exist before they start.
    pub fn reserve(&mut self, n: u64) {
        val need = self.buf.size() + n
        if need > self.buf.capacity() {
            var cap = self.buf.capacity()
            if cap == 0u64 { cap = 64u64 }
            while cap < need { cap = cap * 2u64 }
            self.buf.grow_to(cap)
        }
    }

    # `put_span`, sixteen bytes at a time.
    #
    # The copy goes through `__simd_load` / `__simd_store` on the raw
    # pointers rather than `push`, which is what makes it worth
    # writing: `push` is a call and a bounds test per byte. The room
    # must already be reserved -- stated as a `requires` rather than
    # checked here, so the caller cannot forget quietly.
    pub unsafe fn put_span_fast(&mut self, src: Span<u8>, from: u64, len: u64)
        requires from + len <= src.len()
        requires self.len() + len <= self.capacity()
    {
        val dst = self.buf.as_ptr()
        val sp = src.as_raw()
        val base = self.buf.size()
        var i: u64 = 0u64
        while i + 16u64 <= len {
            val v: u8x16 = __simd_load(sp, from + i)
            __simd_store(dst, base + i, v)
            i = i + 16u64
        }
        while i < len {
            val b: u8 = src.get(from + i)
            __builtin_ptr_write(dst, base + i, b)
            i = i + 1u64
        }
        self.buf.set_size(base + len)
    }

    # Append `len` bytes that are already in this buffer, starting at
    # `start`. This is an LZ77 match copy.
    #
    # **Only correct when the distance is at least 16.** A block copy
    # reads sixteen bytes that the same call may still be writing; at
    # a distance of sixteen or more every byte a block reads was
    # written at least one block ago, which is exactly the condition
    # the caller states in its `requires`.
    pub unsafe fn append_from_self(&mut self, start: u64, len: u64)
        requires start + 16u64 <= self.len()
    {
        val p = self.buf.as_ptr()
        val base = self.buf.size()
        var i: u64 = 0u64
        while i + 16u64 <= len {
            val v: u8x16 = __simd_load(p, start + i)
            __simd_store(p, base + i, v)
            i = i + 16u64
        }
        while i < len {
            val b: u8 = __builtin_ptr_read::<u8>(p, start + i)
            __builtin_ptr_write(p, base + i, b)
            i = i + 1u64
        }
        self.buf.set_size(base + len)
    }

    # Overwrite four bytes that were written earlier -- headers carry
    # lengths that are only known once the body is out.
    pub fn patch_u32(&mut self, at: u64, v: u64)
        requires at + 4u64 <= self.len()
        requires v <= 0xFFFFFFFFu64
    {
        var i: u64 = 0u64
        while i < 4u64 {
            self.buf.set(at + i, ((v >> (i * 8u64)) & 0xFFu64) as u8)
            i = i + 1u64
        }
    }

    pub fn patch_u64(&mut self, at: u64, v: u64)
        requires at + 8u64 <= self.len()
    {
        var i: u64 = 0u64
        while i < 8u64 {
            self.buf.set(at + i, ((v >> (i * 8u64)) & 0xFFu64) as u8)
            i = i + 1u64
        }
    }
}

# A cursor over bytes somebody else owns.
#
# The readers are `take_*` rather than `u32` / `u64`: a primitive
# type name cannot be a method name in this language, and the parser
# says so in a way that takes a moment to place ("expected method
# name after fn").
pub struct ByteReader {
    pos: u64,
    end: u64,
}

impl ByteReader {
    pub fn new(end: u64) -> Self { ByteReader { pos: 0u64, end: end } }
    pub fn seek(&mut self, at: u64) { self.pos = at }
    pub fn position(&self) -> u64 { self.pos }
    pub fn remaining(&self) -> u64 { if self.pos >= self.end { 0u64 } else { self.end - self.pos } }

    pub fn take_u8(&mut self, src: Span<u8>) -> u8
        requires self.pos + 1u64 <= self.end
    {
        val b: u8 = src.get(self.pos)
        self.pos = self.pos + 1u64
        b
    }

    pub fn take_u16(&mut self, src: Span<u8>) -> u64
        requires self.pos + 2u64 <= self.end
    {
        val lo: u8 = src.get(self.pos)
        val hi: u8 = src.get(self.pos + 1u64)
        self.pos = self.pos + 2u64
        (lo as u64) | ((hi as u64) << 8u64)
    }

    pub fn take_u32(&mut self, src: Span<u8>) -> u64
        requires self.pos + 4u64 <= self.end
    {
        var v: u64 = 0u64
        var i: u64 = 0u64
        while i < 4u64 {
            val b: u8 = src.get(self.pos + i)
            v = v | ((b as u64) << (i * 8u64))
            i = i + 1u64
        }
        self.pos = self.pos + 4u64
        v
    }

    pub fn take_u64(&mut self, src: Span<u8>) -> u64
        requires self.pos + 8u64 <= self.end
    {
        var v: u64 = 0u64
        var i: u64 = 0u64
        while i < 8u64 {
            val b: u8 = src.get(self.pos + i)
            v = v | ((b as u64) << (i * 8u64))
            i = i + 1u64
        }
        self.pos = self.pos + 8u64
        v
    }

    pub fn take_i64(&mut self, src: Span<u8>) -> i64 {
        val v = self.take_u64(src)
        v as i64
    }

    pub fn take_varint(&mut self, src: Span<u8>) -> u64
        requires self.pos < self.end
    {
        var v: u64 = 0u64
        var shift: u64 = 0u64
        var more = true
        while more {
            val b: u8 = src.get(self.pos)
            self.pos = self.pos + 1u64
            v = v | (((b & 0x7Fu8) as u64) << shift)
            if (b & 0x80u8) == 0u8 { more = false }
            shift = shift + 7u64
        }
        v
    }

    # Whether the next four bytes spell `tag`, consuming them if so.
    pub unsafe fn take_magic(&mut self, src: Span<u8>, tag: str) -> bool {
        val p = __builtin_str_to_ptr(tag)
        val n = tag.len()
        var i: u64 = 0u64
        var ok = true
        while i < n {
            val a: u8 = src.get(self.pos + i)
            val b: u8 = __builtin_ptr_read::<u8>(p, i)
            if a != b { ok = false }
            i = i + 1u64
        }
        self.pos = self.pos + n
        ok
    }
}
