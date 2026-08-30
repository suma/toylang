# Stdlib `Vec<T>` — user-space dynamic array implemented entirely
# on top of the language's pointer primitives (`__builtin_heap_alloc`
# / `__builtin_heap_realloc` / `__builtin_ptr_read` /
# `__builtin_ptr_write` / `__builtin_sizeof`). No special-casing in
# the parser, the type checker, or any backend. Sibling to
# `core/std/dict.t` (`Dict<K, V>`).
#
# Auto-loaded from `<core>/std/collections/vec.t -> ["std",
# "collections", "vec"]`. Module name therefore is
# `std.collections.vec`. No `package` line — it would name the
# module `std.collections.vec`, but the auto-load integration
# already infers the path from the file system location, and
# matching `core/std/dict.t` etc. for consistency keeps the
# stdlib bodies free of redundant declarations.
#
# API:
#   - `Vec::new() -> Self`
#   - `v.push(value)` (`&mut self`) — append, geometric grow
#   - `v.pop() -> T` (`&mut self`) — remove last (panics when empty)
#   - `v.get(i) -> T` — random read (bounds-checked; panics)
#   - `v.set(i, value)` (`&mut self`) — random write (bounds
#     check)
#   - `v.size() -> u64` — current element count
#   - `v.capacity() -> u64` — allocated slots
#   - `v.is_empty() -> bool`
#
# Method `size` is named for symmetry with `core/std/dict.t::size`
# rather than Rust's `len` to dodge any potential clash with the
# `len` field of `Vec<T>` itself when method dispatch needs to
# resolve `v.len(...)`.
#
# Per-monomorph generic substitution (DICT-AOT-NEW Phase C) makes
# `__builtin_sizeof(value)` and `val: T = __builtin_ptr_read(...)`
# work for arbitrary T at AOT. `&mut self` Stage 1 propagates
# `self.cap = ...` / `self.data = ...` / `self.len = ...`
# mutations back to the caller's binding via the Self-out-parameter
# writeback convention.

struct Vec<T> {
    data: ptr,
    len: u64,
    cap: u64,
    elem_size: u64,
}

impl<T> Vec<T> {
    fn new() -> Self {
        Vec {
            data: __builtin_heap_alloc(0u64),
            len: 0u64,
            cap: 0u64,
            elem_size: 0u64,
        }
    }

    # Append. Geometric grow: 0 → 4 → 8 → 16 → ... so `n`
    # consecutive `push`es cost amortised O(1).
    fn push(&mut self, value: T) {
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof(value)
        }
        if self.cap == 0u64 {
            self.cap = 4u64
            self.data = __builtin_heap_realloc(self.data, self.cap * self.elem_size)
        } elif self.len >= self.cap {
            self.cap = self.cap * 2u64
            self.data = __builtin_heap_realloc(self.data, self.cap * self.elem_size)
        }
        __builtin_ptr_write(self.data, self.len * self.elem_size, value)
        self.len = self.len + 1u64
    }

    # Remove and return the last element. Panics on an empty Vec
    # (DEBUG-OBS D6) — it used to read whatever sat at offset 0 and
    # underflow `self.len` to `u64::MAX`, which turned one mistake
    # into a Vec that reports 18 quintillion elements.
    fn pop(&mut self) -> T {
        if self.len == 0u64 { panic("Vec::pop on an empty Vec") }
        self.len = self.len - 1u64
        val v: T = __builtin_ptr_read(self.data, self.len * self.elem_size)
        v
    }

    # Random-access read, bounds-checked (DEBUG-OBS D6).
    #
    # A built-in array traps on an out-of-range index (RUNTIME-TRAP);
    # this used to be the one indexed read that did not, and reading
    # past the end reached the host — `value not defined` from inside
    # the IR VM, with no toylang position or backtrace left.
    fn get(&self, index: u64) -> T {
        if index >= self.len { panic("Vec::get index out of bounds") }
        val v: T = __builtin_ptr_read(self.data, index * self.elem_size)
        v
    }

    # Random-access write, bounds-checked. `push` writes through the
    # raw pointer, so appending is not affected by this.
    fn set(&mut self, index: u64, value: T) {
        if index >= self.len { panic("Vec::set index out of bounds") }
        __builtin_ptr_write(self.data, index * self.elem_size, value)
    }

    fn size(&self) -> u64 {
        self.len
    }

    fn capacity(&self) -> u64 {
        self.cap
    }

    fn is_empty(&self) -> bool {
        self.len == 0u64
    }

    # Pointer to the underlying byte/element buffer. Used by
    # callers (e.g. `core/std/string.t::String::as_ptr`) that need
    # to read raw bytes through the active allocator's `ptr_read`
    # without crossing the `Vec` field-access privacy line.
    fn as_ptr(&self) -> ptr {
        self.data
    }

    # Logically clear the vec. Capacity / data buffer are kept so
    # a subsequent series of `push`es doesn't pay for the first
    # `heap_realloc`. To actually free the buffer the caller would
    # drop the binding — the `impl Drop for Vec<T>` below frees the
    # buffer (and the drop glue frees each element) when it dies.
    fn clear(&mut self) {
        self.len = 0u64
    }
}

