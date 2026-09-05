# NOTE: no `package` line — auto-load derives the module path from the
# file system (`core/std/collections/set.t -> ["std", "collections",
# "set"]`), the same way `vec.t` next to it does.
#
# Stdlib `Set<T>` — the membership half of `core/std/dict.t`, and the
# same table (COLLECTIONS C2, `design-docs/COLLECTIONS.md`).
#
# Why a separate struct rather than `Dict<T, ()>`: a unit value does
# not survive the compiled lanes (`parameter `value` cannot have type
# Unit`), and `Dict<T, bool>` would spend a byte per element on a value
# nobody reads *and* give `insert` the wrong answer — a set's `insert`
# reports whether the element is new, which an overwrite cannot.
#
# The probe, the mixer and the 7/8 growth threshold are deliberately
# the same arithmetic as `dict.t`. Two copies of that arithmetic is the
# honest cost of not having a way to abstract "a table with an optional
# value column" cheaply; a consistency test pins the two against each
# other by checking that a `Set` and a `Dict` fed the same keys iterate
# in the same order.
#
# Costs match `Dict`: `insert` / `contains` are O(1) expected, `remove`
# is O(n) because it keeps insertion order by shifting the survivors
# down and rebuilding the table.

struct Set<T: Hash> {
    elems: ptr,
    # u32 index into the elements, or `dict_slot_empty()`.
    slots: ptr,
    count: u64,
    # element capacity in the high 32 bits, slot-table size in the low.
    caps: u64,
    elem_size: u64,
}

impl<T: Hash> Set<T> {
    fn new() -> Self {
        Set {
            elems: __builtin_heap_alloc(0u64),
            slots: __builtin_heap_alloc(0u64),
            count: 0u64,
            caps: 0u64,
            elem_size: 0u64,
        }
    }

    # Add `value`. Returns whether it was new: an element already in
    # the set leaves it untouched (and keeps its position, so
    # re-adding does not reorder iteration).
    unsafe fn insert(&mut self, value: T) -> bool {
        if self.elem_size == 0u64 {
            self.elem_size = __builtin_sizeof(value)
        }
        val es: u64 = self.elem_size
        var ecap: u64 = self.caps >> 32u64
        var scap: u64 = self.caps & 0xFFFFFFFFu64

        if scap == 0u64 {
            scap = 8u64
            self.slots = __builtin_heap_realloc(self.slots, scap * 4u64)
            var t: u64 = 0u64
            while t < scap {
                __builtin_ptr_write(self.slots, t * 4u64, dict_slot_empty())
                t = t + 1u64
            }
            self.caps = (ecap << 32u64) | scap
        }

        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(value.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: T = __builtin_ptr_read::<T>(self.elems, idx * es)
            if existing == value {
                return false
            }
            j = (j + 1u64) & mask
        }

        if ecap == 0u64 {
            ecap = 4u64
            self.elems = __builtin_heap_realloc(self.elems, ecap * es)
            self.caps = (ecap << 32u64) | scap
        } elif self.count >= ecap {
            ecap = ecap * 2u64
            self.elems = __builtin_heap_realloc(self.elems, ecap * es)
            self.caps = (ecap << 32u64) | scap
        }
        __builtin_ptr_write(self.elems, self.count * es, value)
        __builtin_ptr_write(self.slots, j * 4u64, self.count as u32)
        self.count = self.count + 1u64

        # Past a 7/8 load factor the table doubles: linear probing needs
        # the empty slots, and a full table never terminates a probe.
        if self.count * 8u64 >= scap * 7u64 {
            val ncap: u64 = scap * 2u64
            self.slots = __builtin_heap_realloc(self.slots, ncap * 4u64)
            var t2: u64 = 0u64
            while t2 < ncap {
                __builtin_ptr_write(self.slots, t2 * 4u64, dict_slot_empty())
                t2 = t2 + 1u64
            }
            val nmask: u64 = ncap - 1u64
            var i: u64 = 0u64
            while i < self.count {
                val e2: T = __builtin_ptr_read::<T>(self.elems, i * es)
                var p: u64 = hash_mix(e2.hash()) & nmask
                loop {
                    val s2: u32 = __builtin_ptr_read::<u32>(self.slots, p * 4u64)
                    if s2 == dict_slot_empty() {
                        break
                    }
                    p = (p + 1u64) & nmask
                }
                __builtin_ptr_write(self.slots, p * 4u64, i as u32)
                i = i + 1u64
            }
            self.caps = (ecap << 32u64) | ncap
        }
        true
    }

