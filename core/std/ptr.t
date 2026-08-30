# Stdlib `Ptr<T>` — a typed window over raw memory (POINTER P3).
#
# Raw `ptr` is C's `void*`: the read's shape comes from an annotation,
# two windows over different types are the same type, and the stride is
# hand-multiplied at every access. `Ptr<T>` carries the pointee in the
# type, so the element size and the read/write shape come from `T`:
#
#     val p: Ptr<u64> = Ptr::alloc(3u64)
#     p.set(0u64, 7u64)
#     val v: u64 = p.get(0u64)     # or p[0u64]
#
# Nothing here is special-cased in the parser, the type checker or any
# backend: the struct holds a bare `addr: ptr` field (`T` never appears
# by value), `__builtin_sizeof::<T>()` (POINTER P1) answers the stride,
# and `__getitem__` / `__setitem__` (POINTER P2) are the bracket sugar
# for `get` / `set`. See design-docs/POINTER.md for the layer map.
#
# ## What this is not
#
# - **Not owning.** `alloc` sizes a buffer, but nothing frees it —
#   there is no `impl Drop` here (P5, ownership, is a separate phase).
#   Free with `__builtin_heap_free(p.as_raw())` when you own the
#   allocation.
# - **Not bounds-checked.** `get` / `set` trust the index, exactly like
#   the raw builtins underneath. `Span<T>` (P4) is the bounds-checked
#   sibling.
#
# ## Non-null
#
# A `Ptr<T>` value is **non-null** — the module's constructors keep
# it that way (`alloc` rounds an empty request up so the address is
# never 0, and every backend answers `heap_alloc(0)` with null).
# Absence is modelled one level up, as `Option<Ptr<T>>` (POINTER P5)
# — the `next: ptr` + `has_next: bool` pairing a raw-`ptr` linked
# list needs is exactly what that replaces. The invariant is by
# construction here, not compiler-enforced: struct field visibility
# is recorded but not enforced, so a hand-written
# `Ptr { addr: ... }` literal is the raw-pointer escape hatch.
#
# ## API
#
#   - `Ptr::alloc(count) -> Self` — heap buffer for `count` elements
#     (`count * sizeof::<T>()` bytes, at least 1 byte). The caller
#     owns it.
#   - `p.get(i) -> T` / `p.set(i, value)` — element read / write at
#     element index `i` (byte offset `i * sizeof::<T>()`).
#   - `p[i]` / `p[i] = v` — same thing through bracket syntax.
#   - `p.offset(count) -> Self` — a window `count` elements forward.
#     The original window stays valid; the two share the allocation.
#   - `p.as_raw() -> ptr` — the bare address, for code that needs it.

struct Ptr<T> {
    # Address of element 0. `T` deliberately does not appear in any
    # field: that is what keeps the struct's layout finite and the
    # recursion / move checks uninterested.
    addr: ptr,
}

impl<T> Ptr<T> {
    # Allocate a heap buffer for `count` elements. The stride comes
    # from the written type, so no representative value is needed
    # (`__builtin_sizeof` with a value used to force `alloc` to take a
    # `proto: T` argument — POINTER.md 実測 2).
    #
    # Non-null (P5): every backend answers `heap_alloc(0)` with null,
    # so an empty request is rounded up to one byte. `sizeof::<T>()`
    # never under-counts for a real element type; the `== 0` guard
    # only fires for `count == 0` (or a hypothetical zero-width `T`).
    fn alloc(count: u64) -> Self {
        val bytes: u64 = __builtin_sizeof::<T>() * count
        val n: u64 = if bytes == 0u64 { 1u64 } else { bytes }
        val p: ptr = __builtin_heap_alloc(n)
        Ptr { addr: p }
    }

    # Element read. The annotation is the read's shape; the stride is
    # the type's, so the two can no longer disagree the way
    # `val v: f64 = __builtin_ptr_read(vec.data, i * 8u64)` could.
    fn get(&self, i: u64) -> T {
        val v: T = __builtin_ptr_read(self.addr, i * __builtin_sizeof::<T>())
        v
    }

    # Element write. `set` does not touch the struct's own fields, so
    # a shared `&self` is honest — the write goes through the raw
    # address, and every window over the same allocation sees it.
    fn set(&self, i: u64, value: T) {
        __builtin_ptr_write(self.addr, i * __builtin_sizeof::<T>(), value)
    }

    # A window `count` elements forward. The result shares the
    # allocation with `self`; neither frees anything.
    fn offset(self: Self, count: u64) -> Self {
        val shifted: ptr = __builtin_ptr_offset(self.addr, count * __builtin_sizeof::<T>())
        Ptr { addr: shifted }
    }

    # The bare address, for code (or a `Span<T>`) that wants to carry
    # it without the type.
    fn as_raw(self: Self) -> ptr {
        self.addr
    }

    # Bracket sugar. `p[i]` and `p.set` are the same operations; the
    # compiled lanes lower the bracket forms to these calls (P2).
    fn __getitem__(&self, i: u64) -> T {
        val v: T = __builtin_ptr_read(self.addr, i * __builtin_sizeof::<T>())
        v
    }

    fn __setitem__(&self, i: u64, value: T) {
        __builtin_ptr_write(self.addr, i * __builtin_sizeof::<T>(), value)
    }
}
