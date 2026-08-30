# Stdlib `Span<T>` — a bounds-checked view over a `Ptr<T>` window
# (POINTER P4).
#
# `Ptr<T>` knows the pointee's type but not the length, so every
# container still carries its own bounds check and a `Ptr<T>` handed
# to a function arrives without one. `Span<T>` pairs the window with
# a length:
#
#     val p: Ptr<u64> = Ptr::alloc(4u64)
#     val s: Span<u64> = Span::from_parts(p, 4u64)
#     s.set(0u64, 7u64)
#     val v: u64 = s[2u64]           # or s.get(2u64)
#
# Out-of-range access panics with a message naming the operation and
# the length — the same contract a built-in array's trap has
# (RUNTIME-TRAP) and `Vec::get` has had since DEBUG-OBS D6.
#
# ## What this is
#
# - **A view, not an owner.** `from_parts` copies a `(Ptr<T>, len)`
#   pair; nothing is allocated or freed. Freeing stays with whoever
#   owns the allocation (`__builtin_heap_free(p.as_raw())`).
# - **The library answer to `&[T]`** (todo NEW-TYPE-SYSTEM): a span
#   crosses function boundaries the way a slice would, and
#   `s.as_raw()` is the element-indexed address `__simd_load` /
#   `__simd_store` want.
#
# ## What this is not (POINTER.md 未解決論点, 選択肢 1 = 現状の既定)
#
# **Escape is not checked.** Nothing stops a `Span<T>` from being
# returned or stored in a field that outlives the memory it views —
# the language's escape rule (REF-Stage-2) applies to `&T` only, and
# the region check (REGIONS, E0022) only chases scoped-allocator
# origins. A dangling span reads whatever sits at the address, same
# as a dangling raw `ptr`. If that becomes a real failure mode,
# `ref struct`-style escape markers (POINTER.md 選択肢 2) are the
# follow-up.

struct Span<T> {
    # The window the elements live behind. A `Ptr<T>` field, so the
    # stride knowledge stays in the type.
    data: Ptr<T>,
    # Number of elements visible through `data`. `len` is a method
    # name too; the field wins on field access, the method on call.
    count: u64,
}

impl<T> Span<T> {
    # Pair a window with its length. The span does not own the
    # allocation; `len` must be the number of elements the owner
    # actually made room for.
    fn from_parts(p: Ptr<T>, len: u64) -> Self {
        Span { data: p, count: len }
    }

    # Bounds-checked element read. Panics naming the length on an
    # out-of-range index — the message shape is the same on every
    # backend (the `Vec::get` convention).
    unsafe fn get(&self, i: u64) -> T {
        if i >= self.count { panic("Span::get index out of bounds") }
        val v: T = __builtin_ptr_read(self.data.addr, i * __builtin_sizeof::<T>())
        v
    }

    # Bounds-checked element write.
    unsafe fn set(&self, i: u64, value: T) {
        if i >= self.count { panic("Span::set index out of bounds") }
        __builtin_ptr_write(self.data.addr, i * __builtin_sizeof::<T>(), value)
    }

    # Number of elements in the view.
    fn len(&self) -> u64 {
        self.count
    }

    fn is_empty(&self) -> bool {
        self.count == 0u64
    }

    # The window itself, for code that wants to keep the typed
    # pointer (e.g. to hand the allocation back to its owner).
    fn as_ptr(self: Self) -> Ptr<T> {
        self.data
    }

    # The bare element-0 address. This is what `__simd_load` /
    # `__simd_store` take (element-indexed addressing from there).
    fn as_raw(self: Self) -> ptr {
        self.data.addr
    }

    # Bracket sugar — the same bounds-checked operations under
    # indexing syntax.
    unsafe fn __getitem__(&self, i: u64) -> T {
        if i >= self.count { panic("Span::get index out of bounds") }
        val v: T = __builtin_ptr_read(self.data.addr, i * __builtin_sizeof::<T>())
        v
    }

    unsafe fn __setitem__(&self, i: u64, value: T) {
        if i >= self.count { panic("Span::set index out of bounds") }
        __builtin_ptr_write(self.data.addr, i * __builtin_sizeof::<T>(), value)
    }
}
