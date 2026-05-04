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
#   - `v.pop() -> T` (`&mut self`) — remove last (caller ensures
#     non-empty; reads garbage when called on empty Vec)
#   - `v.get(i) -> T` — random read (no bounds check)
#   - `v.set(i, value)` (`&mut self`) — random write (no bounds
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

    # Remove and return the last element. Pre: `self.len > 0u64`
    # (caller's responsibility). Calling on an empty Vec reads
    # garbage from the slot at offset 0 and underflows `self.len`
    # to `u64::MAX`.
    fn pop(&mut self) -> T {
        self.len = self.len - 1u64
        val v: T = __builtin_ptr_read(self.data, self.len * self.elem_size)
        v
    }

    # Random-access read. No bounds check — caller is responsible
    # for `index < self.len`. Returns whatever bytes happen to live
    # at the slot when called out-of-range.
    fn get(&self, index: u64) -> T {
        val v: T = __builtin_ptr_read(self.data, index * self.elem_size)
        v
    }

    # Random-access write. No bounds check.
    fn set(&mut self, index: u64, value: T) {
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
    # drop the binding and let the active allocator reclaim it.
    fn clear(&mut self) {
        self.len = 0u64
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

    # Append a single byte. Equivalent to `push` but the named
    # variant documents intent (and parallels the legacy
    # `String::push_char` API). `c: char` uses the alias from
    # `core/std/char.t` — the cross-module alias-resolution pass
    # (`frontend::resolve_type_aliases`) substitutes it to `u8`
    # at type-check time so dispatch picks up the existing
    # generic `Vec<T>::push` body.
    fn push_char(&mut self, c: char) {
        self.push(c)
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
        var i: u64 = 0u64
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
