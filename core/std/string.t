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
# `to_ascii_upper` / `to_ascii_lower` / `concat` / `contains` / `to_string`)
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
    # Move the buffer to `new_cap` bytes, or stop the program
    # (ERROR_MODEL D5). As `Vec::grow_to`: the check is before
    # `self.data` is assigned, because `realloc` leaves the original
    # block alone when it fails and writing the null in first would
    # lose it. A zero-byte request legitimately answers null
    # (`core/std/ptr.t`), so only a non-zero one can have failed.
    unsafe fn grow_to(&mut self, new_cap: u64) {
        val grown: ptr = __builtin_heap_realloc(self.data, new_cap)
        if new_cap > 0u64 && __builtin_ptr_is_null(grown) {
            panic("String::grow: allocation failed ({new_cap} bytes)")
        }
        self.data = grown
        self.cap = new_cap
    }

    # Make room for `n` more bytes than `size()`, or say why not.
    # The point is to fail once, before a loop of `push`es that then
    # stay within capacity -- see `core/std/alloc.t`.
    unsafe fn try_reserve(&mut self, n: u64) -> Result<(), AllocError> {
        val used: u64 = self.len
        val want: Option<u64> = used.checked_add(n)
        val need: u64 = match want {
            Option::Some(c) => c,
            Option::None => { return Result::Err(AllocError::SizeOverflow) }
        }
        if need <= self.cap { return Result::Ok(()) }
        val grown: ptr = __builtin_heap_realloc(self.data, need)
        if need > 0u64 && __builtin_ptr_is_null(grown) {
            return Result::Err(AllocError::OutOfMemory)
        }
        self.data = grown
        self.cap = need
        Result::Ok(())
    }

    unsafe fn from_str(s: str) -> Self {
        val n: u64 = s.len()
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        if n > 0u64 && __builtin_ptr_is_null(data) {
            panic("String::from_str: allocation failed ({n} bytes)")
        }
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
    unsafe fn push(&mut self, b: u8) {
        if self.cap == 0u64 {
            self.grow_to(4u64)
        } elif self.len >= self.cap {
            self.grow_to(self.cap * 2u64)
        }
        __builtin_ptr_write(self.data, self.len, b)
        self.len = self.len + 1u64
    }

    # Remove and return the last byte. Pre: `self.len > 0u64`
    # (caller's responsibility).
    unsafe fn pop(&mut self) -> u8 {
        if self.len == 0u64 { panic("String::pop on an empty String") }
        self.len = self.len - 1u64
        val b: u8 = __builtin_ptr_read(self.data, self.len)
        b
    }

    # Random read, bounds-checked (DEBUG-OBS D6). Reading past the end
    # used to reach the host rather than fail as a toylang program.
    unsafe fn get(&self, i: u64) -> u8 {
        if i >= self.len { panic("String::get index out of bounds") }
        val b: u8 = __builtin_ptr_read(self.data, i)
        b
    }

    # Random write, bounds-checked. `push` writes through the raw
    # pointer, so appending is not affected by this.
    unsafe fn set(&mut self, i: u64, b: u8) {
        if i >= self.len { panic("String::set index out of bounds") }
        __builtin_ptr_write(self.data, i, b)
    }

    # A window over the bytes (CONV-SPAN), for code that takes a
    # `Span<u8>` — no copy, so a write through it lands in this
    # String's own buffer.
    #
    # `None` while the String has never allocated (`String::new()`
    # with nothing pushed): there is no address to view.
    fn as_span(&self) -> Option<Span<u8>> {
        Span::try_from_raw_parts(self.data, self.len)
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
    unsafe fn extend_bytes(&mut self, src: ptr, count: u64) {
        var i: u64 = 0u64
        while i < count {
            val b: u8 = __builtin_ptr_read(src, i)
            self.push(b)
            i = i + 1u64
        }
    }

    # Append the bytes of a `str` -- a literal, or any borrowed
    # string.
    #
    #     s.push_str("hello")
    #
    # This name used to mean "append a `String`", which meant
    # `s.push_str("literal")` -- the thing everyone writes first --
    # type-checked and then died at run time with `Cannot access
    # field on non-struct object: ConstString`. The names now say
    # which type they take (STDLIB-TEXT §8).
    unsafe fn push_str(&mut self, other: str) {
        self.extend_bytes(other.as_ptr(), other.len())
    }

    # Append the bytes of another String. Auto-borrow at the call
    # site lets `s.push_string(t)` work with `t: String`.
    fn push_string(&mut self, other: &String) {
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
    unsafe fn eq(&self, other: &String) -> bool {
        val n: u64 = self.len
        if n != other.len {
            return false
        }
        # SIMD: 16 bytes per comparison while a whole chunk fits.
        # The bound is `i + 16 <= n`, never `i < n`, because a
        # vector load reads all 16 bytes -- a chunk that straddles
        # the end of the buffer would read past the allocation.
        var i: u64 = 0u64
        while i + 16u64 <= n {
            val a: u8x16 = __simd_load(self.data, i)
            val b: u8x16 = __simd_load(other.data, i)
            if !__simd_all(a == b) {
                return false
            }
            i = i + 16u64
        }
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

    # Shared body of `to_ascii_upper` / `to_ascii_lower` (CaseConvert). Copies
    # the bytes, then adds or subtracts 0x20 on every byte inside
    # `[lo, hi]`, leaving the rest untouched -- so bytes outside
    # `a-z` / `A-Z`, including every continuation byte of a
    # multi-byte UTF-8 sequence, pass through unchanged.
    #
    # `up` picks the direction: subtract to reach uppercase, add to
    # reach lowercase.
    unsafe fn fold_ascii_case(&self, lo: u8, hi: u8, up: bool) -> String {
        val n: u64 = self.len
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        if n > 0u64 && __builtin_ptr_is_null(data) {
            panic("String::fold_ascii_case: allocation failed ({n} bytes)")
        }
        __builtin_mem_copy(self.data, data, n)
        # SIMD: 16 bytes per pass while a whole chunk fits. The bound
        # is `i + 16 <= n` -- a vector load reads all 16 bytes, so a
        # chunk straddling the end would read past the allocation.
        val lo_v: u8x16 = __simd_splat(lo)
        val hi_v: u8x16 = __simd_splat(hi)
        val delta: u8x16 = __simd_splat(0x20u8)
        var i: u64 = 0u64
        while i + 16u64 <= n {
            val v: u8x16 = __simd_load(data, i)
            # `>=` and `<=` each answer per lane, so the two masks
            # combine with a bitwise `&` rather than `&&`.
            val in_range = (v >= lo_v) & (v <= hi_v)
            var folded: u8x16 = v + delta
            if up {
                folded = v - delta
            }
            __simd_store(data, i, __simd_select(in_range, folded, v))
            i = i + 16u64
        }
        while i < n {
            val b: u8 = __builtin_ptr_read(data, i)
            if b >= lo && b <= hi {
                if up {
                    __builtin_ptr_write(data, i, b - 0x20u8)
                } else {
                    __builtin_ptr_write(data, i, b + 0x20u8)
                }
            }
            i = i + 1u64
        }
        String {
            data: data,
            len: n,
            cap: n,
            elem_size: 1u64,
        }
    }

    # Inherent `to_string()` — clone `self` into a fresh String.
    # Idempotent (matches Rust's `String::to_string` behaviour).
    # Inherent rather than via a `ToString` trait because the
    # trait's non-`Self` return type tripped the frontend's
    # trait-conformance canonicalisation in mixed
    # `Identifier(String)` / `Struct(String, [])` shapes; the
    # inherent form is functionally equivalent at the call site.
    # Whether these bytes are valid UTF-8, and so whether `to_str()`
    # will accept them (STDLIB-TEXT §2).
    #
    # `String` holds arbitrary bytes; `str` holds text. Crossing from
    # one to the other is checked, and a `String` that came from the
    # network or a file may not have text in it. This is the way to
    # **ask before** crossing -- the same discipline `try_reserve` uses
    # for allocation, and for the same reason: a per-byte `Result` from
    # `to_str` would put a branch in every string that was always fine.
    #
    # RFC 3629: no over-long encodings, no surrogates (U+D800..U+DFFF),
    # nothing above U+10FFFF.
    unsafe fn is_utf8(&self) -> bool {
        var i: u64 = 0u64
        while i < self.len {
            val b: u8 = __builtin_ptr_read(self.data, i)
            var need: u64 = 0u64
            var lo: u32 = 0u32
            var hi: u32 = 0u32
            var cp: u32 = 0u32
            if b < 128u8 {
                i = i + 1u64
                continue
            } elif b >= 194u8 && b <= 223u8 {
                need = 1u64
                cp = (b as u32) - 192u32
                lo = 128u32
                hi = 2047u32
            } elif b >= 224u8 && b <= 239u8 {
                need = 2u64
                cp = (b as u32) - 224u32
                lo = 2048u32
                hi = 65535u32
            } elif b >= 240u8 && b <= 244u8 {
                need = 3u64
                cp = (b as u32) - 240u32
                lo = 65536u32
                hi = 1114111u32
            } else {
                # 0x80..0xC1 is a stray continuation or an over-long
                # two-byte lead; 0xF5..0xFF is past U+10FFFF.
                return false
            }
            if i + need >= self.len + 1u64 { return false }
            var k: u64 = 1u64
            while k <= need {
                val c: u8 = __builtin_ptr_read(self.data, i + k)
                if c < 128u8 || c > 191u8 { return false }
                cp = cp * 64u32 + ((c as u32) - 128u32)
                k = k + 1u64
            }
            if cp < lo || cp > hi { return false }
            # Surrogates are not scalar values, so they are not text.
            if cp >= 55296u32 && cp <= 57343u32 { return false }
            i = i + need + 1u64
        }
        true
    }

    unsafe fn to_string(&self) -> String {
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

# `Display` — render the buffer's bytes as a `str`, so `println(s)`
# and `"{s}"` show the text rather than the struct's fields. Without
# this, the stdlib's own string type printed as
# `String { cap: 2, data: 12, elem_size: 1, len: 2 }`.
#
# `__builtin_str_from_bytes` copies, so the result does not alias the
# buffer and is unaffected by a later `push`.
# `Clone` for a String: `to_string` already builds an independent
# buffer, so this is that under the name a `<T: Clone>` bound asks
# for.
impl Clone for String {
    unsafe fn clone(&self) -> Self {
        # Bound rather than returned directly: the compiled lanes
        # refuse a compound-returning method in expression position.
        val copy: String = self.to_string()
        copy
    }
}

impl Display for String {
    unsafe fn to_str(&self) -> str {
        __builtin_str_from_bytes(self.data, self.len)
    }
}

# `substring(start, end)` — half-open byte slice `[start, end)`.
# Both indices are byte offsets, not codepoint counts. Out-of-range
# / inverted ranges panic via `assert(...)`.
impl Substring for String {
    unsafe fn substring(&self, start: u64, end: u64) -> String {
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
    unsafe fn trim(&self) -> String {
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

# `to_ascii_upper()` / `to_ascii_lower()` — ASCII-only case folding. Bytes
# outside `b'a'..=b'z'` / `b'A'..=b'Z'` are copied unchanged so
# multi-byte UTF-8 sequences pass through as-is.
# ASCII case folding is entirely lane-wise -- add or subtract 0x20
# where the byte falls in the letter range, leave it alone otherwise
# -- so both directions copy the bytes once and then fold them 16 at
# a time. Building the buffer up front (rather than `push`-ing byte
# by byte) is what makes the vector path reachable: `push` would
# force a per-byte round trip through the geometric grow.
impl CaseConvert for String {
    # The `val` binding is not decoration: the compiled lanes reject
    # a compound-returning method in expression position, so the
    # result has to be named before it is returned.
    fn to_ascii_upper(&self) -> String {
        val r: String = self.fold_ascii_case('a', 'z', true)
        r
    }

    fn to_ascii_lower(&self) -> String {
        val r: String = self.fold_ascii_case('A', 'Z', false)
        r
    }
}

# `concat(other)` — append `other`'s bytes after `self`'s.
# Per-byte `push` instead of `extend_bytes(self.data, self.len)`
# because the latter mixes `&mut self` against a temporary
# `result` binding plus two `ptr` arguments derived from field
# accesses — a combination the AOT lower can't round-trip
# cleanly today.
impl Concat<String> for String {
    unsafe fn concat(&self, other: &String) -> String {
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

# `contains(needle)` — O(n * m) worst case, but the scan for a
# candidate start is done 16 bytes at a time (memchr's trick).
# Empty `needle` matches at position 0 (Rust / libc convention).
#
# The skip is sound because a match starting anywhere in
# `[i, i+16)` would have to begin with `needle`'s first byte: if no
# lane in that window equals it, all sixteen positions can be
# discarded at once.
impl Contains<String> for String {
    unsafe fn contains(&self, needle: &String) -> bool {
        val n: u64 = self.len
        val m: u64 = needle.len
        if m == 0u64 {
            return true
        }
        if m > n {
            return false
        }
        val first: u8 = __builtin_ptr_read(needle.data, 0u64)
        val first_v: u8x16 = __simd_splat(first)
        var i: u64 = 0u64
        while i + m <= n {
            # Only when a whole chunk fits: a vector load reads all
            # 16 bytes, so a chunk straddling the end of the buffer
            # would read past the allocation.
            if i + 16u64 <= n {
                val chunk: u8x16 = __simd_load(self.data, i)
                if !__simd_any(chunk == first_v) {
                    i = i + 16u64
                    continue
                }
            }
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

# `split(sep)` — O(n * m) worst case, with the scan for a candidate
# separator done 16 bytes at a time (the same memchr trick
# `contains` uses). Empty `sep` panics. Each part is a fresh `String` allocated through the
# active allocator; the outer `Vec<String>` holds them in
# encounter order (including a trailing empty slice if the input
# ends with `sep`, matching Rust's `str::split` shape).
impl Split<String, Vec<String>> for String {
    unsafe fn split(&self, sep: &String) -> Vec<String> {
        assert(sep.len > 0u64, "split: separator must be non-empty")
        var result: Vec<String> = Vec::new()
        val n: u64 = self.len
        val m: u64 = sep.len
        val first: u8 = __builtin_ptr_read(sep.data, 0u64)
        val first_v: u8x16 = __simd_splat(first)
        var start: u64 = 0u64
        var i: u64 = 0u64
        while i + m <= n {
            # Same memchr-style skip `contains` uses: a match must
            # begin with the separator's first byte, so a 16-byte
            # window containing none of it can be discarded whole.
            # `start` is untouched -- it only moves on a match -- so
            # skipping does not disturb the part boundaries.
            if i + 16u64 <= n {
                val chunk: u8x16 = __simd_load(self.data, i)
                if !__simd_any(chunk == first_v) {
                    i = i + 16u64
                    continue
                }
            }
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

# Iterator-protocol support (STDLIB-ITER): `for b in s.iter() { ... }`
# yields one `u8` per byte of the buffer, in order. Same structural
# protocol as `Vec::iter`. (Byte iteration, not codepoint iteration —
# a UTF-8-aware `chars()` iterator is a future stdlib addition.)
struct StringIter {
    data: ptr,
    len: u64,
    index: u64,
}

impl String {
    # Borrow the string into an iterator. `&self` keeps the caller's
    # binding alive; the returned iterator shares the buffer.
    fn iter(&self) -> StringIter {
        StringIter { data: self.data, len: self.len, index: 0u64 }
    }
}

impl Iterator<u8> for StringIter {
    # Advance by one byte. Returns `None` once `index` has walked
    # past `len`.
    unsafe fn next(&mut self) -> Option<u8> {
        if self.index >= self.len {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val b: u8 = __builtin_ptr_read(self.data, i)
            Option::Some(b)
        }
    }
}


# ---------------------------------------------------------------------
# Searching and building (STDLIB-TEXT §8).
#
# The `str` side of these lives in `core/std/str.t` and answers about
# bytes it borrows. These are the versions for a buffer you own, plus
# the ones that can only exist here because their answer is a new
# buffer.

impl String {
    # Byte offset of the first occurrence of `needle` at or after
    # `start`, or `None`. An empty needle is found at `start`.
    # (`from` would read better and is a keyword — it is how an
    # `extern fn` names its library.)
    #
    # Offsets are bytes, like every other index into a string. UTF-8
    # is self-synchronising, so a match can only begin at a character
    # boundary and a returned offset is always one.
    unsafe fn find_from(&self, needle: &String, start: u64) -> Option<u64> {
        val n: u64 = self.len
        val m: u64 = needle.len
        if start > n { return Option::None }
        if m == 0u64 { return Option::Some(start) }
        if m > n - start { return Option::None }
        var i: u64 = start
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
            if matched { return Option::Some(i) }
            i = i + 1u64
        }
        Option::None
    }

    unsafe fn find(&self, needle: &String) -> Option<u64> {
        self.find_from(needle, 0u64)
    }

    # Byte offset of the **last** occurrence, or `None`. An empty
    # needle is found at `size()`, mirroring `find`'s answer of 0.
    unsafe fn rfind(&self, needle: &String) -> Option<u64> {
        val n: u64 = self.len
        val m: u64 = needle.len
        if m == 0u64 { return Option::Some(n) }
        if m > n { return Option::None }
        var i: u64 = n - m + 1u64
        while i > 0u64 {
            val at: u64 = i - 1u64
            var matched: bool = true
            var j: u64 = 0u64
            while j < m {
                val a: u8 = __builtin_ptr_read(self.data, at + j)
                val b: u8 = __builtin_ptr_read(needle.data, j)
                if a != b {
                    matched = false
                    break
                }
                j = j + 1u64
            }
            if matched { return Option::Some(at) }
            i = i - 1u64
        }
        Option::None
    }

    unsafe fn starts_with(&self, prefix: &String) -> bool {
        val m: u64 = prefix.len
        if m > self.len { return false }
        var j: u64 = 0u64
        while j < m {
            val a: u8 = __builtin_ptr_read(self.data, j)
            val b: u8 = __builtin_ptr_read(prefix.data, j)
            if a != b { return false }
            j = j + 1u64
        }
        true
    }

    unsafe fn ends_with(&self, suffix: &String) -> bool {
        val m: u64 = suffix.len
        val n: u64 = self.len
        if m > n { return false }
        val at: u64 = n - m
        var j: u64 = 0u64
        while j < m {
            val a: u8 = __builtin_ptr_read(self.data, at + j)
            val b: u8 = __builtin_ptr_read(suffix.data, j)
            if a != b { return false }
            j = j + 1u64
        }
        true
    }

    # Compare against a borrowed `str` without allocating a `String`
    # to hold it. `s == t` needs two `String`s; this is the version
    # for the far more common `s == "literal"`.
    unsafe fn eq_str(&self, other: str) -> bool {
        val n: u64 = other.len()
        if n != self.len { return false }
        val p: ptr = other.as_ptr()
        var i: u64 = 0u64
        while i < n {
            val a: u8 = __builtin_ptr_read(self.data, i)
            val b: u8 = __builtin_ptr_read(p, i)
            if a != b { return false }
            i = i + 1u64
        }
        true
    }

    # A new String with every occurrence of `pattern` replaced by
    # `replacement`. Non-overlapping, left to right. An empty
    # `pattern` panics: there is no useful reading of "replace nothing
    # everywhere".
    #
    # (`from` and `to` would read better and are both keywords -- one
    # names an `extern fn`'s library, the other is the range form of a
    # `for` loop.)
    unsafe fn replace(&self, pattern: &String, replacement: &String) -> String {
        assert(pattern.len > 0u64, "String::replace: the pattern must not be empty")
        var out: String = String::new()
        var i: u64 = 0u64
        val n: u64 = self.len
        val m: u64 = pattern.len
        while i < n {
            var matched: bool = false
            if i + m <= n {
                matched = true
                var j: u64 = 0u64
                while j < m {
                    val a: u8 = __builtin_ptr_read(self.data, i + j)
                    val b: u8 = __builtin_ptr_read(pattern.data, j)
                    if a != b {
                        matched = false
                        break
                    }
                    j = j + 1u64
                }
            }
            if matched {
                out.push_string(replacement)
                i = i + m
            } else {
                val c: u8 = __builtin_ptr_read(self.data, i)
                out.push(c)
                i = i + 1u64
            }
        }
        out
    }

    # `self` repeated `n` times. `n == 0` is the empty string.
    unsafe fn repeat(&self, n: u64) -> String {
        var out: String = String::new()
        var k: u64 = 0u64
        while k < n {
            out.push_string(self)
            k = k + 1u64
        }
        out
    }

    # Split on `\n`, dropping a single trailing `\r` from each line
    # so a CRLF file reads the same as an LF one. A trailing newline
    # does **not** produce a final empty line -- the convention every
    # line-oriented tool uses.
    unsafe fn lines(&self) -> Vec<String> {
        var out: Vec<String> = Vec::new()
        val n: u64 = self.len
        var start: u64 = 0u64
        var i: u64 = 0u64
        while i < n {
            val c: u8 = __builtin_ptr_read(self.data, i)
            if c == '\n' {
                var end: u64 = i
                if end > start {
                    val prev: u8 = __builtin_ptr_read(self.data, end - 1u64)
                    if prev == '\r' { end = end - 1u64 }
                }
                val line: String = self.substring(start, end)
                out.push(line)
                start = i + 1u64
            }
            i = i + 1u64
        }
        if start < n {
            val tail: String = self.substring(start, n)
            out.push(tail)
        }
        out
    }

    # Split on runs of ASCII whitespace, dropping empty parts. Unlike
    # `split(sep)` this treats a run as one separator, which is what
    # makes it useful for reading columns out of a line.
    unsafe fn split_whitespace(&self) -> Vec<String> {
        var out: Vec<String> = Vec::new()
        val n: u64 = self.len
        var i: u64 = 0u64
        while i < n {
            val c: u8 = __builtin_ptr_read(self.data, i)
            if c.is_ascii_space() {
                i = i + 1u64
                continue
            }
            val start: u64 = i
            while i < n {
                val b: u8 = __builtin_ptr_read(self.data, i)
                if b.is_ascii_space() { break }
                i = i + 1u64
            }
            val part: String = self.substring(start, i)
            out.push(part)
        }
        out
    }
}

impl String {
    # `parts` joined with `sep` between them, in order. The inverse of
    # `split`, and the reason it is an associated function on `String`
    # rather than a method on `Vec`: `Vec<T>` is generic over anything,
    # and a container should not grow a method that only exists for
    # one element type.
    unsafe fn join(parts: &Vec<String>, sep: &String) -> String {
        var out: String = String::new()
        var i: u64 = 0u64
        val n: u64 = parts.size()
        while i < n {
            if i > 0u64 { out.push_string(sep) }
            val part: String = parts.get(i)
            out.push_string(part)
            i = i + 1u64
        }
        out
    }
}

# ---------------------------------------------------------------------
# Codepoints (STDLIB-TEXT §7).
#
# The decoding half. Encoding (`push_char`) has been here since there
# was a `push_char`, because writing a string is what needed it first;
# reading one back a character at a time had no API at all.
#
# Same three fields as `StringIter`, so it costs nothing extra against
# the return-register budget. The difference is the step: one to four
# bytes, decided by the lead byte.
struct CharsIter {
    data: ptr,
    len: u64,
    index: u64,
}

impl String {
    # Borrow the string into an iterator over Unicode scalar values.
    # `iter()` is the byte-at-a-time view; this is the character one.
    #
    #     for c in s.chars() { ... }        # c: char (u32)
    #
    # `&self` keeps the caller's binding alive; the iterator shares
    # the buffer and is invalidated by a `push` that reallocates.
    fn chars(&self) -> CharsIter {
        CharsIter { data: self.data, len: self.len, index: 0u64 }
    }
}

impl Iterator<char> for CharsIter {
    # Advance by one codepoint.
    #
    # A `String` holds arbitrary bytes, so this can meet a sequence
    # that is not UTF-8. It answers **U+FFFD and advances one byte**,
    # the `from_utf8_lossy` convention -- `None` already means "the
    # end", so a failure cannot be reported there without making every
    # loop unable to tell a broken byte from a finished string. A
    # caller that needs to know asks `is_utf8()` first, the same
    # discipline `to_str` uses.
    #
    # On a `str`, whose bytes are valid UTF-8 by construction, U+FFFD
    # can only come back if it was actually written.
    unsafe fn next(&mut self) -> Option<char> {
        if self.index >= self.len {
            return Option::None
        }
        val b: u8 = __builtin_ptr_read(self.data, self.index)
        if b < 128u8 {
            self.index = self.index + 1u64
            return Option::Some(b as u32)
        }
        var need: u64 = 0u64
        var lo: u32 = 0u32
        var cp: u32 = 0u32
        if b >= 194u8 && b <= 223u8 {
            need = 1u64
            cp = (b as u32) - 192u32
            lo = 128u32
        } elif b >= 224u8 && b <= 239u8 {
            need = 2u64
            cp = (b as u32) - 224u32
            lo = 2048u32
        } elif b >= 240u8 && b <= 244u8 {
            need = 3u64
            cp = (b as u32) - 240u32
            lo = 65536u32
        } else {
            self.index = self.index + 1u64
            return Option::Some(65533u32)
        }
        if self.index + need >= self.len + 1u64 {
            self.index = self.index + 1u64
            return Option::Some(65533u32)
        }
        var k: u64 = 1u64
        while k <= need {
            val c: u8 = __builtin_ptr_read(self.data, self.index + k)
            if c < 128u8 || c > 191u8 {
                self.index = self.index + 1u64
                return Option::Some(65533u32)
            }
            cp = cp * 64u32 + ((c as u32) - 128u32)
            k = k + 1u64
        }
        # Over-long forms and surrogates are not scalar values, so they
        # are as invalid as a stray byte -- and rejecting them is what
        # keeps this in step with `is_utf8`.
        if cp < lo || (cp >= 55296u32 && cp <= 57343u32) || cp > 1114111u32 {
            self.index = self.index + 1u64
            return Option::Some(65533u32)
        }
        self.index = self.index + need + 1u64
        Option::Some(cp)
    }
}


# Iterator adapters (STDLIB-ITER-ADAPT): `map` / `filter` /
# `enumerate` / `collect` on a `StringIter` (one `u8` per byte).
# Same design as the `VecIter` adapters in
# `core/std/collections/vec.t` — ordinary structs exposing
# `fn next(&mut self) -> Option<T>`, type params kept out of every
# field. `collect` takes the iterator by value and drains it into a
# `Vec`; `enumerate` has no `collect` (tuple-element Vecs are not
# AOT-lowerable).

struct StringMapIter<U> {
    source: StringIter,
    f: fn (u8) -> U,
}

impl<U> StringMapIter<U> {

    fn collect(self: Self) -> Vec<U> {
        val out: Vec<U> = Vec::new()
        var it = self
        loop {
            match it.next() {
                Option::Some(v) => { out.push(v) }
                Option::None => { break }
            }
        }
        out
    }
}

impl<U> Iterator<U> for StringMapIter<U> {
    # Apply `f` to each byte on the way out.
    unsafe fn next(&mut self) -> Option<U> {
        match self.source.next() {
            Option::Some(b) => Option::Some(self.f(b)),
            Option::None => Option::None,
        }
    }
}


impl StringIter {
    fn map<U>(&self, f: fn (u8) -> U) -> StringMapIter<U> {
        val src: StringIter = StringIter {
            data: self.data,
            len: self.len,
            index: self.index,
        }
        StringMapIter { source: src, f: f }
    }
}

struct StringFilterIter {
    source: StringIter,
    pred: fn (u8) -> bool,
}

impl StringFilterIter {

    fn collect(self: Self) -> Vec<u8> {
        val out: Vec<u8> = Vec::new()
        var it = self
        loop {
            match it.next() {
                Option::Some(v) => { out.push(v) }
                Option::None => { break }
            }
        }
        out
    }
}

impl Iterator<u8> for StringFilterIter {
    # Yield only the bytes for which `pred` returns true.
    unsafe fn next(&mut self) -> Option<u8> {
        loop {
            match self.source.next() {
                Option::Some(b) => {
                    if self.pred(b) {
                        val r: Option<u8> = Option::Some(b)
                        return r
                    }
                    continue
                }
                Option::None => {
                    break
                }
            }
        }
        val r: Option<u8> = Option::None
        r
    }
}


impl StringIter {
    fn filter(&self, pred: fn (u8) -> bool) -> StringFilterIter {
        val src: StringIter = StringIter {
            data: self.data,
            len: self.len,
            index: self.index,
        }
        StringFilterIter { source: src, pred: pred }
    }
}

struct StringEnumerateIter {
    source: StringIter,
    index: u64,
}

impl Iterator<(u64, u8)> for StringEnumerateIter {
    # Yield `(index, byte)` pairs, starting at 0.
    unsafe fn next(&mut self) -> Option<(u64, u8)> {
        match self.source.next() {
            Option::Some(b) => {
                val i = self.index
                self.index = self.index + 1u64
                Option::Some((i, b))
            }
            Option::None => Option::None,
        }
    }
}


impl StringIter {
    fn enumerate(&self) -> StringEnumerateIter {
        val src: StringIter = StringIter {
            data: self.data,
            len: self.len,
            index: self.index,
        }
        StringEnumerateIter { source: src, index: 0u64 }
    }
}

# From/Into: `str -> String`. The blanket `Into` side (any
# `U: From<T>` gives `T: Into<U>`) is derived by the type checker at
# the `.into()` call site (`core/std/convert.t`), so only the `From`
# impl is written here. `s.into()` with a `String` expected type
# rewrites to `String::from(s)`.
impl From<str> for String {
    fn from(value: str) -> String {
        val r: String = String::from_str(value)
        r
    }
}

# STDLIB-ORD: byte-wise lexicographic ordering, the same comparison
# `==` (`eq`) does but on `<`. The shorter prefix is the smaller
# string; equal length means equal content. `Ord for String` lives
# here (the module owning the type), matching `eq` / `concat` / etc.
# FNV-1a over the bytes, with the constants `core/std/hash.t` pins for
# `str`, so `String::from_str("k").hash() == "k".hash()`. Written in
# toylang rather than routed through `__extern_str_hash`: the extern
# takes a `str`, and reaching one from a `String` means materialising
# a copy of the bytes — the allocation the str impl exists to avoid.
# `get` is a typed-slot read, so this walk allocates nothing.
impl Hash for String {
    fn hash(self: Self) -> u64 {
        val n: u64 = self.size()
        var h: u64 = 14695981039346656037u64
        var i: u64 = 0u64
        while i < n {
            val b: u8 = self.get(i)
            h = (h ^ (b as u64)) * 1099511628211u64
            i = i + 1u64
        }
        h
    }
}

impl Ord for String {
    fn lt(self: Self, other: Self) -> bool {
        val n: u64 = self.size()
        val m: u64 = other.size()
        val k: u64 = if n < m { n } else { m }
        var i: u64 = 0u64
        while i < k {
            val a: u8 = self.get(i)
            val b: u8 = other.get(i)
            if a != b {
                return a < b
            }
            i = i + 1u64
        }
        n < m
    }
}