# STDLIB-ORD: stable insertion sort over the `Ord` trait
# (`core/std/ord.t`). O(n^2) worst case — fine for the sizes toy
# programs sort, and much simpler than a generic partition (which
# would need swap-by-value and a 3-way comparison). The `<` on the
# element type resolves to the `lt` the `Ord` impl provides, so
# `Vec<u64>` / `Vec<i64>` / `Vec<f64>` / `Vec<bool>` and any
# `impl Ord` struct (including `String`, byte-wise) sort.
impl<T: Ord> Vec<T> {
    # Sort in place, ascending. Stable: equal elements keep their
    # relative order. Elements are read as copies out of the buffer
    # (like `get`), and `lt` takes `self: Self` which aliases rather
    # than moves, so `key` stays usable across the inner loop.
    #
    # Each `self.get(...)` is bound to a local before use — a
    # compound-returning method call directly in an expression
    # position (a `set` argument, a `lt` argument) cannot be
    # AOT-lowered for compound `T` (e.g. `Vec<String>`).
    fn sort(&mut self) {
        var i: u64 = 1u64
        while i < self.len {
            val key: T = self.get(i)
            var j: u64 = i
            while j > 0u64 {
                val prev: T = self.get(j - 1u64)
                if !key.lt(prev) {
                    break
                }
                self.set(j, prev)
                j = j - 1u64
            }
            self.set(j, key)
            i = i + 1u64
        }
    }
}

# DROP-GLUE: the buffer dies with the binding. The element values
# are glued by the backend *before* this runs (contents first, then
# the storage free), so a `Vec<Box<i64>>` releases every box when
# the vec goes out of scope. `data` is null for a never-grown vec
# (`Vec::new` allocates 0 bytes), and freeing null is a no-op.
impl<T> Drop for Vec<T> {
    fn drop(&mut self) {
        __builtin_heap_free(self.data)
    }
}

# Iterator-protocol support (STDLIB-ITER): `for x in v.iter() { ... }`
# works against any struct exposing `fn next(&mut self) -> Option<T>`
# (structural / duck-typed — the desugaring in
# `frontend/src/parser/stmt.rs::desugar_for_in_iterator` never checks
# for a `trait Iterator<T>` impl). The iterator snapshots the buffer
# pointer and walks it with the same `__builtin_ptr_read` the vec
# itself uses. `T` deliberately appears in no field (like `Box<T>`),
# so the struct needs no per-monomorph instantiation of its own.
# Mutating the vec while iterating is the caller's hazard (a realloc
# moves the buffer), same as Rust.
struct VecIter<T> {
    data: ptr,
    len: u64,
    elem_size: u64,
    index: u64,
}

impl<T> Vec<T> {
    # Borrow the vec into an iterator. `&self` keeps the caller's
    # binding alive; the returned iterator shares the buffer.
    fn iter(&self) -> VecIter<T> {
        VecIter {
            data: self.data,
            len: self.len,
            elem_size: self.elem_size,
            index: 0u64,
        }
    }
}

impl<T> VecIter<T> {
    # Advance by one element. Returns `None` once `index` has walked
    # past `len`. The element is read as a copy out of the buffer —
    # exactly like `Vec::get`, so compound `T` (including `Box`) is
    # an alias of the stored value.
    fn next(&mut self) -> Option<T> {
        if self.index >= self.len {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val e: T = __builtin_ptr_read(self.data, i * self.elem_size)
            Option::Some(e)
        }
    }
}

# Concrete-args impl: byte-vector helpers live here because the
# inner `__builtin_ptr_read(...)` produces `u8` and `push(value)`
# needs the receiver `Vec<T>`'s `T` to be `u8` for the push to
# type-check. CONCRETE-IMPL Phase 2 lets this `impl Vec<u8>` and
# the generic `impl<T> Vec<T>` above coexist in the registry.
impl Vec<u8> {
    # Bulk-copy a `str`'s UTF-8 bytes onto a fresh heap-allocated
    # `Vec<u8>`. Migration target for callers that used to
    # construct a `String` from a string literal — `String` is
    # now a `type` alias for `Vec<u8>`, so this is *the*
    # constructor for byte-string values.
    #
    # The trailing NUL terminator is intentionally NOT copied
    # (`size()` matches `s.len()` exactly). Bulk allocate +
    # memcpy:
    #   - AOT: `s.as_ptr()` is the byte_start of the `.rodata`
    #     `[bytes][NUL][u64 len]` layout; `__builtin_mem_copy`
    #     lowers to libc memcpy(3).
    #   - Interpreter: `s.as_ptr()` populates typed-slot `u8`
    #     entries; `HeapManager::copy_memory` is typed-slots-aware
    #     and propagates them to the destination buffer.
    #
    # The `heap_alloc(0) + heap_realloc(p, n)` pair handles
    # `n == 0` gracefully (realloc(p, 0) returns a freed/null-
    # equivalent pointer; mem_copy with size 0 is a no-op).
    fn from_str(s: str) -> Self {
        val n: u64 = s.len()
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        __builtin_mem_copy(s.as_ptr(), data, n)
        val result: Vec<u8> = Vec {
            data: data,
            len: n,
            cap: n,
            elem_size: 1u64,
        }
        result
    }

