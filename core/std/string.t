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
        if self.len == 0u64 { panic("String::pop on an empty String") }
        self.len = self.len - 1u64
        val b: u8 = __builtin_ptr_read(self.data, self.len)
        b
    }

    # Random read, bounds-checked (DEBUG-OBS D6). Reading past the end
    # used to reach the host rather than fail as a toylang program.
    fn get(&self, i: u64) -> u8 {
        if i >= self.len { panic("String::get index out of bounds") }
        val b: u8 = __builtin_ptr_read(self.data, i)
        b
    }

    # Random write, bounds-checked. `push` writes through the raw
    # pointer, so appending is not affected by this.
    fn set(&mut self, i: u64, b: u8) {
        if i >= self.len { panic("String::set index out of bounds") }
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

    # Shared body of `to_upper` / `to_lower` (CaseConvert). Copies
    # the bytes, then adds or subtracts 0x20 on every byte inside
    # `[lo, hi]`, leaving the rest untouched -- so bytes outside
    # `a-z` / `A-Z`, including every continuation byte of a
    # multi-byte UTF-8 sequence, pass through unchanged.
    #
    # `up` picks the direction: subtract to reach uppercase, add to
    # reach lowercase.
    fn fold_ascii_case(&self, lo: u8, hi: u8, up: bool) -> String {
        val n: u64 = self.len
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
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

# `Display` — render the buffer's bytes as a `str`, so `println(s)`
# and `"{s}"` show the text rather than the struct's fields. Without
# this, the stdlib's own string type printed as
# `String { cap: 2, data: 12, elem_size: 1, len: 2 }`.
#
# `__builtin_str_from_bytes` copies, so the result does not alias the
# buffer and is unaffected by a later `push`.
impl Display for String {
    fn to_str(&self) -> str {
        __builtin_str_from_bytes(self.data, self.len)
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
    fn to_upper(&self) -> String {
        val r: String = self.fold_ascii_case(0x61u8, 0x7Au8, true)
        r
    }

    fn to_lower(&self) -> String {
        val r: String = self.fold_ascii_case(0x41u8, 0x5Au8, false)
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

# `contains(needle)` — O(n * m) worst case, but the scan for a
# candidate start is done 16 bytes at a time (memchr's trick).
# Empty `needle` matches at position 0 (Rust / libc convention).
#
# The skip is sound because a match starting anywhere in
# `[i, i+16)` would have to begin with `needle`'s first byte: if no
# lane in that window equals it, all sixteen positions can be
# discarded at once.
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
    fn split(&self, sep: &String) -> Vec<String> {
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

impl StringIter {
    # Advance by one byte. Returns `None` once `index` has walked
    # past `len`.
    fn next(&mut self) -> Option<u8> {
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
    # Apply `f` to each byte on the way out.
    fn next(&mut self) -> Option<U> {
        match self.source.next() {
            Option::Some(b) => Option::Some(self.f(b)),
            Option::None => Option::None,
        }
    }

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
    # Yield only the bytes for which `pred` returns true.
    fn next(&mut self) -> Option<u8> {
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

impl StringEnumerateIter {
    # Yield `(index, byte)` pairs, starting at 0.
    fn next(&mut self) -> Option<(u64, u8)> {
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
