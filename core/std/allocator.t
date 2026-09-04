# Stdlib `trait Alloc` + wrapper structs.
#
# `trait Alloc` provides the user-facing malloc / realloc /
# free interface. `Arena` and `FixedBuffer` carry a toylang-
# side `(addr, size)` tracking table so they can answer
# Odin/Zig-style introspection queries (`bytes_used` / `used`
# / `remaining` / `is_empty`) and bulk-free their tracked
# entries on `reset()` / `drop()` without relying on a
# specialized runtime allocator. Both wrap the global default
# allocator (`__builtin_default_allocator()`) and re-implement
# the policy in toylang:
#
#   - Arena: individual `free` is a no-op; allocations live
#     until `reset()` or `drop()` walks the tracking table.
#   - FixedBuffer: enforces a byte-count quota in toylang;
#     `free` releases per-pointer; `reset()` returns the
#     quota to zero.
#
# Usage (named binding triggers Drop at block exit):
#
#     val arena = Arena::new()
#     val p = arena.alloc(64u64)
#     arena.bytes_used()         # 64
#     arena.reset()
#     # arena.drop() fires at scope exit via the Drop trait
#
# Name split: `Allocator` (opaque runtime handle, used for
# the `_h` field consumed by the language's `with` auto-
# extract) vs `Alloc` (this trait). The two never collide.

# ---- Layout reporting (MEMORY_PROFILING M3) ----
#
# Fragmentation is a property of an allocator's layout, so the
# allocator reports it. The runtime profiler cannot compute it: it
# sees a stream of sizes, not a region.
#
# `known == false` means "this allocator does not manage a region, so
# there is nothing to report" — which is emphatically not the same as
# "zero fragmentation". `Global`, `Arena` and `FixedBuffer` all answer
# that way, because none of them owns a region: every one of them
# forwards individual allocations to the default allocator and keeps
# bookkeeping on the side. `FixedBuffer`'s `cap` is a quota, not a
# buffer.
pub struct AllocLayout {
    known: bool,
    managed_bytes: u64,   # total bytes under management
    live_bytes: u64,      # of which in use
    free_blocks: u64,     # number of separate free runs
    largest_free: u64,    # biggest single free run
}

impl AllocLayout {
    fn opaque() -> Self {
        AllocLayout {
            known: false,
            managed_bytes: 0u64,
            live_bytes: 0u64,
            free_blocks: 0u64,
            largest_free: 0u64,
        }
    }

    fn is_known(&self) -> bool { self.known }
    fn managed(&self) -> u64 { self.managed_bytes }
    fn live(&self) -> u64 { self.live_bytes }
    fn blocks(&self) -> u64 { self.free_blocks }
    fn largest(&self) -> u64 { self.largest_free }

    # Free bytes that are not in the largest run, as a permille of all
    # free bytes. Permille rather than a float so the number is exact
    # and compares equal across backends.
    #
    # Zero when nothing is free: an allocator with no free space is
    # not fragmented, it is full.
    fn external_fragmentation_permille(&self) -> u64 {
        if self.known == false { return 0u64 }
        if self.managed_bytes <= self.live_bytes { return 0u64 }
        val free_total: u64 = self.managed_bytes - self.live_bytes
        if free_total == 0u64 { return 0u64 }
        val scattered: u64 = free_total - self.largest_free
        scattered * 1000u64 / free_total
    }
}

pub trait Alloc {
    unsafe fn alloc(&mut self, size: u64) -> ptr
    unsafe fn free(&mut self, p: ptr)
    unsafe fn realloc(&mut self, p: ptr, new_size: u64) -> ptr

    # Default: report nothing. An allocator that does not manage a
    # region has no layout to describe, and saying so is the correct
    # answer — inventing zeros would put a fabricated fragmentation
    # figure in front of the reader.
    unsafe fn layout_report(&self) -> AllocLayout {
        # Written out rather than calling `AllocLayout::opaque()`: an
        # associated-function call in a trait default body is not
        # lowerable by the AOT MVP yet.
        AllocLayout {
            known: false,
            managed_bytes: 0u64,
            live_bytes: 0u64,
            free_blocks: 0u64,
            largest_free: 0u64,
        }
    }
}

# ---- Default global allocator ----
pub struct Global {
    h: Allocator,
}

impl Global {
    unsafe fn new() -> Self {
        Global { h: __builtin_default_allocator() }
    }
}

