# Stdlib `trait Alloc` + wrapper structs.
#
# `trait Alloc` provides the user-facing malloc / realloc /
# free interface. Implementations delegate to an underlying
# runtime allocator handle via `with allocator = self._h { ... }`,
# routing each call through the active-allocator stack so
# `__builtin_heap_alloc` dispatches to the right backend.
#
# In addition to the trait surface, `Arena` and `FixedBuffer`
# carry a toylang-side `(addr, size)` tracking table so they
# can answer Odin/Zig-style introspection queries
# (`bytes_used` / `used` / `remaining` / `is_empty`) and a
# `reset()` method that releases every tracked allocation
# without throwing the wrapper away. The runtime arena /
# fixed_buffer registries also do their own tracking — this
# is intentional duplication so that the inline-temporary form
# `with allocator = Arena::new() { __builtin_heap_alloc(...) }`
# (which bypasses the wrapper and routes raw heap_alloc through
# the runtime handle) keeps working with arena semantics. Users
# who want the new methods should bind the wrapper:
#
#     val arena = Arena::new()
#     val p = arena.alloc(64u64)
#     arena.bytes_used()         # 64
#     arena.reset()
#     # arena.drop() fires at scope exit via the Drop trait
#
# Name split: `Allocator` (primitive runtime handle) vs `Alloc`
# (this trait). The two never collide.

pub trait Alloc {
    fn alloc(&mut self, size: u64) -> ptr
    fn free(&mut self, p: ptr)
    fn realloc(&mut self, p: ptr, new_size: u64) -> ptr
}

# ---- Default global allocator ----
pub struct Global {
    h: Allocator,
}

impl Global {
    fn new() -> Self {
        Global { h: __builtin_default_allocator() }
    }
}

impl Alloc for Global {
    fn alloc(&mut self, size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_alloc(size)
        }
    }

    fn free(&mut self, p: ptr) {
        with allocator = self.h {
            __builtin_heap_free(p)
        }
    }

    fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_realloc(p, new_size)
        }
    }
}

# ---- Arena allocator (bulk-free on `drop` / `reset`) ----
#
# Layout note: every field after `_h` is part of the toylang-
# side tracking table. `_h` is the field consumed by the
# language's `with allocator = arena_value { ... }` auto-
# extract — it is the only `Allocator`-typed field, which the
# type checker enforces.
pub struct Arena {
    _h: Allocator,
    addrs: ptr,        # parallel array of tracked ptrs (heap-allocated, default)
    sizes: ptr,        # parallel array of tracked sizes (u64, parallel to addrs)
    count: u64,        # number of live tracked entries
    cap_slots: u64,    # capacity of addrs/sizes in slots (each slot is 8 bytes)
    bytes_used: u64,   # cumulative tracked bytes (sum of sizes[0..count])
}

impl Arena {
    fn new() -> Self {
        Arena {
            _h: __builtin_arena_allocator(),
            addrs: __builtin_null_ptr(),
            sizes: __builtin_null_ptr(),
            count: 0u64,
            cap_slots: 0u64,
            bytes_used: 0u64,
        }
    }

    fn bytes_used(&self) -> u64 { self.bytes_used }

    # Bulk-free every tracked allocation. The arena stays valid
    # for further use after `reset()` — call sites can keep
    # alloc'ing through it.
    fn reset(&mut self) {
        # Release runtime-arena tracking (no-op for the runtime
        # arena's individual frees, but bulk-free here).
        __builtin_arena_drop(self._h)
        # Clear toylang-side tracking. Underlying memory is gone
        # courtesy of the line above; we just zero our bookkeeping.
        self.count = 0u64
        self.bytes_used = 0u64
    }

    # Internal: ensure `addrs` and `sizes` have room for one more
    # entry. Doubles capacity (8 / 16 / 32 / ...) on overflow.
    fn _ensure_slot(&mut self) {
        if self.count >= self.cap_slots {
            val new_cap = if self.cap_slots == 0u64 { 8u64 } else { self.cap_slots * 2u64 }
            with allocator = __builtin_default_allocator() {
                self.addrs = __builtin_heap_realloc(self.addrs, new_cap * 8u64)
                self.sizes = __builtin_heap_realloc(self.sizes, new_cap * 8u64)
            }
            self.cap_slots = new_cap
        }
    }

    # Internal: linear search for `p` in `addrs`. Returns the
    # index, or `count` (one past end) if not found.
    fn _find(&self, p: ptr) -> u64 {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read(self.addrs, i * 8u64)
            if __builtin_ptr_eq(a, p) {
                return i
            }
            i = i + 1u64
        }
        self.count
    }
}

