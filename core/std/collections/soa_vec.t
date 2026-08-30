# Stdlib `SoaVec<T>` — DATA-ORIENTED Phase 2.
#
# The heap sibling of `soa [T; N]` (Phase 0): a dynamic array whose
# elements are stored **by column**. One allocation is divided into
# one column per leaf scalar of `T`, so every `x` sits next to every
# other `x`:
#
#     buffer: [col_0 x cap][col_1 x cap] ... [col_{k-1} x cap]
#     leaf j of element i  ->  prefix_j * cap + i * stride_j
#
# The arithmetic lives in `__builtin_soa_read` / `__builtin_soa_write`
# because `prefix_j` and `stride_j` are per-leaf constants a generic
# body cannot name — the same reason `Vec<T>` reaches for
# `__builtin_ptr_read` / `__builtin_ptr_write` rather than computing
# field offsets itself. Everything above that line is ordinary
# toylang, and no backend knows this type exists.
#
# Spelled `soa Vec<T>` in source; the parser rewrites the annotation
# to `SoaVec<T>` (`design-docs/DATA_ORIENTED.md`). Unlike the stack
# form, this is **its own type** rather than a layout flag on `Vec<T>`
# — a heap container's layout is observable through `as_ptr`, through
# what a grow has to copy, and through `retains(N)` once columns pack
# tightly, so one type cannot answer for both without a runtime tag.
#
# API — deliberately identical to `Vec<T>`, so switching a program
# between the two is one annotation:
#   - `SoaVec::new() -> Self`
#   - `v.push(value)` (`&mut self`) — append, geometric grow
#   - `v.pop() -> T` (`&mut self`) — remove last (panics when empty)
#   - `v.get(i) -> T` — random read (bounds-checked; panics)
#   - `v.set(i, value)` (`&mut self`) — random write (bounds-checked)
#   - `v.size() -> u64` / `v.capacity() -> u64` / `v.is_empty() -> bool`
#   - `v.clear()` (`&mut self`)
#   - `v.iter() -> SoaVecIter<T>` — the iterator protocol
#
# `as_ptr` is deliberately **absent**: the buffer's bytes mean
# something different here, and handing them out under `Vec`'s name
# would be the one place the two types could be confused.

struct SoaVec<T> {
    data: ptr,
    len: u64,
    cap: u64,
    elem_size: u64,
}

impl<T> SoaVec<T> {
    fn new() -> Self {
        SoaVec {
            data: __builtin_heap_alloc(0u64),
            len: 0u64,
            cap: 0u64,
            elem_size: 0u64,
        }
    }

    # Append. Geometric grow: 0 -> 4 -> 8 -> 16 -> ...
    #
    # A grow cannot be a `heap_realloc`. Column `j` starts at
    # `prefix_j * cap`, so growing the capacity moves every column but
    # the first: the old `[x x x x][y y y y]` has to become
    # `[x x x x _ _ _ _][y y y y _ _ _ _]`, which a resize-in-place
    # would leave reading `y`s where the second half of `x` now
    # belongs. So the elements are moved one at a time into the fresh
    # buffer, each landing at its new capacity's offsets.
    unsafe fn push(&mut self, value: T) {
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof(value)
        }
        if self.len >= self.cap {
            var new_cap: u64 = 4u64
            if self.cap > 0u64 {
                new_cap = self.cap * 2u64
            }
            val fresh = __builtin_heap_alloc(new_cap * self.elem_size)
            var i: u64 = 0u64
            while i < self.len {
                val moved: T = __builtin_soa_read(self.data, i, self.cap)
                __builtin_soa_write(fresh, i, new_cap, moved)
                i = i + 1u64
            }
            __builtin_heap_free(self.data)
            self.data = fresh
            self.cap = new_cap
        }
        __builtin_soa_write(self.data, self.len, self.cap, value)
        self.len = self.len + 1u64
    }

    # Remove and return the last element. Panics on an empty vec,
    # matching `Vec::pop` (DEBUG-OBS D6).
    unsafe fn pop(&mut self) -> T {
        if self.len == 0u64 { panic("SoaVec::pop on an empty SoaVec") }
        self.len = self.len - 1u64
        val v: T = __builtin_soa_read(self.data, self.len, self.cap)
        v
    }

    # Random-access read, bounds-checked. Reads every column of the
    # element — the whole value is what the caller asked for. Reading
    # *one* field without touching the other columns is what Phase 1
    # (`&[T]` windows onto a column) is for.
    unsafe fn get(&self, index: u64) -> T {
        if index >= self.len { panic("SoaVec::get index out of bounds") }
        val v: T = __builtin_soa_read(self.data, index, self.cap)
        v
    }

    unsafe fn set(&mut self, index: u64, value: T) {
        if index >= self.len { panic("SoaVec::set index out of bounds") }
        __builtin_soa_write(self.data, index, self.cap, value)
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

    # Logically clear. The buffer and its capacity are kept, exactly
    # as `Vec::clear` keeps them.
    fn clear(&mut self) {
        self.len = 0u64
    }
}

# DROP-GLUE: the buffer dies with the binding, and the elements die
# first. The backend's glue walks the columns for this type the same
# way it walks the interleaved buffer for `Vec<T>`
# (`compiler_lower/src/drop_glue.rs`), so a `SoaVec<Box<i64>>`
# releases every box.
impl<T> Drop for SoaVec<T> {
    fn drop(&mut self) {
        __builtin_heap_free(self.data)
    }
}

# Iterator protocol (STDLIB-ITER), same shape as `VecIter<T>`: a
# snapshot of the buffer walked by `next`. `cap` rides along because
# it is half of every column address — an iterator over a vec that
# grows underneath it is the caller's hazard, exactly as for `Vec`.
struct SoaVecIter<T> {
    data: ptr,
    len: u64,
    cap: u64,
    index: u64,
}

impl<T> SoaVec<T> {
    fn iter(&self) -> SoaVecIter<T> {
        SoaVecIter {
            data: self.data,
            len: self.len,
            cap: self.cap,
            index: 0u64,
        }
    }
}

impl<T> SoaVecIter<T> {
    unsafe fn next(&mut self) -> Option<T> {
        if self.index >= self.len {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val v: T = __builtin_soa_read(self.data, i, self.cap)
            Option::Some(v)
        }
    }
}
