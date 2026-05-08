# `String` — heap-allocated growable byte buffer. Defined as a
# `type` alias for `Vec<u8>`, not a wrapper struct: every method
# users want lives directly on `Vec<T>` (the generic ones — `push`,
# `pop`, `get`, `set`, `size`, `capacity`, `is_empty`, `as_ptr`,
# `clear`) or on the byte-specific `impl Vec<u8>` (`from_str`,
# `push_str`, `push_char`, `eq`, `extend_bytes`).
#
# Sibling to the language's static `str` type:
#
#   - `str` — pointer + length to a `.rodata` UTF-8 byte sequence
#     (or, in the interpreter, an immutable run of typed-slot u8s).
#     Cheap to pass around; not mutable; lifetime is the program /
#     literal scope.
#   - `String` — a `Vec<u8>` on the heap. Owned, growable, freed
#     via the active allocator's normal path. Use this when a
#     `str` needs to be copied onto the heap (e.g. read input,
#     concatenate, etc.).
#
# Construction: `Vec::from_str("text")` with a `String` (= `Vec<u8>`)
# annotation provides T=u8 inference, e.g.
#
#     val s: String = Vec::from_str("hello")
#
# `str` superset extension (the `impl` blocks below) — `Vec<u8>`
# satisfies `core/std/str.t`'s `Length` / `AsPtr` traits and
# `core/std/str_ops.t`'s `Substring` / `Trim` / `CaseConvert` /
# `Concat` / `Contains` / `ToString` traits, so `.len()` /
# `.as_ptr()` / `.substring(start, end)` / `.trim()` /
# `.to_upper()` / `.to_lower()` / `.concat(other)` /
# `.contains(needle)` / `.to_string()` work on both `str` and
# `String` receivers with the same call shape. Internally each
# impl either delegates to the existing inherent `Vec<u8>` API
# (`.size()` / `self.data`) or walks the byte buffer with
# `__builtin_ptr_read`.
#
# Cross-module alias resolution (`frontend::resolve_type_aliases`)
# substitutes `String` with `Vec<u8>` everywhere in the integrated
# AST after parsing. User code can therefore write `val s: String`
# / `&String` / `-> String` and have it lower as `Vec<u8>` in
# every backend.
#
# Auto-loaded from `<core>/std/string.t -> ["std", "string"]`.
# Loads after `core/std/collections/vec.t` (segments-sort puts
# `["string"]` past `["collections", "vec"]`); the alias target's
# `Vec` reference is resolved at type-check time, so load order
# does not affect correctness.

type String = Vec<u8>

# `len()` — UTF-8 byte length. Identity wrapper around the
# existing `Vec<u8>::size()` so callers can use the same
# `.len()` name as `str`.
impl Length for Vec<u8> {
    fn len(self: Self) -> u64 {
        self.size()
    }
}

# `as_ptr()` — pointer to the first byte. The trait method on
# `str` returns a NUL-terminated pointer; `String` doesn't promise
# NUL termination (the buffer is sized exactly to `self.len`), so
# callers walking the bytes should pair `s.as_ptr()` with
# `s.len()` rather than scan for `'\0'`.
impl AsPtr for Vec<u8> {
    fn as_ptr(self: Self) -> ptr {
        self.data
    }
}

# `substring(start, end)` — half-open byte slice `[start, end)`.
# Both indices are byte offsets, not codepoint counts. Out-of-range
# / inverted ranges panic via `assert(...)`. The returned `Vec<u8>`
# is allocated through the active allocator and contains only the
# requested byte range.
impl Substring for Vec<u8> {
    fn substring(&self, start: u64, end: u64) -> Vec<u8> {
        assert(start <= end, "substring: start must be <= end")
        assert(end <= self.len, "substring: end out of range")
        var result: Vec<u8> = Vec::new()
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
# carriage return (0x0D). A buffer consisting entirely of
# whitespace returns an empty `Vec<u8>`. The trailing
# `val r = ...; r` bind (instead of `self.substring(...)` directly
# in tail position) sidesteps the AOT MVP limitation where
# compound-returning instance methods can't sit in expression
# position.
impl Trim for Vec<u8> {
    fn trim(&self) -> Vec<u8> {
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
        val r: Vec<u8> = self.substring(start, end)
        r
    }
}

# `to_upper()` / `to_lower()` — ASCII-only case folding. Bytes
# outside `b'a'..=b'z'` / `b'A'..=b'Z'` are copied unchanged so
# multi-byte UTF-8 sequences pass through as-is. Returns a fresh
# `Vec<u8>` (does not mutate `self`).
impl CaseConvert for Vec<u8> {
    fn to_upper(&self) -> Vec<u8> {
        var result: Vec<u8> = Vec::new()
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

    fn to_lower(&self) -> Vec<u8> {
        var result: Vec<u8> = Vec::new()
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
# The new vector is allocated through the active allocator
# (`Vec::new()` + per-byte `push`), so a `with allocator = ...`
# scope flows through naturally. Per-byte `push` instead of
# `extend_bytes(self.data, self.len)` because the latter mixes
# `&mut self` against a temporary `result` binding plus two
# `ptr` arguments derived from field accesses — a combination
# the AOT lower can't round-trip cleanly today.
impl Concat<Vec<u8>> for Vec<u8> {
    fn concat(&self, other: &Vec<u8>) -> Vec<u8> {
        var result: Vec<u8> = Vec::new()
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
impl Contains<Vec<u8>> for Vec<u8> {
    fn contains(&self, needle: &Vec<u8>) -> bool {
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

# `to_string()` — clone `self` into a fresh `Vec<u8>`. Idempotent
# on `String` (matches Rust's `String::to_string` behaviour).
impl ToString for Vec<u8> {
    fn to_string(self: Self) -> Vec<u8> {
        var result: Vec<u8> = Vec::new()
        var i: u64 = 0u64
        while i < self.len {
            val b: u8 = __builtin_ptr_read(self.data, i)
            result.push(b)
            i = i + 1u64
        }
        result
    }
}

# `split(sep)` — naive O(n * m) byte-loop split. Empty `sep`
# panics. Each part is a fresh `Vec<u8>` allocated through the
# active allocator; the outer `Vec<Vec<u8>>` holds them in
# encounter order (including a trailing empty slice if the input
# ends with `sep`, matching Rust's `str::split` shape). The
# AOT-COMPOUND-PTR-RW landing makes `Vec<T>` work for compound
# `T`, so the outer container fits the existing `Vec<u8>` impl
# unchanged.
impl Split<Vec<u8>, Vec<Vec<u8>>> for Vec<u8> {
    fn split(&self, sep: &Vec<u8>) -> Vec<Vec<u8>> {
        assert(sep.len > 0u64, "split: separator must be non-empty")
        var result: Vec<Vec<u8>> = Vec::new()
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
                val part: Vec<u8> = self.substring(start, i)
                result.push(part)
                start = i + m
                i = start
            } else {
                i = i + 1u64
            }
        }
        val tail: Vec<u8> = self.substring(start, n)
        result.push(tail)
        result
    }
}