impl Drop for Arena {
    # Auto-cleanup runs in two contexts:
    #   1. Named binding `val a = Arena::new()` going out of
    #      scope — the regular Drop machinery fires `a.drop()`.
    #   2. Inline temporary `with allocator = Arena::new() { ... }`
    #      — the interpreter / AOT auto-cleanup hook calls the
    #      runtime arena's `reset()` directly, bypassing this
    #      method (the runtime registry is the source of truth
    #      for raw `__builtin_heap_alloc` calls in that scope).
    #      Toylang metadata in the wrapper struct is leaked in
    #      that case, but the wrapper itself is unreachable
    #      after the with-scope so the leak ends with the
    #      process. Future work can route the inline-temporary
    #      cleanup through this method instead.
    fn drop(&mut self) {
        # Bulk-free runtime tracking + zero our counters.
        self.reset()
        # Release the metadata arrays themselves (allocated via
        # default at construction / growth time).
        if self.cap_slots != 0u64 {
            with allocator = __builtin_default_allocator() {
                __builtin_heap_free(self.addrs)
                __builtin_heap_free(self.sizes)
            }
            self.cap_slots = 0u64
            self.addrs = __builtin_null_ptr()
            self.sizes = __builtin_null_ptr()
        }
    }
}

impl Alloc for Arena {
    fn alloc(&mut self, size: u64) -> ptr {
        # Size 0 short-circuits to a null pointer; binding a null
        # pointer to a `val` and re-reading it is rejected by the
        # interpreter's identifier lookup, so handle the
        # zero-size case before introducing the local binding.
        if size == 0u64 {
            return __builtin_null_ptr()
        }
        val p = with allocator = self._h {
            __builtin_heap_alloc(size)
        }
        # Runtime arena allocator never returns null for non-zero
        # sizes (it draws from the shared HeapManager), so we can
        # record the entry unconditionally.
        self._ensure_slot()
        __builtin_ptr_write(self.addrs, self.count * 8u64, p)
        __builtin_ptr_write(self.sizes, self.count * 8u64, size)
        self.count = self.count + 1u64
        self.bytes_used = self.bytes_used + size
        p
    }

    fn free(&mut self, p: ptr) {
        # Arena policy: per-pointer free is a no-op. Forwarded
        # to `_h` for symmetry; the runtime arena ignores it too.
        with allocator = self._h {
            __builtin_heap_free(p)
        }
    }

    fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        # Mirror Arena::alloc: avoid binding null results to a
        # `val` by handling the "result is guaranteed null" cases
        # up front (new_size == 0 → free + null).
        if new_size == 0u64 {
            with allocator = self._h {
                __builtin_heap_free(p)
            }
            return __builtin_null_ptr()
        }
        val idx = self._find(p)
        val q = with allocator = self._h {
            __builtin_heap_realloc(p, new_size)
        }
        # Runtime arena's realloc returns non-null for non-zero
        # new_size (allocates a fresh slot if needed).
        if idx < self.count {
            val old: u64 = __builtin_ptr_read(self.sizes, idx * 8u64)
            self.bytes_used = self.bytes_used - old + new_size
            __builtin_ptr_write(self.addrs, idx * 8u64, q)
            __builtin_ptr_write(self.sizes, idx * 8u64, new_size)
        } else {
            # Untracked input (e.g. realloc(null, n)) — register fresh.
            self._ensure_slot()
            __builtin_ptr_write(self.addrs, self.count * 8u64, q)
            __builtin_ptr_write(self.sizes, self.count * 8u64, new_size)
            self.count = self.count + 1u64
            self.bytes_used = self.bytes_used + new_size
        }
        q
    }
}

# ---- Fixed-buffer allocator (capacity-limited) ----
pub struct FixedBuffer {
    _h: Allocator,
    cap: u64,
    addrs: ptr,
    sizes: ptr,
    count: u64,
    cap_slots: u64,
    used_bytes: u64,
}

impl FixedBuffer {
    fn new(capacity: u64) -> Self {
        FixedBuffer {
            _h: __builtin_fixed_buffer_allocator(capacity),
            cap: capacity,
            addrs: __builtin_null_ptr(),
            sizes: __builtin_null_ptr(),
            count: 0u64,
            cap_slots: 0u64,
            used_bytes: 0u64,
        }
    }