    unsafe fn contains(&self, value: T) -> bool {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return false
        }
        val es: u64 = self.elem_size
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(value.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: T = __builtin_ptr_read::<T>(self.elems, idx * es)
            if existing == value {
                return true
            }
            j = (j + 1u64) & mask
        }
        false
    }

    # Drop `value` if present, returning whether it was there.
    #
    # Order-preserving, like `Dict::remove`: the elements after the
    # hole shift down one and the table is rebuilt, because every index
    # it holds above the hole has moved.
    unsafe fn remove(&mut self, value: T) -> bool {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return false
        }
        val es: u64 = self.elem_size
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(value.hash()) & mask
        var found: u64 = self.count
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: T = __builtin_ptr_read::<T>(self.elems, idx * es)
            if existing == value {
                found = idx
                break
            }
            j = (j + 1u64) & mask
        }
        if found >= self.count {
            return false
        }

        var i: u64 = found
        while i + 1u64 < self.count {
            val nv: T = __builtin_ptr_read::<T>(self.elems, (i + 1u64) * es)
            __builtin_ptr_write(self.elems, i * es, nv)
            i = i + 1u64
        }
        self.count = self.count - 1u64

        var t: u64 = 0u64
        while t < scap {
            __builtin_ptr_write(self.slots, t * 4u64, dict_slot_empty())
            t = t + 1u64
        }
        var e: u64 = 0u64
        while e < self.count {
            val e2: T = __builtin_ptr_read::<T>(self.elems, e * es)
            var p: u64 = hash_mix(e2.hash()) & mask
            loop {
                val s2: u32 = __builtin_ptr_read::<u32>(self.slots, p * 4u64)
                if s2 == dict_slot_empty() {
                    break
                }
                p = (p + 1u64) & mask
            }
            __builtin_ptr_write(self.slots, p * 4u64, e as u32)
            e = e + 1u64
        }
        true
    }

    fn size(&self) -> u64 {
        self.count
    }

    fn is_empty(&self) -> bool {
        self.count == 0u64
    }

    # Forget every element. Keeps the buffers: the table is emptied
    # rather than freed, so a set that is refilled does not re-allocate
    # (same treatment `Vec::clear` gives its buffer).
    unsafe fn clear(&mut self) {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        var t: u64 = 0u64
        while t < scap {
            __builtin_ptr_write(self.slots, t * 4u64, dict_slot_empty())
            t = t + 1u64
        }
        self.count = 0u64
    }
}

# Iterator protocol (STDLIB-ITER): `for v in s.iter() { ... }` yields
# the elements in insertion order, because it walks the elements rather
# than the slot table — the same reason `DictIter` does.
struct SetIter<T> {
    elems: ptr,
    count: u64,
    elem_size: u64,
    index: u64,
}

impl<T: Hash> Set<T> {
    # Borrow the set into an iterator. `&self` keeps the caller's
    # binding alive; the iterator shares the element buffer.
    fn iter(&self) -> SetIter<T> {
        SetIter {
            elems: self.elems,
            count: self.count,
            elem_size: self.elem_size,
            index: 0u64,
        }
    }
}

impl<T> Iterator<T> for SetIter<T> {
    unsafe fn next(&mut self) -> Option<T> {
        if self.index >= self.count {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val v: T = __builtin_ptr_read::<T>(self.elems, i * self.elem_size)
            Option::Some(v)
        }
    }
}

