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
    fn get(self: Self, index: u64) -> T {
        val v: T = __builtin_ptr_read(self.data, index * self.elem_size)
        v
    }

    # Random-access write. No bounds check.
    fn set(&mut self, index: u64, value: T) {
        __builtin_ptr_write(self.data, index * self.elem_size, value)
    }

    fn size(self: Self) -> u64 {
        self.len
    }

    fn capacity(self: Self) -> u64 {
        self.cap
    }

    fn is_empty(self: Self) -> bool {
        self.len == 0u64
    }

    # Pointer to the underlying byte/element buffer. Used by
    # callers (e.g. `core/std/string.t::String::as_ptr`) that need
    # to read raw bytes through the active allocator's `ptr_read`
    # without crossing the `Vec` field-access privacy line.
    fn as_ptr(self: Self) -> ptr {
        self.data
    }
}

# Concrete-args impl: byte-vector helpers live here because the
# inner `__builtin_ptr_read(...)` produces `u8` and `push(value)`
# needs the receiver `Vec<T>`'s `T` to be `u8` for the push to
# type-check. CONCRETE-IMPL Phase 2 lets this `impl Vec<u8>` and
# the generic `impl<T> Vec<T>` above coexist in the registry.
impl Vec<u8> {
    # Append `count` bytes from `src` to the end of the vec.
    # Used by `core/std/string.t::String::push_str` and any other
    # caller that wants bulk-append from a pointer source. Body
    # delegates to `push` per byte so the existing geometric grow
    # logic kicks in without needing pointer-arithmetic builtins
    # (no `__builtin_ptr_offset` exists today). For typical demo
    # workloads this is fine; a future bulk-`mem_copy` form would
    # be a perf optimisation.
    fn extend_bytes(&mut self, src: ptr, count: u64) {
        var i: u64 = 0u64
        while i < count {
            val b: u8 = __builtin_ptr_read(src, i)
            self.push(b)
            i = i + 1u64
        }
    }
}

# Note: `str → Vec<u8>` (heap-buffer) construction lives in
# `core/std/string.t::String::from_str(s)`. The `String` struct
# wraps a `Vec<u8>` and exposes the heap-managed-byte-buffer
# operations (`String::new`, `String::from_str`, `len`, `as_ptr`).
