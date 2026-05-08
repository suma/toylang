# `String` — heap-allocated growable byte buffer. **Nominal**
# struct (no longer a `type` alias for `Vec<u8>`), so error
# messages and trait dispatch see `String` as its own type. The
# memory layout matches `Vec<u8>` exactly (data / len / cap /
# elem_size), which keeps every backend's existing
# generic-struct lowering paths working without per-type special-
# casing — the field-by-field `__builtin_heap_*` / `ptr_read` /
# `ptr_write` calls operate on raw bytes regardless of the
# nominal wrapper.
#
# Sibling to the language's static `str` type:
#
#   - `str` — pointer + length to a `.rodata` UTF-8 byte sequence
#     (or, in the interpreter, an immutable run of typed-slot u8s).
#     Cheap to pass around; not mutable; lifetime is the program /
#     literal scope.
#   - `String` — owned, growable, freed via the active allocator's
#     normal path. Use this when a `str` needs to be copied onto
#     the heap (e.g. read input, concatenate, etc.).
#
# Construction:
#
#     val s: String = String::from_str("hello")
#     var t: String = String::new()
#
# `Vec<u8>` (the generic byte-vector) remains available as a
# distinct type for byte-level work that doesn't need the String
# semantics — they no longer collapse to the same nominal type.
#
# `str` superset extension: `String` carries every read-only
# string operation (`len` / `as_ptr` / `substring` / `trim` /
# `to_upper` / `to_lower` / `concat` / `contains` / `to_string`)
# as inherent methods so the call shape matches `str`'s built-in
# methods exactly. The trait declarations in
# `core/std/str_ops.t` (`Substring` / `Trim` / `CaseConvert` /
# `Concat` / `Contains`) `impl` against `String` here too — they
# fit the per-receiver `Self`-returning shape cleanly.
# `ToString` is intentionally inherent-only on each type
# (str / String) instead of a trait — its non-`Self` return type
# (`String`) tripped the frontend's trait-conformance
# canonicalisation in mixed `Identifier(String)` /
# `Struct(String, [])` shapes; an inherent method sidesteps it
# without losing the user-facing call shape.
#
# Auto-loaded from `<core>/std/string.t -> ["std", "string"]`.

struct String {
    data: ptr,
    len: u64,
    cap: u64,
    elem_size: u64,
}

impl String {
    # Empty string. `elem_size = 1` because every byte is a u8.
    fn new() -> Self {
        String {
            data: __builtin_heap_alloc(0u64),
            len: 0u64,
            cap: 0u64,
            elem_size: 1u64,
        }
    }

    # Bulk-copy a `str`'s UTF-8 bytes onto a fresh String. The
    # trailing NUL terminator is intentionally NOT copied
    # (`size()` matches `s.len()` exactly).
    fn from_str(s: str) -> Self {
        val n: u64 = s.len()
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        __builtin_mem_copy(s.as_ptr(), data, n)
        String {
            data: data,
            len: n,
            cap: n,
            elem_size: 1u64,
        }
    }

    # Append. Geometric grow: 0 -> 4 -> 8 -> 16 -> ... amortised
    # O(1) per call.
    fn push(&mut self, b: u8) {
        if self.cap == 0u64 {
            self.cap = 4u64
            self.data = __builtin_heap_realloc(self.data, self.cap)
        } elif self.len >= self.cap {
            self.cap = self.cap * 2u64
            self.data = __builtin_heap_realloc(self.data, self.cap)
        }
        __builtin_ptr_write(self.data, self.len, b)
        self.len = self.len + 1u64
    }

    # Remove and return the last byte. Pre: `self.len > 0u64`
    # (caller's responsibility).
    fn pop(&mut self) -> u8 {
        self.len = self.len - 1u64
        val b: u8 = __builtin_ptr_read(self.data, self.len)
        b
    }

    # Random read (no bounds check).
    fn get(&self, i: u64) -> u8 {
        val b: u8 = __builtin_ptr_read(self.data, i)
        b
    }

    # Random write (no bounds check).
    fn set(&mut self, i: u64, b: u8) {
        __builtin_ptr_write(self.data, i, b)
    }

    # Current byte count.
    fn size(&self) -> u64 {
        self.len
    }

    # Inherent `len()` — same value as `size()`. Mirrors the
    # `Length for str` trait method name so `s.len()` works
    # uniformly on `str` / `String` receivers without forcing
    # users through `.size()`.
    fn len(&self) -> u64 {
        self.len
    }

    # Allocated byte capacity.
    fn capacity(&self) -> u64 {
        self.cap
    }

    fn is_empty(&self) -> bool {
        self.len == 0u64
    }