impl Alloc for Global {
    unsafe fn alloc(&mut self, size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_alloc(size)
        }
    }

    unsafe fn free(&mut self, p: ptr) {
        with allocator = self.h {
            __builtin_heap_free(p)
        }
    }

    unsafe fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
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
    # Live tracked bytes: the sum of sizes[0..count]. Because this
    # arena's `free` is a no-op, nothing leaves the set until `reset()`
    # or `drop()`, so this also happens to equal the cumulative total —
    # the two diverge the moment a per-pointer `free` is implemented.
    # MEMORY_PROFILING M0 fixed the terminology; the runtime's
    # `MemoryStats` keeps `live_bytes` and `cumulative_bytes` apart.
    bytes_used: u64,
}

impl Arena {
    unsafe fn new() -> Self {
        Arena {
            _h: __builtin_default_allocator(),
            addrs: __builtin_null_ptr(),
            sizes: __builtin_null_ptr(),
            count: 0u64,
            cap_slots: 0u64,
            bytes_used: 0u64,
        }
    }

    # Live tracked bytes. See the field comment: not a cumulative
    # total, despite coinciding with one while `free` is a no-op.
    fn bytes_used(&self) -> u64 { self.bytes_used }

    # Bulk-free every tracked allocation. The arena stays valid
    # for further use after `reset()` — call sites can keep
    # alloc'ing through it.
    unsafe fn reset(&mut self) {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read::<ptr>(self.addrs, i * 8u64)
            with allocator = __builtin_default_allocator() {
                __builtin_heap_free(a)
            }
            i = i + 1u64
        }
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
    unsafe fn _find(&self, p: ptr) -> u64 {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read::<ptr>(self.addrs, i * 8u64)
            if __builtin_ptr_eq(a, p) {
                return i
            }
            i = i + 1u64
        }
        self.count
    }
}

