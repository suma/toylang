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
#   - `v.pop() -> T` (`&mut self`) — remove last
#   - `v.get(i) -> T` — random read
#   - `v.set(i, value)` (`&mut self`) — random write
#   - `v.size() -> u64` — current element count
#   - `v.capacity() -> u64` — allocated slots
#   - `v.is_empty() -> bool`
#
# The positions a caller may name are stated as `requires` clauses
# rather than left to a comment: `--api core/std/collections/vec.t`
# prints them, and a violation reports the offending value
# (`with index = 5`) instead of a fixed string. The `panic` inside
# each body is kept on purpose and is *not* dead code — contracts are
# compiled out by `--release`, and a `Vec` whose bounds stop being
# checked there would be the one indexed read in the language that
# goes unguarded in a release build (a built-in `arr[i]` keeps its
# guard, GUARD_ELISION.md). Contract first, panic as the net.
#
# Method `size` is named for symmetry with `core/std/dict.t::size`
# rather than Rust's `len` to dodge any potential clash with the
# `len` field of `Vec<T>` itself when method dispatch needs to
# resolve `v.len(...)`.
#
# Per-monomorph generic substitution (DICT-AOT-NEW Phase C) makes
# `__builtin_sizeof(value)` and `__builtin_ptr_read::<T>(...)`
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

    # A vector with room for `n` elements already reserved, so a
    # loop that fills it allocates once instead of growing as it
    # goes. `size()` is still 0 — the room exists, the elements do
    # not.
    #
    # The stride comes from the type rather than from a first
    # `push`, which is what lets the buffer exist before any element
    # does. `n == 0` allocates nothing, exactly like `new()`.
    fn with_capacity(n: u64) -> Self {
        val stride: u64 = __builtin_sizeof::<T>()
        val room: Option<u64> = n.checked_mul(stride)
        val bytes: u64 = match room {
            Option::Some(b) => b,
            Option::None => panic("Vec::with_capacity: capacity overflows u64"),
        }
        val data: ptr = __builtin_heap_alloc(bytes)
        # `heap_alloc(0)` returns null *by contract* (`core/std/ptr.t`),
        # so only a non-zero request can have failed -- reading every
        # null as failure would make `Vec::new()` an out-of-memory
        # panic.
        if bytes > 0u64 && __builtin_ptr_is_null(data) {
            panic("Vec::with_capacity: allocation failed ({bytes} bytes)")
        }
        Vec {
            data: data,
            len: 0u64,
            cap: n,
            elem_size: stride,
        }
    }

    # `with_capacity` for a caller that has a budget to respect: the
    # same request, answered instead of enforced (ERROR_MODEL D5).
    # Separate from `try_reserve` because there is no value yet to
    # call a method on.
    fn try_with_capacity(n: u64) -> Result<Self, AllocError> {
        val stride: u64 = __builtin_sizeof::<T>()
        val room: Option<u64> = n.checked_mul(stride)
        val bytes: u64 = match room {
            Option::Some(b) => b,
            Option::None => { return Result::Err(AllocError::SizeOverflow) }
        }
        val data: ptr = __builtin_heap_alloc(bytes)
        if bytes > 0u64 && __builtin_ptr_is_null(data) {
            return Result::Err(AllocError::OutOfMemory)
        }
        val v: Vec<T> = Vec {
            data: data,
            len: 0u64,
            cap: n,
            elem_size: stride,
        }
        Result::Ok(v)
    }

    # Make room for `n` more elements than `size()`, or say why not.
    #
    # The whole point is that it fails **once**, before the loop, so
    # the pushes that follow are within capacity and cannot regrow --
    # which is what lets `push` keep a signature that does not return
    # a `Result`. The promise ends at that capacity: a push past it
    # reallocates again and can panic again.
    #
    # `elem_size` is 0 until the first `push` on a `Vec::new()`, since
    # that is where the stride is learned; reserving before then reads
    # it from the type instead.
    unsafe fn try_reserve(&mut self, n: u64) -> Result<(), AllocError> {
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof::<T>()
        }
        val used: u64 = self.len
        val want: Option<u64> = used.checked_add(n)
        val need: u64 = match want {
            Option::Some(c) => c,
            Option::None => { return Result::Err(AllocError::SizeOverflow) }
        }
        if need <= self.cap { return Result::Ok(()) }
        val room: Option<u64> = need.checked_mul(self.elem_size)
        val bytes: u64 = match room {
            Option::Some(b) => b,
            Option::None => { return Result::Err(AllocError::SizeOverflow) }
        }
        val grown: ptr = __builtin_heap_realloc(self.data, bytes)
        # `need > self.cap >= 0` and a stride of at least one byte make
        # `bytes` non-zero here, so a null really is a failure -- the
        # zero-request contract (`core/std/ptr.t`) cannot fire.
        if bytes > 0u64 && __builtin_ptr_is_null(grown) {
            return Result::Err(AllocError::OutOfMemory)
        }
        self.data = grown
        self.cap = need
        Result::Ok(())
    }

    # Move the buffer to `new_cap` elements, or stop the program.
    #
    # The check happens **before** `self.data` is assigned. `realloc`
    # leaves the original block alone when it fails, so a vector that
    # could not grow is still intact and still owns its bytes; writing
    # the null in first would lose the old pointer -- a leak, and every
    # later element written to address 0.
    unsafe fn grow_to(&mut self, new_cap: u64)
        requires new_cap >= self.len
    {
        val room: Option<u64> = new_cap.checked_mul(self.elem_size)
        val bytes: u64 = match room {
            Option::Some(b) => b,
            Option::None => panic("Vec::grow: capacity overflows u64"),
        }
        val grown: ptr = __builtin_heap_realloc(self.data, bytes)
        if bytes > 0u64 && __builtin_ptr_is_null(grown) {
            panic("Vec::grow: allocation failed ({bytes} bytes)")
        }
        self.data = grown
        self.cap = new_cap
    }

    # Declare that the first `n` elements are live.
    #
    # For buffers something *else* filled: a `recv` writing into
    # `v.as_span()`, or any code that writes through the raw pointer
    # without going through `push`. Without this the bytes are there
    # and `size()` still says 0.
    #
    # Capacity is the bound. Elements between the old and new
    # size are whatever the memory already held, so this is only
    # sound after they have actually been written — the checker
    # cannot see that, and (P6's rule being about pointee-touching
    # builtins, which this calls none of) it is not an `unsafe fn`
    # either.
    fn set_size(&mut self, n: u64)
        requires n <= self.cap
    {
        if n > self.cap { panic("Vec::set_size beyond capacity") }
        self.len = n
    }

    # A window over the elements (CONV-SPAN), for code that takes a
    # `Span<T>` — reading, writing and sub-slicing all reach this
    # vector's own memory, with no copy.
    #
    # `None` while the vector has never allocated (`Vec::new()` with
    # nothing pushed yet): there is no address to view. A vector that
    # allocated and was then cleared answers `Some` with a length of
    # 0. The window does not track the vector: a later `push` can
    # reallocate and leave it dangling (`Span`'s escape is unchecked,
    # POINTER P4).
    fn as_span(&self) -> Option<Span<T>> {
        Span::try_from_raw_parts(self.data, self.len)
    }

    # A window over the whole *allocation*, `capacity()` elements
    # from index 0 — not just the live ones.
    #
    # This is the receiving end: something writes into the reserved
    # room, then `set_size(n)` declares how much of it is real.
    # `as_span()` is the other half and views `size()` elements, so
    # it is empty until then.
    #
    #     var buf: Vec<u8> = Vec::with_capacity(4096u64)
    #     # ... fill buf.capacity_span() ...
    #     buf.set_size(n)
    #
    # Elements at or past `size()` hold whatever the memory already
    # did. Reading one before writing it is not a memory error — the
    # bounds check passes — but the value is meaningless.
    fn capacity_span(&self) -> Option<Span<T>> {
        Span::try_from_raw_parts(self.data, self.cap)
    }

    # Append. Geometric grow: 0 → 4 → 8 → 16 → ... so `n`
    # consecutive `push`es cost amortised O(1).
    unsafe fn push(&mut self, value: T) {
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof(value)
        }
        if self.cap == 0u64 {
            self.grow_to(4u64)
        } elif self.len >= self.cap {
            self.grow_to(self.cap * 2u64)
        }
        __builtin_ptr_write(self.data, self.len * self.elem_size, value)
        self.len = self.len + 1u64
    }

    # Remove and return the last element. An empty vec has no last
    # element to name (DEBUG-OBS D6) — this used to read whatever sat
    # at offset 0 and underflow `self.len` to `u64::MAX`, which turned
    # one mistake into a Vec that reports 18 quintillion elements.
    unsafe fn pop(&mut self) -> T
        requires self.len > 0u64
    {
        if self.len == 0u64 { panic("Vec::pop on an empty Vec") }
        self.len = self.len - 1u64
        val v: T = __builtin_ptr_read::<T>(self.data, self.len * self.elem_size)
        v
    }

    # Random-access read (DEBUG-OBS D6).
    #
    # A built-in array traps on an out-of-range index (RUNTIME-TRAP);
    # this used to be the one indexed read that did not, and reading
    # past the end reached the host — `value not defined` from inside
    # the IR VM, with no toylang position or backtrace left.
    unsafe fn get(&self, index: u64) -> T
        requires index < self.len
    {
        if index >= self.len { panic("Vec::get index out of bounds") }
        val v: T = __builtin_ptr_read::<T>(self.data, index * self.elem_size)
        v
    }

    # Random-access write. `push` writes through the raw pointer, so
    # appending is not affected by the bound stated here.
    unsafe fn set(&mut self, index: u64, value: T)
        requires index < self.len
    {
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

    # --- COLLECTIONS C3 ---------------------------------------------
    #
    # The four that need `==` on the element (`contains`, `index_of`)
    # carry no bound: `==` between two values of a type parameter is
    # allowed and dispatches to the element's own `eq`, and an element
    # type with no answer for it is reported at the call site
    # (`[E0010]`, COLLECTIONS C0(a)). There is no `Eq` trait to bound
    # against.

    # Insert `value` at `index`, shifting everything from there up one
    # place. `index == size()` appends, which makes `insert` total over
    # the positions a caller can name — hence `<=` where `get` has `<`.
    unsafe fn insert(&mut self, index: u64, value: T)
        requires index <= self.len
    {
        if index > self.len { panic("Vec::insert index out of bounds") }
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof(value)
        }
        if self.cap == 0u64 {
            self.grow_to(4u64)
        } elif self.len >= self.cap {
            self.grow_to(self.cap * 2u64)
        }
        var i: u64 = self.len
        while i > index {
            val prev: T = __builtin_ptr_read::<T>(self.data, (i - 1u64) * self.elem_size)
            __builtin_ptr_write(self.data, i * self.elem_size, prev)
            i = i - 1u64
        }
        __builtin_ptr_write(self.data, index * self.elem_size, value)
        self.len = self.len + 1u64
    }

    # Remove the element at `index` and return it, shifting the rest
    # down. Order-preserving and O(n); `swap_remove` is the O(1) one.
    unsafe fn remove(&mut self, index: u64) -> T
        requires index < self.len
    {
        if index >= self.len { panic("Vec::remove index out of bounds") }
        val out: T = __builtin_ptr_read::<T>(self.data, index * self.elem_size)
        var i: u64 = index
        while i + 1u64 < self.len {
            val next: T = __builtin_ptr_read::<T>(self.data, (i + 1u64) * self.elem_size)
            __builtin_ptr_write(self.data, i * self.elem_size, next)
            i = i + 1u64
        }
        self.len = self.len - 1u64
        out
    }

    # Remove the element at `index` and return it, moving the last
    # element into the hole. O(1), and it reorders — the name says so,
    # which `Dict::remove` used not to (it swapped silently and broke
    # iteration order).
    unsafe fn swap_remove(&mut self, index: u64) -> T
        requires index < self.len
    {
        if index >= self.len { panic("Vec::swap_remove index out of bounds") }
        val out: T = __builtin_ptr_read::<T>(self.data, index * self.elem_size)
        val last: T = __builtin_ptr_read::<T>(self.data, (self.len - 1u64) * self.elem_size)
        __builtin_ptr_write(self.data, index * self.elem_size, last)
        self.len = self.len - 1u64
        out
    }

    # Whether any element equals `value`. Linear.
    unsafe fn contains(&self, value: T) -> bool {
        var i: u64 = 0u64
        while i < self.len {
            val e: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            if e == value {
                return true
            }
            i = i + 1u64
        }
        false
    }

    # The position of the first element equal to `value`, or `None`.
    unsafe fn index_of(&self, value: T) -> Option<u64> {
        var i: u64 = 0u64
        while i < self.len {
            val e: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            if e == value {
                return Option::Some(i)
            }
            i = i + 1u64
        }
        Option::None
    }

    # Reverse in place.
    unsafe fn reverse(&mut self) {
        if self.len == 0u64 {
            return
        }
        var i: u64 = 0u64
        var j: u64 = self.len - 1u64
        while i < j {
            val a: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            val b: T = __builtin_ptr_read::<T>(self.data, j * self.elem_size)
            __builtin_ptr_write(self.data, i * self.elem_size, b)
            __builtin_ptr_write(self.data, j * self.elem_size, a)
            i = i + 1u64
            j = j - 1u64
        }
    }

    # Sort in place with a caller-supplied `less`, which must answer
    # "does a come strictly before b". Same stable insertion sort as
    # `sort` below, and the way to sort an element type that has no
    # `Ord` impl — or to sort one of those in another order
    # (`fn (a: u64, b: u64) -> bool { b < a }` for descending).
    #
    # An AOT closure cannot take a compound parameter, so this is
    # scalar element types on the compiled lanes; `sort` (the `Ord`
    # one) is what sorts a `Vec<String>` there.
    unsafe fn sort_by(&mut self, less: fn (T, T) -> bool) {
        var i: u64 = 1u64
        while i < self.len {
            # Raw reads and writes rather than `get` / `set`, for the
            # reason spelled out on `sort` below: a local bound from a
            # compound-returning method call carries drop glue, and an
            # element copy is an alias rather than an owned value.
            val key: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            var j: u64 = i
            while j > 0u64 {
                val prev: T = __builtin_ptr_read::<T>(self.data, (j - 1u64) * self.elem_size)
                if !less(key, prev) {
                    break
                }
                __builtin_ptr_write(self.data, j * self.elem_size, prev)
                j = j - 1u64
            }
            __builtin_ptr_write(self.data, j * self.elem_size, key)
            i = i + 1u64
        }
    }
}

