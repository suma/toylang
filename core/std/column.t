# Stdlib `Column<T>` — DATA-ORIENTED Phase 1.
#
# A window onto **one field of every element** of an array:
#
#     struct Particle { x: f64, y: f64, mass: f64 }
#     val ps: soa [Particle; 1024] = ...
#     val ms = ps.mass                  # Column<f64>, 1024 long
#     total(ms)                         # fn total(ms: Column<f64>) -> f64
#
# This is the piece the design doc calls the slice `&[T]`: without it
# a column can be looped over but not *passed*, so "the function that
# only reads mass" cannot be written. `ps.mass` is produced by the
# compiler (there is no source-level way to name a column's address),
# and everything below is ordinary toylang over it.
#
# The window carries its own `stride` rather than assuming elements
# are adjacent, which is what lets the *same* type describe a column
# of either layout: under `soa` the field's values are contiguous
# (stride = the field's width), under the interleaved default they are
# one element apart (stride = the element's size). A program can
# therefore be measured with `soa` on and off without its function
# signatures changing — the whole point of the modifier.
#
# The stride is not readable from the API, deliberately: it is a fact
# about placement, and placement is what the tree-walking interpreter
# does not model (it holds arrays as values, not as memory). A
# `stride()` accessor would be the one call whose answer differed by
# engine.
#
# It is a **view, not an owner**: no `Drop`, and it does not keep the
# array alive. Like `Span<T>` (`core/std/span.t`, whose escape rule
# this shares) it must not outlive what it points at.
#
# There is deliberately no `as_ptr` / `as_raw`. A column is addressable
# on the compiled lanes and not on the tree-walking interpreter, which
# holds arrays as values rather than as memory; keeping the address in
# is what lets one type mean the same thing on every engine. A SIMD
# receptacle (`__simd_load` over a unit-stride column) therefore waits
# for the address question to be answered for all four lanes.

struct Column<T> {
    addr: ptr,
    len: u64,
    stride: u64,
}

impl<T> Column<T> {
    # Read element `index` of the column. Bounds-checked, panicking
    # like `Vec::get` / `Span::get` rather than reading past the end.
    unsafe fn get(&self, index: u64) -> T {
        if index >= self.len { panic("Column::get index out of bounds") }
        val v: T = __builtin_ptr_read::<T>(self.addr, index * self.stride)
        v
    }

    # Name an element without taking it (ELEMENT-BORROW). The stride
    # is the column's, so this is the same address `get` reads.
    unsafe fn borrow(&self, index: u64) -> &T {
        if index >= self.len { panic("Column::borrow index out of bounds") }
        val e: &T = __builtin_ptr_ref::<T>(self.addr, index * self.stride)
        e
    }

    # Write through the window — the array behind it changes.
    unsafe fn set(&mut self, index: u64, value: T) {
        if index >= self.len { panic("Column::set index out of bounds") }
        __builtin_ptr_write(self.addr, index * self.stride, value)
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0u64
    }
}
