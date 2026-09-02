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
# ## Escape (WINDOW-ESCAPE, `[E0026]`)
#
# **A window may not outlive the buffer it views.** When the buffer is
# a binding in the same frame, the window cannot be returned, nor
# bound or assigned outside that binding's scope. Staying beside the
# buffer is fine — that is what a window is for — and a window on a
# *parameter* belongs to the caller, which is why `Vec::as_span(&self)`
# below hands one back and is correct.
#
# The check is REGION's (E0022) over a different owner and shares its
# pass. Two hazards stay uncovered: a window captured by a closure,
# and one held across a `push` that reallocates.

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

    # Pair a *raw* address with a length (CONV-SPAN). `None` for a
    # null address, so the window's `Ptr<T>` keeps the non-null
    # invariant (POINTER P5) rather than holding it by convention —
    # the same rule `Ptr::try_from_raw` follows, in one call.
    #
    # Nothing checks that `len` matches what the owner allocated, or
    # that the memory outlives the span; those stay the caller's, as
    # they are for `from_parts`.
    fn try_from_raw_parts(p: ptr, len: u64) -> Option<Self> {
        val window: Option<Ptr<T>> = Ptr::try_from_raw(p)
        match window {
            Option::Some(w) => Option::Some(Span { data: w, count: len }),
            Option::None => Option::None,
        }
    }

    # A sub-window: `len` elements starting at `offset`. **No copy** —
    # the result views the same memory, so a write through it is
    # visible through the original.
    #
    # This is what keeps splitting a buffer free: a parser handed
    # `buf.slice(0u64, n)` reads the bytes where they landed instead
    # of in a fresh `Vec`. Out-of-range bounds panic, like `get` —
    # an index mistake is a program error, not a value to inspect.
    fn slice(&self, offset: u64, len: u64) -> Span<T> {
        if offset > self.count { panic("Span::slice offset out of bounds") }
        # `self.count - offset` cannot underflow after the check
        # above, and phrasing the test this way avoids overflowing
        # `offset + len`.
        if len > self.count - offset { panic("Span::slice length out of bounds") }
        # Shift the address here rather than through
        # `self.data.offset(...)`: a method call on a field receiver
        # inside this generic impl sends the tree-walker into
        # unbounded recursion (todo SPAN-FIELD-METHOD-RECURSION). The
        # arithmetic is `Ptr::offset`'s, spelled out.
        val shifted: ptr =
            __builtin_ptr_offset(self.data.addr, offset * __builtin_sizeof::<T>())
        val base: Ptr<T> = Ptr { addr: shifted }
        Span { data: base, count: len }
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