# STDLIB-ORD: stable insertion sort over the `Ord` trait
# (`core/std/cmp.t`). O(n^2) worst case — fine for the sizes toy
# programs sort, and much simpler than a generic partition (which
# would need swap-by-value and a 3-way comparison). The `<` on the
# element type resolves to the `lt` the `Ord` impl provides, so
# `Vec<u64>` / `Vec<i64>` / `Vec<f64>` / `Vec<bool>` and any
# `impl Ord` struct (including `String`, byte-wise) sort.
# `resize` needs a value for the slots it adds, and the caller has
# none to give -- only the type does (STDLIB-TRAIT-BASE §7). Its own
# block so `Vec<T>` does not require `T: Default` everywhere, the same
# shape `sort` uses for `Ord`.
impl<T: Clone> Clone for Vec<T> {
    unsafe fn clone(&self) -> Self {
        var out: Vec<T> = Vec::new()
        var i: u64 = 0u64
        while i < self.len {
            val v: T = self.get(i)
            val c: T = v.clone()
            out.push(c)
            i = i + 1u64
        }
        out
    }
}

impl<T: Default> Vec<T> {
    # Make `size()` exactly `n`: drop the tail, or fill with `T`'s
    # default. Shrinking keeps the capacity, like `clear`.
    unsafe fn resize(&mut self, n: u64) {
        if n <= self.len {
            self.len = n
            return
        }
        val fill: T = T::default()
        while self.len < n {
            self.push(fill)
        }
    }
}