impl Drop for Arena {
    # Fires at scope exit for named bindings (`val a = Arena::new()`
    # then a goes out of scope) and for inline temporaries
    # (`with allocator = Arena::new() { ... }` once the auto-drop
    # hook routes through the user Drop method).
    unsafe fn drop(&mut self) {
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
    unsafe fn alloc(&mut self, size: u64) -> ptr {
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
        # Default allocator returns non-null for non-zero sizes; record
        # the entry unconditionally.
        self._ensure_slot()
        __builtin_ptr_write(self.addrs, self.count * 8u64, p)
        __builtin_ptr_write(self.sizes, self.count * 8u64, size)
        self.count = self.count + 1u64
        self.bytes_used = self.bytes_used + size
        p
    }

    unsafe fn free(&mut self, p: ptr) {
        # Arena policy: per-pointer free is a no-op; everything is
        # released in bulk via `reset()` or `drop()`.
    }

    unsafe fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        if new_size == 0u64 {
            # Arena policy keeps the existing allocation tracked until reset.
            return __builtin_null_ptr()
        }
        val idx = self._find(p)
        val q = with allocator = self._h {
            __builtin_heap_realloc(p, new_size)
        }
        # ERROR_MODEL D5: a failed `realloc` leaves the original block
        # alone, so the arena still owns `p` at its old size. Updating
        # the table anyway forgot that address while adding the bytes
        # it never got -- a leak plus a `bytes_used` that grew on a
        # request that was refused.
        if __builtin_ptr_is_null(q) {
            return __builtin_null_ptr()
        }
        if idx < self.count {
            val old: u64 = __builtin_ptr_read::<u64>(self.sizes, idx * 8u64)
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
    # Live bytes, in the MEMORY_PROFILING M0 sense: `free` decrements
    # it, so it is the quota actually in use rather than a running
    # total of everything ever handed out.
    used_bytes: u64,
}

impl FixedBuffer {
    unsafe fn new(capacity: u64) -> Self {
        FixedBuffer {
            _h: __builtin_default_allocator(),
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

    unsafe fn reset(&mut self) {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read::<ptr>(self.addrs, i * 8u64)
            with allocator = __builtin_default_allocator() {
                __builtin_heap_free(a)
            }
            i = i + 1u64
        }
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

    unsafe fn _find(&self, p: ptr) -> u64 {
        var i = 0u64
        while i < self.count {
            val a: ptr = __builtin_ptr_read::<ptr>(self.addrs, i * 8u64)
            if __builtin_ptr_eq(a, p) {
                return i
            }
            i = i + 1u64
        }
        self.count
    }

    # Internal: remove entry at `idx` by swapping with the last
    # entry and decrementing count.
    unsafe fn _swap_remove(&mut self, idx: u64) {
        val last = self.count - 1u64
        if idx != last {
            val last_addr: ptr = __builtin_ptr_read::<ptr>(self.addrs, last * 8u64)
            val last_size: u64 = __builtin_ptr_read::<u64>(self.sizes, last * 8u64)
            __builtin_ptr_write(self.addrs, idx * 8u64, last_addr)
            __builtin_ptr_write(self.sizes, idx * 8u64, last_size)
        }
        self.count = last
    }
}

impl Drop for FixedBuffer {
    unsafe fn drop(&mut self) {
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
    unsafe fn alloc(&mut self, size: u64) -> ptr {
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

    unsafe fn free(&mut self, p: ptr) {
        val idx = self._find(p)
        if idx < self.count {
            val sz: u64 = __builtin_ptr_read::<u64>(self.sizes, idx * 8u64)
            with allocator = self._h {
                __builtin_heap_free(p)
            }
            self.used_bytes = self.used_bytes - sz
            self._swap_remove(idx)
        }
    }

    unsafe fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        if new_size == 0u64 {
            self.free(p)
            return __builtin_null_ptr()
        }
        val idx = self._find(p)
        # A compound read still has to be a top-level let-binding in
        # the AOT lane, not nested inside `if`. Read the current size
        # up front (when known) into a separate `var`.
        var old: u64 = 0u64
        if idx < self.count {
            val sz: u64 = __builtin_ptr_read::<u64>(self.sizes, idx * 8u64)
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

# ---- SlotRegion: an allocator with a layout to report ----
#
# MEMORY_PROFILING M3. `Global` / `Arena` / `FixedBuffer` forward every
# allocation to the default allocator and keep bookkeeping on the
# side, so none of them owns a layout and all three report nothing.
# This one manages a fixed set of equally-sized slots and satisfies a
# request only from a run of *consecutive* free slots — so freeing in
# the middle really does scatter the free space, and `layout_report`
# has something true to describe.
#
# Slots rather than offsets into one block because toylang has no
# pointer-arithmetic builtin: there is no way to hand out an interior
# pointer of a single allocation. Each slot is therefore its own
# allocation, made up front, and the region manages the *index* space.
# Fragmentation is over that space, which is a real constraint — a
# 3-slot request fails when the free slots are scattered singly, even
# with plenty of bytes free.
#
# It exists as much to prove the mechanism as to be used: a
# user-written allocator implements the same trait and lands in the
# same report with no further wiring.
pub struct SlotRegion {
    _h: Allocator,
    slot_bytes: u64,
    slot_count: u64,
    ptrs: ptr,     # slot index -> pointer
    used: ptr,     # slot index -> run length when a run starts here, else 0
    live_slots: u64,
}

impl SlotRegion {
    unsafe fn new(slot_bytes: u64, slot_count: u64) -> Self {
        # Build the backing arrays in plain pointer locals (not a
        # `SlotRegion` value) so returning the struct literal below does
        # not fire a transient `Drop` that would register a spurious,
        # empty layout in the report.
        var ptrs = __builtin_null_ptr()
        var used = __builtin_null_ptr()
        with allocator = __builtin_default_allocator() {
            ptrs = __builtin_heap_alloc(slot_count * 8u64)
            used = __builtin_heap_alloc(slot_count * 8u64)
        }
        var i: u64 = 0u64
        while i < slot_count {
            var p = __builtin_null_ptr()
            with allocator = __builtin_default_allocator() {
                p = __builtin_heap_alloc(slot_bytes)
            }
            __builtin_ptr_write(ptrs, i * 8u64, p)
            __builtin_ptr_write(used, i * 8u64, 0u64)
            i = i + 1u64
        }
        SlotRegion {
            _h: __builtin_default_allocator(),
            slot_bytes: slot_bytes,
            slot_count: slot_count,
            ptrs: ptrs,
            used: used,
            live_slots: 0u64,
        }
    }

    fn capacity(&self) -> u64 { self.slot_count * self.slot_bytes }
    fn live(&self) -> u64 { self.live_slots * self.slot_bytes }

    # How many slots a request of `size` bytes needs.
    fn _slots_for(&self, size: u64) -> u64 {
        (size + self.slot_bytes - 1u64) / self.slot_bytes
    }

    # True when `n` slots starting at `at` are all free.
    unsafe fn _run_free(&self, at: u64, n: u64) -> bool {
        if at + n > self.slot_count { return false }
        var i: u64 = at
        var ok: bool = true
        while i < at + n {
            val u: u64 = __builtin_ptr_read::<u64>(self.used, i * 8u64)
            if u != 0u64 { ok = false }
            i = i + 1u64
        }
        ok
    }
}

impl Alloc for SlotRegion {
    unsafe fn alloc(&mut self, size: u64) -> ptr {
        if size == 0u64 { return __builtin_null_ptr() }
        val need: u64 = self._slots_for(size)
        if need > self.slot_count { return __builtin_null_ptr() }
        var at: u64 = self.slot_count
        var i: u64 = 0u64
        while i + need <= self.slot_count {
            if self._run_free(i, need) {
                at = i
                i = self.slot_count
            } else {
                i = i + 1u64
            }
        }
        if at >= self.slot_count { return __builtin_null_ptr() }
        # The run's first slot records its length; the rest are marked
        # occupied so a later scan cannot start inside a live run.
        __builtin_ptr_write(self.used, at * 8u64, need)
        var k: u64 = at + 1u64
        while k < at + need {
            __builtin_ptr_write(self.used, k * 8u64, 1u64)
            k = k + 1u64
        }
        self.live_slots = self.live_slots + need
        # Annotated binding: the AOT lowering takes the read width from
        # the annotation, so a bare expression-position read is not
        # supported there.
        val slot_ptr: ptr = __builtin_ptr_read::<ptr>(self.ptrs, at * 8u64)
        slot_ptr
    }

    unsafe fn free(&mut self, p: ptr) {
        if __builtin_ptr_is_null(p) { return }
        var i: u64 = 0u64
        var at: u64 = self.slot_count
        while i < self.slot_count {
            val q: ptr = __builtin_ptr_read::<ptr>(self.ptrs, i * 8u64)
            if __builtin_ptr_eq(q, p) {
                at = i
                i = self.slot_count
            } else {
                i = i + 1u64
            }
        }
        if at >= self.slot_count { return }
        val n: u64 = __builtin_ptr_read::<u64>(self.used, at * 8u64)
        if n == 0u64 { return }
        var k: u64 = at
        while k < at + n {
            __builtin_ptr_write(self.used, k * 8u64, 0u64)
            k = k + 1u64
        }
        self.live_slots = self.live_slots - n
    }

    unsafe fn realloc(&mut self, p: ptr, new_size: u64) -> ptr {
        if new_size == 0u64 {
            self.free(p)
            return __builtin_null_ptr()
        }
        val np = self.alloc(new_size)
        if __builtin_ptr_is_null(np) { return __builtin_null_ptr() }
        if __builtin_ptr_is_null(p) == false { self.free(p) }
        np
    }

    # The point of M3: this allocator owns a layout, so it can
    # describe one. A free "block" is a run of consecutive free slots,
    # which is exactly the unit a request has to fit into.
    unsafe fn layout_report(&self) -> AllocLayout {
        var blocks: u64 = 0u64
        var largest: u64 = 0u64
        var run: u64 = 0u64
        var i: u64 = 0u64
        while i < self.slot_count {
            val u: u64 = __builtin_ptr_read::<u64>(self.used, i * 8u64)
            if u == 0u64 {
                run = run + 1u64
                if run == 1u64 { blocks = blocks + 1u64 }
                if run > largest { largest = run }
            } else {
                run = 0u64
            }
            i = i + 1u64
        }
        AllocLayout {
            known: true,
            managed_bytes: self.slot_count * self.slot_bytes,
            live_bytes: self.live_slots * self.slot_bytes,
            free_blocks: blocks,
            largest_free: largest * self.slot_bytes,
        }
    }
}

# MEMORY_PROFILING M3 residual: the region's final layout is folded into
# the `--profile=mem` report. `Drop` fires just before `main` returns, so
# registering here is what makes the report automatic — the runtime
# profiler cannot reach back into a toylang object once the run ends, but
# the allocator itself is still alive at Drop time and pushes its numbers.
#
# The layout is registered *before* the slots are released: fragmentation
# is a statement about the intact region, and it stops meaning anything
# the moment the underlying slots are gone.
impl Drop for SlotRegion {
    unsafe fn drop(&mut self) {
        val l = self.layout_report()
        __builtin_record_allocator_layout("SlotRegion", l.managed(), l.live(), l.blocks(), l.largest())
        var i: u64 = 0u64
        while i < self.slot_count {
            val p: ptr = __builtin_ptr_read::<ptr>(self.ptrs, i * 8u64)
            with allocator = __builtin_default_allocator() {
                __builtin_heap_free(p)
            }
            i = i + 1u64
        }
        with allocator = __builtin_default_allocator() {
            __builtin_heap_free(self.ptrs)
            __builtin_heap_free(self.used)
        }
        self.ptrs = __builtin_null_ptr()
        self.used = __builtin_null_ptr()
        self.live_slots = 0u64
    }
}