    # Append `count` bytes from `src` to the end of the vec.
    # Used by `push_str` below and any other caller that wants
    # bulk-append from a pointer source. Body delegates to `push`
    # per byte so the existing geometric grow logic kicks in
    # without needing pointer-arithmetic builtins (no
    # `__builtin_ptr_offset` exists today). For typical demo
    # workloads this is fine; a future bulk-`mem_copy` form
    # would be a perf optimisation.
    fn extend_bytes(&mut self, src: ptr, count: u64) {
        var i: u64 = 0u64
        while i < count {
            val b: u8 = __builtin_ptr_read(src, i)
            self.push(b)
            i = i + 1u64
        }
    }

    # Append the bytes of another `Vec<u8>` (i.e. `String`) to
    # `self` in-place. `other` is taken by reference (`&Vec<u8>`)
    # — REF-Stage-2 minimum subset: caller-side auto-borrow lets
    # `s.push_str(b)` work with `b: Vec<u8>`.
    fn push_str(&mut self, other: &Vec<u8>) {
        self.extend_bytes(other.as_ptr(), other.size())
    }

    # UTF-8 encode a Unicode codepoint and append the resulting 1-4
    # bytes. `c: char` is `u32` (see `core/std/char.t`); the encoding
    # follows RFC 3629:
    #
    #   < 0x80     -> 1 byte:  0xxxxxxx
    #   < 0x800    -> 2 bytes: 110xxxxx 10xxxxxx
    #   < 0x10000  -> 3 bytes: 1110xxxx 10xxxxxx 10xxxxxx
    #   < 0x110000 -> 4 bytes: 11110xxx 10xxxxxx 10xxxxxx 10xxxxxx
    #
    # Surrogate codepoints (U+D800..U+DFFF) and codepoints
    # >= U+110000 are not valid Unicode scalars and panic. Lexer
    # already rejects them in `'\u{...}'` literals via
    # `char::from_u32`; the runtime check guards values built from
    # arithmetic.
    #
    # Implementation notes: shifts intentionally use `u64` operands —
    # the type checker forces shift right-hand sides to `u64`
    # (`frontend/src/type_checker/utility.rs:128`), so we widen the
    # codepoint once and narrow each output byte with `as u8`.
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

    # Byte-wise equality. Two byte vectors are equal iff they
    # have the same length and every byte matches. Length check
    # first so different-sized vectors short-circuit without
    # walking the buffer. Both receivers are immutable references
    # — callers may pass either `Vec<u8>` (i.e. `String`) or
    # `&Vec<u8>` thanks to auto-borrow.
    fn eq(&self, other: &Vec<u8>) -> bool {
        val n: u64 = self.size()
        if n != other.size() {
            return false
        }
        val pa: ptr = self.as_ptr()
        val pb: ptr = other.as_ptr()
        # SIMD: 16 bytes per comparison while a whole chunk fits.
        # The bound is `i + 16 <= n`, never `i < n` -- a vector load
        # reads all 16 bytes, so a chunk straddling the end of the
        # buffer would read past the allocation.
        var i: u64 = 0u64
        while i + 16u64 <= n {
            val va: u8x16 = __simd_load(pa, i)
            val vb: u8x16 = __simd_load(pb, i)
            if !__simd_all(va == vb) {
                return false
            }
            i = i + 16u64
        }
        while i < n {
            val a: u8 = __builtin_ptr_read(pa, i)
            val b: u8 = __builtin_ptr_read(pb, i)
            if a != b {
                return false
            }
            i = i + 1u64
        }
        true
    }
}