impl<T: Ord> Vec<T> {
    # Sort in place, ascending. Stable: equal elements keep their
    # relative order. Elements are read as copies out of the buffer,
    # and `lt` borrows both sides, so `key` stays usable across the
    # inner loop.
    #
    # **The buffer is addressed directly rather than through `get` /
    # `set`, and that is load-bearing for an element type that owns
    # something** (STRING-NO-DROP). A local bound from a
    # compound-returning *method call* carries drop glue, so
    # `val key: T = self.get(i)` on a `Vec<String>` frees the very
    # buffer the element still holds -- the copy aliases it, and the
    # glue cannot tell an alias from an owned value. A local bound
    # from `__builtin_ptr_read` does not, which is why `contains` /
    # `index_of` were already written this way.
    #
    # The value cannot go straight into the argument either: a
    # compound-returning method call directly in an expression
    # position (a `set` argument, a `lt` argument) does not AOT-lower
    # for compound `T`. Hence a `val` and a raw write.
    unsafe fn sort(&mut self) {
        var i: u64 = 1u64
        while i < self.len {
            val key: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            var j: u64 = i
            while j > 0u64 {
                val prev: T = __builtin_ptr_read::<T>(self.data, (j - 1u64) * self.elem_size)
                if !key.lt(prev) {
                    break
                }
                __builtin_ptr_write(self.data, j * self.elem_size, prev)
                j = j - 1u64
            }
            __builtin_ptr_write(self.data, j * self.elem_size, key)
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

impl<T> Iterator<T> for VecIter<T> {
    # Advance by one element. Returns `None` once `index` has walked
    # past `len`. The element is read as a copy out of the buffer —
    # exactly like `Vec::get`, so compound `T` (including `Box`) is
    # an alias of the stored value.
    unsafe fn next(&mut self) -> Option<T> {
        if self.index >= self.len {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val e: T = __builtin_ptr_read::<T>(self.data, i * self.elem_size)
            Option::Some(e)
        }
    }
}


# Concrete-args impl: byte-vector helpers live here because the
# inner `__builtin_ptr_read::<u8>(...)` produces `u8` and `push(value)`
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
    unsafe fn from_str(s: str) -> Self {
        val n: u64 = s.len()
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        if n > 0u64 && __builtin_ptr_is_null(data) {
            panic("Vec::from_str: allocation failed ({n} bytes)")
        }
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
    unsafe fn extend_bytes(&mut self, src: ptr, count: u64) {
        var i: u64 = 0u64
        while i < count {
            val b: u8 = __builtin_ptr_read::<u8>(src, i)
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
    # >= U+110000 are not valid Unicode scalars, and the signature
    # says so. Lexer already rejects them in `'\u{...}'` literals via
    # `char::from_u32`; the contract guards values built from
    # arithmetic. Unlike the indexed methods this one carries no
    # matching `panic` — a violation writes malformed UTF-8, which is
    # a wrong answer and not a memory error, so there is nothing for a
    # `--release` build to be protected from.
    #
    # Implementation notes: shifts intentionally use `u64` operands —
    # the type checker forces shift right-hand sides to `u64`
    # (`frontend/src/type_checker/utility.rs:128`), so we widen the
    # codepoint once and narrow each output byte with `as u8`.
    fn push_char(&mut self, c: char)
        requires c < 0x110000u32
        requires !(c >= 0xD800u32 && c <= 0xDFFFu32)
    {
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
    unsafe fn eq(&self, other: &Vec<u8>) -> bool {
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
            val a: u8 = __builtin_ptr_read::<u8>(pa, i)
            val b: u8 = __builtin_ptr_read::<u8>(pb, i)
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

impl<T, U> Iterator<U> for MapIter<T, U> {
    # Apply `f` to each element on the way out.
    unsafe fn next(&mut self) -> Option<U> {
        match self.source.next() {
            Option::Some(v) => Option::Some(self.f(v)),
            Option::None => Option::None,
        }
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

impl<T> Iterator<T> for FilterIter<T> {
    # Yield only the elements for which `pred` returns true.
    unsafe fn next(&mut self) -> Option<T> {
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

impl<T> Iterator<(u64, T)> for EnumerateIter<T> {
    # Yield `(index, element)` pairs, starting at 0.
    unsafe fn next(&mut self) -> Option<(u64, T)> {
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

impl<A, B> Iterator<(A, B)> for ZipIter<A, B> {
    # Yield `(a, b)` pairs, stopping at the shorter of the two
    # sources. `elems` packs the two element strides into one field
    # (`a_elem << 32 | b_elem`) so the struct fits in the backend's
    # receiver-writeback register budget (the same trick `DictIter`
    # uses for its key/value sizes).
    unsafe fn next(&mut self) -> Option<(A, B)> {
        if self.index >= self.min_len {
            Option::None
        } else {
            val a_elem = self.elems >> 32u64
            val b_elem = self.elems & 0xFFFFFFFFu64
            val ai: A = __builtin_ptr_read::<A>(self.a_data, self.index * a_elem)
            val bi: B = __builtin_ptr_read::<B>(self.b_data, self.index * b_elem)
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