    # Reset the byte count to 0 without releasing the buffer.
    # Subsequent `push` calls reuse the existing capacity.
    fn clear(&mut self) {
        self.len = 0u64
    }

    # Inherent pointer accessor. Mirrors `AsPtr for str`'s
    # `as_ptr()` so the call shape works on both receivers.
    # `String` doesn't promise NUL termination (the buffer is
    # sized exactly to `self.len`); pair `s.as_ptr()` with
    # `s.len()` rather than scan for `'\0'`.
    fn as_ptr(&self) -> ptr {
        self.data
    }

    # Append `count` bytes from `src` to the end of the buffer.
    # Per-byte `push` so geometric grow kicks in without needing
    # pointer-arithmetic builtins.
    fn extend_bytes(&mut self, src: ptr, count: u64) {
        var i: u64 = 0u64
        while i < count {
            val b: u8 = __builtin_ptr_read(src, i)
            self.push(b)
            i = i + 1u64
        }
    }

    # Append the bytes of another String. Auto-borrow at the call
    # site lets `s.push_str(t)` work with `t: String`.
    fn push_str(&mut self, other: &String) {
        self.extend_bytes(other.data, other.len)
    }

    # UTF-8 encode a Unicode codepoint and append the resulting
    # 1-4 bytes (RFC 3629). Surrogate codepoints (U+D800..U+DFFF)
    # and codepoints >= U+110000 are not valid Unicode scalars
    # and panic.
    fn push_char(&mut self, c: char) {
        assert(c < 0x110000u32, "push_char: codepoint out of range")
        assert(!(c >= 0xD800u32 && c <= 0xDFFFu32),
               "push_char: surrogate codepoint not allowed")
        val cp: u64 = c as u64
        if cp < 0x80u64 {
            self.push(cp as u8)
        } elif cp < 0x800u64 {
            self.push(((0xC0u64 | (cp >> 6u64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | (cp & 0x3Fu64)) & 0xFFu64) as u8)
        } elif cp < 0x10000u64 {
            self.push(((0xE0u64 | (cp >> 12u64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | ((cp >> 6u64) & 0x3Fu64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | (cp & 0x3Fu64)) & 0xFFu64) as u8)
        } else {
            self.push(((0xF0u64 | (cp >> 18u64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | ((cp >> 12u64) & 0x3Fu64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | ((cp >> 6u64) & 0x3Fu64)) & 0xFFu64) as u8)
            self.push(((0x80u64 | (cp & 0x3Fu64)) & 0xFFu64) as u8)
        }
    }

    # Byte-wise equality. Two strings are equal iff they have the
    # same length and every byte matches. Length check first so
    # different-sized strings short-circuit without walking the
    # buffer. Operator overload (`==` / `!=`) routes here via the
    # `eq` method dispatch (frontend's struct_eq_compatible
    # check).
    fn eq(&self, other: &String) -> bool {
        val n: u64 = self.len
        if n != other.len {
            return false
        }
        var i: u64 = 0u64
        while i < n {
            val a: u8 = __builtin_ptr_read(self.data, i)
            val b: u8 = __builtin_ptr_read(other.data, i)
            if a != b {
                return false
            }
            i = i + 1u64
        }
        true
    }

    # Inherent `to_string()` — clone `self` into a fresh String.
    # Idempotent (matches Rust's `String::to_string` behaviour).
    # Inherent rather than via a `ToString` trait because the
    # trait's non-`Self` return type tripped the frontend's
    # trait-conformance canonicalisation in mixed
    # `Identifier(String)` / `Struct(String, [])` shapes; the
    # inherent form is functionally equivalent at the call site.
    fn to_string(&self) -> String {
        var result: String = String::new()
        var i: u64 = 0u64
        while i < self.len {
            val b: u8 = __builtin_ptr_read(self.data, i)
            result.push(b)
            i = i + 1u64
        }
        result
    }
}

# `substring(start, end)` — half-open byte slice `[start, end)`.
# Both indices are byte offsets, not codepoint counts. Out-of-range
# / inverted ranges panic via `assert(...)`.
impl Substring for String {
    fn substring(&self, start: u64, end: u64) -> String {
        assert(start <= end, "substring: start must be <= end")
        assert(end <= self.len, "substring: end out of range")
        var result: String = String::new()
        var i: u64 = start
        while i < end {
            val b: u8 = __builtin_ptr_read(self.data, i)
            result.push(b)
            i = i + 1u64
        }
        result
    }
}

# `trim()` — strip ASCII whitespace from both ends. Recognises
# space (0x20), horizontal tab (0x09), newline (0x0A), and
# carriage return (0x0D). The trailing `val r = ...; r` bind
# (instead of `self.substring(...)` directly in tail position)
# sidesteps the AOT MVP limitation where compound-returning
# instance methods can't sit in expression position.
impl Trim for String {
    fn trim(&self) -> String {
        val n: u64 = self.len
        var start: u64 = 0u64
        while start < n {
            val b: u8 = __builtin_ptr_read(self.data, start)
            if b == 0x20u8 || b == 0x09u8 || b == 0x0Au8 || b == 0x0Du8 {
                start = start + 1u64
            } else {
                break
            }
        }
        var end: u64 = n
        while end > start {
            val b: u8 = __builtin_ptr_read(self.data, end - 1u64)
            if b == 0x20u8 || b == 0x09u8 || b == 0x0Au8 || b == 0x0Du8 {
                end = end - 1u64
            } else {
                break
            }
        }
        val r: String = self.substring(start, end)
        r
    }
}

# `to_upper()` / `to_lower()` — ASCII-only case folding. Bytes
# outside `b'a'..=b'z'` / `b'A'..=b'Z'` are copied unchanged so
# multi-byte UTF-8 sequences pass through as-is.
impl CaseConvert for String {
    fn to_upper(&self) -> String {
        var result: String = String::new()
        var i: u64 = 0u64
        while i < self.len {
            val b: u8 = __builtin_ptr_read(self.data, i)
            if b >= 0x61u8 && b <= 0x7Au8 {
                result.push(b - 0x20u8)
            } else {
                result.push(b)
            }
            i = i + 1u64
        }
        result
    }

    fn to_lower(&self) -> String {
        var result: String = String::new()
        var i: u64 = 0u64
        while i < self.len {
            val b: u8 = __builtin_ptr_read(self.data, i)
            if b >= 0x41u8 && b <= 0x5Au8 {
                result.push(b + 0x20u8)
            } else {
                result.push(b)
            }
            i = i + 1u64
        }
        result
    }
}

# `concat(other)` — append `other`'s bytes after `self`'s.
# Per-byte `push` instead of `extend_bytes(self.data, self.len)`
# because the latter mixes `&mut self` against a temporary
# `result` binding plus two `ptr` arguments derived from field
# accesses — a combination the AOT lower can't round-trip
# cleanly today.
impl Concat<String> for String {
    fn concat(&self, other: &String) -> String {
        var result: String = String::new()
        var i: u64 = 0u64
        while i < self.len {
            val a: u8 = __builtin_ptr_read(self.data, i)
            result.push(a)
            i = i + 1u64
        }
        var j: u64 = 0u64
        while j < other.len {
            val b: u8 = __builtin_ptr_read(other.data, j)
            result.push(b)
            j = j + 1u64
        }
        result
    }
}

# `contains(needle)` — naive O(n * m) byte loop. Empty `needle`
# matches at position 0 (Rust / libc convention). Sufficient for
# short needles, the typical user-code case.
impl Contains<String> for String {
    fn contains(&self, needle: &String) -> bool {
        val n: u64 = self.len
        val m: u64 = needle.len
        if m == 0u64 {
            return true
        }
        if m > n {
            return false
        }
        var i: u64 = 0u64
        while i + m <= n {
            var matched: bool = true
            var j: u64 = 0u64
            while j < m {
                val a: u8 = __builtin_ptr_read(self.data, i + j)
                val b: u8 = __builtin_ptr_read(needle.data, j)
                if a != b {
                    matched = false
                    break
                }
                j = j + 1u64
            }
            if matched {
                return true
            }
            i = i + 1u64
        }
        false
    }
}

# `split(sep)` — naive O(n * m) byte-loop split. Empty `sep`
# panics. Each part is a fresh `String` allocated through the
# active allocator; the outer `Vec<String>` holds them in
# encounter order (including a trailing empty slice if the input
# ends with `sep`, matching Rust's `str::split` shape).
impl Split<String, Vec<String>> for String {
    fn split(&self, sep: &String) -> Vec<String> {
        assert(sep.len > 0u64, "split: separator must be non-empty")
        var result: Vec<String> = Vec::new()
        val n: u64 = self.len
        val m: u64 = sep.len
        var start: u64 = 0u64
        var i: u64 = 0u64
        while i + m <= n {
            var matched: bool = true
            var j: u64 = 0u64
            while j < m {
                val a: u8 = __builtin_ptr_read(self.data, i + j)
                val b: u8 = __builtin_ptr_read(sep.data, j)
                if a != b {
                    matched = false
                    break
                }
                j = j + 1u64
            }
            if matched {
                val part: String = self.substring(start, i)
                result.push(part)
                start = i + m
                i = start
            } else {
                i = i + 1u64
            }
        }
        val tail: String = self.substring(start, n)
        result.push(tail)
        result
    }
}