# Iterator adapters (STDLIB-ITER-ADAPT): `map` / `filter` /
# `enumerate` / `zip` / `collect` on a `VecIter<T>`. Each adapter is a
# small struct that holds a snapshot of the source iterator plus the
# adapting function (or counter). They work with `for x in it.map(f)`
# through the same structural `next(&mut self) -> Option<T>` protocol
# the base iterators use, so the parser's for-loop desugaring needs no
# special-casing. Function fields are stored as `fn (T) -> U` values;
# the AOT / JIT backends dispatch them as field-closure calls.
#
# The adapter structs keep their type params OUT of any field (like
# `Box<T>`) so the JIT's non-parameterised `struct_layouts` table can
# lay them out without per-monomorph entries. `collect` takes the
# iterator by value (`self: Self`) rather than `&mut self` — the
# receiver writeback plus the `Vec` return would otherwise exceed the
# backend register budget (6 receiver fields + 4 Vec fields). Only
# scalar-element adapters (`VecIter` / `MapIter` / `FilterIter`) get
# `collect`: `Vec<(A, B)>` needs `__builtin_sizeof` on a tuple value,
# which the AOT backend cannot resolve yet.

struct MapIter<T, U> {
    source: VecIter<T>,
    f: fn (T) -> U,
}

impl<T, U> MapIter<T, U> {
    # Apply `f` to each element on the way out.
    fn next(&mut self) -> Option<U> {
        match self.source.next() {
            Option::Some(v) => Option::Some(self.f(v)),
            Option::None => Option::None,
        }
    }

    # Drain the mapped stream into a fresh `Vec<U>`. The iterator is
    # consumed by value, so the caller's binding keeps its state
    # (compound alias semantics); a second call would re-read from the
    # start.
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

impl<T> VecIter<T> {
    # `it.map(f)` yields `f(x)` for each element `x`.
    fn map<U>(&self, f: fn (T) -> U) -> MapIter<T, U> {
        val src: VecIter<T> = VecIter {
            data: self.data,
            len: self.len,
            elem_size: self.elem_size,
            index: self.index,
        }
        MapIter { source: src, f: f }
    }
}

struct FilterIter<T> {
    source: VecIter<T>,
    pred: fn (T) -> bool,
}

impl<T> FilterIter<T> {
    # Yield only the elements for which `pred` returns true.
    fn next(&mut self) -> Option<T> {
        loop {
            match self.source.next() {
                Option::Some(v) => {
                    if self.pred(v) {
                        val r: Option<T> = Option::Some(v)
                        return r
                    }
                    continue
                }
                Option::None => {
                    break
                }
            }
        }
        val r: Option<T> = Option::None
        r
    }

    fn collect(self: Self) -> Vec<T> {
        val out: Vec<T> = Vec::new()
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

impl<T> VecIter<T> {
    fn filter(&self, pred: fn (T) -> bool) -> FilterIter<T> {
        val src: VecIter<T> = VecIter {
            data: self.data,
            len: self.len,
            elem_size: self.elem_size,
            index: self.index,
        }
        FilterIter { source: src, pred: pred }
    }
}

struct EnumerateIter<T> {
    source: VecIter<T>,
    index: u64,
}

impl<T> EnumerateIter<T> {
    # Yield `(index, element)` pairs, starting at 0.
    fn next(&mut self) -> Option<(u64, T)> {
        match self.source.next() {
            Option::Some(v) => {
                val i = self.index
                self.index = self.index + 1u64
                Option::Some((i, v))
            }
            Option::None => Option::None,
        }
    }
}

impl<T> VecIter<T> {
    fn enumerate(&self) -> EnumerateIter<T> {
        val src: VecIter<T> = VecIter {
            data: self.data,
            len: self.len,
            elem_size: self.elem_size,
            index: self.index,
        }
        EnumerateIter { source: src, index: 0u64 }
    }
}

struct ZipIter<A, B> {
    a_data: ptr,
    b_data: ptr,
    min_len: u64,
    elems: u64,
    index: u64,
}

impl<A, B> ZipIter<A, B> {
    # Yield `(a, b)` pairs, stopping at the shorter of the two
    # sources. `elems` packs the two element strides into one field
    # (`a_elem << 32 | b_elem`) so the struct fits in the backend's
    # receiver-writeback register budget (the same trick `DictIter`
    # uses for its key/value sizes).
    fn next(&mut self) -> Option<(A, B)> {
        if self.index >= self.min_len {
            Option::None
        } else {
            val a_elem = self.elems >> 32u64
            val b_elem = self.elems & 0xFFFFFFFFu64
            val ai: A = __builtin_ptr_read(self.a_data, self.index * a_elem)
            val bi: B = __builtin_ptr_read(self.b_data, self.index * b_elem)
            self.index = self.index + 1u64
            Option::Some((ai, bi))
        }
    }
}

impl<T> VecIter<T> {
    fn zip<U>(&self, other: VecIter<U>) -> ZipIter<T, U> {
        val ml = if self.len < other.len { self.len } else { other.len }
        ZipIter {
            a_data: self.data,
            b_data: other.data,
            min_len: ml,
            elems: (self.elem_size << 32u64) | other.elem_size,
            index: 0u64,
        }
    }
}