    fn capacity(&self) -> u64 { self.cap }
    fn used(&self) -> u64 { self.used_bytes }
    fn remaining(&self) -> u64 {
        if self.used_bytes >= self.cap {
            0u64
        } else {
            self.cap - self.used_bytes
        }
    }
    fn is_empty(&self) -> bool { self.used_bytes == 0u64 }

    fn reset(&mut self) {
        __builtin_fixed_buffer_drop(self._h)
        self.count = 0u64
        self.used_bytes = 0u64
    }

    fn _ensure_slot(&mut self) {
        if self.count >= self.cap_slots {
            val new_cap = if self.cap_slots == 0u64 { 8u64 } else { self.cap_slots * 2u64 }
            with allocator = __builtin_default_allocator() {
                self.addrs = __builtin_heap_realloc(self.addrs, new_cap * 8u64)
                self.sizes = __builtin_heap_realloc(self.sizes, new_cap * 8u64)
            }
            self.cap_slots = new_cap
        }
    }

    fn _find(&self, p: ptr) -> u64 {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read(self.addrs, i * 8u64)
            if __builtin_ptr_eq(a, p) {
                return i
            }
            i = i + 1u64
        }
        self.count
    }

    # Internal: remove entry at `idx` by swapping with the last
    # entry and decrementing count.
    fn _swap_remove(&mut self, idx: u64) {
        val last = self.count - 1u64
        if idx != last {
            val last_addr: ptr = __builtin_ptr_read(self.addrs, last * 8u64)
            val last_size: u64 = __builtin_ptr_read(self.sizes, last * 8u64)
            __builtin_ptr_write(self.addrs, idx * 8u64, last_addr)
            __builtin_ptr_write(self.sizes, idx * 8u64, last_size)
        }
        self.count = last
    }
}

impl Drop for FixedBuffer {
    fn drop(&mut self) {
        self.reset()
        if self.cap_slots != 0u64 {
            with allocator = __builtin_default_allocator() {
                __builtin_heap_free(self.addrs)
                __builtin_heap_free(self.sizes)
            }
            self.cap_slots = 0u64
            self.addrs = __builtin_null_ptr()
            self.sizes = __builtin_null_ptr()
        }
    }
}

impl Alloc for FixedBuffer {
    fn alloc(&mut self, size: u64) -> ptr {
        # Quota check + zero-size both produce a null pointer.
        # Return early so we never bind a null pointer to a
        # local `val` (the interpreter's identifier lookup
        # treats `Object::Pointer(0)` as undefined).
        if size == 0u64 {
            return __builtin_null_ptr()
        }
        if self.used_bytes + size > self.cap {
            return __builtin_null_ptr()
        }
        val p = with allocator = self._h {
            __builtin_heap_alloc(size)
        }
        # Quota already cleared above + size > 0 → p is non-null.
        self._ensure_slot()
        __builtin_ptr_write(self.addrs, self.count * 8u64, p)
        __builtin_ptr_write(self.sizes, self.count * 8u64, size)
        self.count = self.count + 1u64
        self.used_bytes = self.used_bytes + size
        p
    }

    fn free(&mut self, p: ptr) {
        val idx = self._find(p)
        if idx < self.count {
            val sz: u64 = __builtin_ptr_read(self.sizes, idx * 8u64)
            with allocator = self._h {
                __builtin_heap_free(p)
            }
            self.used_bytes = self.used_bytes - sz
            self._swap_remove(idx)
        }
    }

    fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        if new_size == 0u64 {
            self.free(p)
            return __builtin_null_ptr()
        }
        val idx = self._find(p)
        # AOT MVP requires `val NAME: TYPE = __builtin_ptr_read(...)` to
        # be a top-level let-binding, not nested inside `if`. Read the
        # current size up front (when known) into a separate `var`.
        var old: u64 = 0u64
        if idx < self.count {
            val sz: u64 = __builtin_ptr_read(self.sizes, idx * 8u64)
            old = sz
        }
        val projected = self.used_bytes - old + new_size
        if projected > self.cap {
            return __builtin_null_ptr()
        }
        val q = with allocator = self._h {
            __builtin_heap_realloc(p, new_size)
        }
        if idx < self.count {
            self.used_bytes = self.used_bytes - old + new_size
            __builtin_ptr_write(self.addrs, idx * 8u64, q)
            __builtin_ptr_write(self.sizes, idx * 8u64, new_size)
        } else {
            self._ensure_slot()
            __builtin_ptr_write(self.addrs, self.count * 8u64, q)
            __builtin_ptr_write(self.sizes, self.count * 8u64, new_size)
            self.count = self.count + 1u64
            self.used_bytes = self.used_bytes + new_size
        }
        q
    }
}
