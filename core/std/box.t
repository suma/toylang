# Stdlib `Box<T>` — one heap-allocated `T`, owned by the binding that
# built it.
#
# `Box` exists for the shape the language otherwise cannot express: a
# type that contains itself. A field or payload holding a `T` by value
# has no finite layout and is refused ([E0013]); a field holding a
# `Box<T>` is a pointer, so the recursion lives in the values.
#
#     enum List {
#         Cons(i64, Box<List>),
#         Nil,
#     }
#
#     struct Tree {
#         v: i64,
#         left: Box<Tree>,
#         has_left: bool,
#     }
#
# Nothing here is special-cased in the parser, the type checker or any
# backend: `T` never appears in a field, only behind `data`, and the
# recursion check knows that a type argument the target does not hold
# by value is not containment.
#
# ## Ownership
#
# `Box` has an `impl Drop`, so the scope that built one frees it on the
# way out — and handing it to something that outlives that scope
# transfers ownership, after which the old name is an error to read
# ([E0014]). See "Ownership" in docs/language.md.
#
# The drop is recursive (DROP-GLUE): when a `Box` dies, its slot's
# contents are freed first — so `Box<Box<i64>>` and a boxed list free
# everything they hold, down to the innermost value — and then the slot
# itself. A value reachable through several aliases (a `get()` copy, a
# shared boxed node) is freed once; later visits are idempotent no-ops.
#
# ## API
#
#   - `Box::new(value) -> Self` — move `value` onto the heap
#   - `b.get() -> T` — read a copy of the boxed value
#   - `b.set(value)` (`&mut self`) — overwrite it
#   - `b.as_ptr() -> ptr` — the raw address, for code that needs it

struct Box<T> {
    # Address of the single `T`. `T` deliberately does not appear in
    # any field: that is what makes `Box<Self>` legal inside the very
    # type being declared.
    data: ptr,
}

# `Clone` for a Box: a second heap cell holding a clone of the value,
# freed independently of the first.
impl<T: Clone> Clone for Box<T> {
    fn clone(&self) -> Self {
        val v: &T = self.borrow()
        val c: T = v.clone()
        val b: Box<T> = Box::new(c)
        b
    }
}

impl<T> Box<T> {
    fn new(value: T) -> Self {
        val bytes: u64 = __builtin_sizeof::<T>()
        val p: ptr = __builtin_heap_alloc(bytes)
        # ERROR_MODEL D5: notice the failure rather than writing the
        # value through address 0. A zero-size `T` legitimately gets
        # null back (`core/std/ptr.t`), so only a non-zero request can
        # have failed.
        if bytes > 0u64 && __builtin_ptr_is_null(p) {
            panic("Box::new: allocation failed ({bytes} bytes)")
        }
        val cell: Ptr<T> = Ptr { addr: p }
        cell.set(0u64, value)
        Box { data: p }
    }

    # A copy of the boxed value. The annotation is what gives the read
    # its shape, so it cannot be dropped.
    fn get(&self) -> T {
        val cell: Ptr<T> = Ptr { addr: self.data }
        val v: T = cell.get(0u64)
        v
    }

    # Name the boxed value without taking it (ELEMENT-BORROW). `get`
    # answers a value that shares the box's resource when `T` owns one,
    # so a binding of it would free what the box still holds.
    fn borrow(&self) -> &T {
        val cell: Ptr<T> = Ptr { addr: self.data }
        val v: &T = cell.borrow(0u64)
        v
    }

    fn set(&mut self, value: T) {
        val cell: Ptr<T> = Ptr { addr: self.data }
        cell.set(0u64, value)
    }

    fn as_ptr(&self) -> ptr {
        self.data
    }
}

impl<T> Drop for Box<T> {
    fn drop(&mut self) {
        __builtin_heap_free(self.data)
    }
}
