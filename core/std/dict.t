# NOTE: no `package` line — same reason as `core/std/str.t` /
# `core/std/hash.t`. Auto-load derives the
# module path from the file system (`core/std/dict.t -> ["std",
# "dict"]`).
#
# Stdlib `Dict<K, V>` — a user-space hash table implemented entirely
# on top of the language's pointer primitives
# (`__builtin_heap_alloc` / `__builtin_heap_realloc` /
# `__builtin_ptr_read` / `__builtin_ptr_write` /
# `__builtin_sizeof`). No special-casing in the parser, the type
# checker, or any backend.
#
# Layout (COLLECTIONS C1, `design-docs/COLLECTIONS.md`): the entries
# stay where they were, in insertion order in the parallel `keys` /
# `vals` arrays, and a separate power-of-two `slots` table of u32
# indices into them is what lookup probes. Keeping the entries apart
# from the table is what lets iteration stay in insertion order — and
# lets `DictIter` and its adapters keep the exact fields they had,
# which the backends' 8-return register budget leaves no room to grow.
#
# Field packing: `caps` and `sizes` each hold two 32-bit numbers
# rather than taking a field apiece. A `&mut self` method returns one
# register per receiver leaf, so `remove(&mut self) -> bool` at six
# leaves plus its result is close to the budget; unpacked, the struct
# would not fit. Entry counts and byte widths are far below 2^32.
#
# `get(key) -> Option<V>` is the canonical lookup that surfaces
# presence in the value (`Option::Some(v)` on hit, `Option::None`
# on miss). `get_or(key, default) -> V` is kept as a convenience
# for callers that already have a fallback handy and want to skip
# the `match`. Internal references to `Option` survive even when
# user code declares its own `struct Option<T>`: the auto-load
# integration aliases stdlib `Option` to `__std_Option` in that
# case so dict.t's `-> Option<V>` still resolves to the stdlib
# enum (DICT-CROSS-MODULE-OPTION fix).
#
# Costs: `insert` / `get` / `get_or` / `contains_key` are O(1)
# expected. `remove` is O(n): it shifts the later entries down to
# keep insertion order and then rebuilds the table, because every
# index the table holds above the hole has moved. A tombstone would
# make removal O(1), but marking an entry dead needs a liveness
# column, and reading it would push the iterator adapters over the
# register budget (see above). Removal is the rarest of the four;
# that is the trade.
#
# `K: Hash` is a real bound, enforced at the call site like any other
# (`[E0010]`). A struct key needs `impl Hash for K` as well as the
# `eq` the lookup compares with — there is no derive here.

# The reserved `slots` value for an empty slot. `pub fn` rather than
# `pub const` because a module-level const is not visible from another
# module (todo MODULE-CONST) — same workaround as `core/std/poll.t`.
pub fn dict_slot_empty() -> u32 {
    0xFFFFFFFFu32
}

struct Dict<K: Hash, V> {
    keys: ptr,
    vals: ptr,
    # u32 index into the entries, or `dict_slot_empty()`.
    slots: ptr,
    count: u64,
    # entry capacity in the high 32 bits, slot-table size in the low.
    caps: u64,
    # key byte width in the high 32 bits, value width in the low.
    sizes: u64,
}

impl<K: Hash, V> Dict<K, V> {
    fn new() -> Self {
        Dict {
            keys: __builtin_heap_alloc(0u64),
            vals: __builtin_heap_alloc(0u64),
            slots: __builtin_heap_alloc(0u64),
            count: 0u64,
            caps: 0u64,
            sizes: 0u64,
        }
    }

    # Insert or update.
    #
    # Probes the table for the key: on a hit, overwrite the value in
    # place (the entry keeps its position, so an update does not
    # reorder iteration); on a miss, append the entry and point the
    # empty slot the probe stopped on at it. The early `return` from
    # inside the loop relies on the DICT-RETURN-WHILE fix to the
    # interpreter loop evaluator.
    unsafe fn insert(&mut self, key: K, value: V) {
        if self.sizes == 0u64 {
            self.sizes = (__builtin_sizeof(key) << 32u64) | __builtin_sizeof(value)
        }
        val ks: u64 = self.sizes >> 32u64
        val vs: u64 = self.sizes & 0xFFFFFFFFu64
        var ecap: u64 = self.caps >> 32u64
        var scap: u64 = self.caps & 0xFFFFFFFFu64

        # First insert: give the table a floor of 8 slots.
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
        var j: u64 = hash_mix(key.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: K = __builtin_ptr_read::<K>(self.keys, idx * ks)
            if existing == key {
                __builtin_ptr_write(self.vals, idx * vs, value)
                return
            }
            j = (j + 1u64) & mask
        }

        if ecap == 0u64 {
            ecap = 4u64
            self.keys = __builtin_heap_realloc(self.keys, ecap * ks)
            self.vals = __builtin_heap_realloc(self.vals, ecap * vs)
            self.caps = (ecap << 32u64) | scap
        } elif self.count >= ecap {
            ecap = ecap * 2u64
            self.keys = __builtin_heap_realloc(self.keys, ecap * ks)
            self.vals = __builtin_heap_realloc(self.vals, ecap * vs)
            self.caps = (ecap << 32u64) | scap
        }
        __builtin_ptr_write(self.keys, self.count * ks, key)
        __builtin_ptr_write(self.vals, self.count * vs, value)
        __builtin_ptr_write(self.slots, j * 4u64, self.count as u32)
        self.count = self.count + 1u64

        # Grow the table past a 7/8 load factor. Linear probing needs
        # the empty slots: at a full table the probe never terminates.
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
                val k2: K = __builtin_ptr_read::<K>(self.keys, i * ks)
                var p: u64 = hash_mix(k2.hash()) & nmask
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
    }

    # Look up `key`; on hit return the stored value, on miss
    # return `default`.
    unsafe fn get_or(self: Self, key: K, default: V) -> V {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return default
        }
        val ks: u64 = self.sizes >> 32u64
        val vs: u64 = self.sizes & 0xFFFFFFFFu64
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(key.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: K = __builtin_ptr_read::<K>(self.keys, idx * ks)
            if existing == key {
                val v: V = __builtin_ptr_read::<V>(self.vals, idx * vs)
                return v
            }
            j = (j + 1u64) & mask
        }
        default
    }

    # Option-returning lookup. Returns `Option::Some(v)` on hit,
    # `Option::None` on miss.
    unsafe fn get(self: Self, key: K) -> Option<V> {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return Option::None
        }
        val ks: u64 = self.sizes >> 32u64
        val vs: u64 = self.sizes & 0xFFFFFFFFu64
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(key.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: K = __builtin_ptr_read::<K>(self.keys, idx * ks)
            if existing == key {
                val v: V = __builtin_ptr_read::<V>(self.vals, idx * vs)
                return Option::Some(v)
            }
            j = (j + 1u64) & mask
        }
        Option::None
    }

    unsafe fn contains_key(self: Self, key: K) -> bool {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return false
        }
        val ks: u64 = self.sizes >> 32u64
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(key.hash()) & mask
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: K = __builtin_ptr_read::<K>(self.keys, idx * ks)
            if existing == key {
                return true
            }
            j = (j + 1u64) & mask
        }
        false
    }

    fn size(self: Self) -> u64 {
        self.count
    }

    # Remove `key` if present, returning whether it was there.
    #
    # Order-preserving: the entries after the hole shift down one, so
    # iteration still yields what is left in insertion order. Every
    # index the table holds above the hole has therefore moved, which
    # is why the table is rebuilt rather than patched — see the cost
    # note in the file header.
    unsafe fn remove(&mut self, key: K) -> bool {
        val scap: u64 = self.caps & 0xFFFFFFFFu64
        if scap == 0u64 {
            return false
        }
        val ks: u64 = self.sizes >> 32u64
        val vs: u64 = self.sizes & 0xFFFFFFFFu64
        val mask: u64 = scap - 1u64
        var j: u64 = hash_mix(key.hash()) & mask
        var found: u64 = self.count
        loop {
            val s: u32 = __builtin_ptr_read::<u32>(self.slots, j * 4u64)
            if s == dict_slot_empty() {
                break
            }
            val idx: u64 = s as u64
            val existing: K = __builtin_ptr_read::<K>(self.keys, idx * ks)
            if existing == key {
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
            val nk: K = __builtin_ptr_read::<K>(self.keys, (i + 1u64) * ks)
            val nv: V = __builtin_ptr_read::<V>(self.vals, (i + 1u64) * vs)
            __builtin_ptr_write(self.keys, i * ks, nk)
            __builtin_ptr_write(self.vals, i * vs, nv)
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
            val k2: K = __builtin_ptr_read::<K>(self.keys, e * ks)
            var p: u64 = hash_mix(k2.hash()) & mask
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
}

# Iterator-protocol support (STDLIB-ITER): `for kv in d.iter() { ... }`
# yields `(key, value)` tuples in insertion order, and keeps doing so
# after a removal — `remove` shifts the survivors down rather than
# swapping the last entry into the hole (COLLECTIONS C1). An update
# through `insert` keeps the entry where it was; a re-insert after a
# removal goes to the end.
#
# Same structural protocol as `Vec::iter` — a
# `next(&mut self) -> Option<(K, V)>` method, no `trait Iterator` impl
# required. `K` / `V` appear in no field of the iterator (like
# `Box<T>`), so it needs no instantiation of its own.
#
# The two element strides are packed into one `sizes` field (key in
# the high 32 bits, value in the low 32): the AOT's `&mut self`
# writeback returns one register per receiver leaf, and this iterator
# with 6 fields + a 3-leaf enum return would exceed the 8-return
# cranelift ABI limit. Element sizes are byte widths of realistic
# keys / values — far below 2^32.
struct DictIter<K, V> {
    keys: ptr,
    vals: ptr,
    count: u64,
    sizes: u64,
    index: u64,
}

# The extra bound lives on its own block so `Dict<K, V>` does not
# require `V: Default` everywhere -- the same shape `impl<T: Ord>
# Vec<T>` uses for `sort`.
impl<K: Hash, V: Default> Dict<K, V> {
    # Look up `key`, answering with `V`'s default on a miss.
    #
    # `get_or` needs a value the caller already has; this one needs
    # only the *type* to have an answer, which is what `Default` is
    # for (STDLIB-TRAIT-BASE §7). Counting occurrences is the shape:
    #
    #     val n = counts.get_or_default(word)
    #     counts.insert(word, n + 1u64)
    unsafe fn get_or_default(self: Self, key: K) -> V {
        val zero: V = V::default()
        self.get_or(key, zero)
    }
}

impl<K: Hash, V> Dict<K, V> {
    # Borrow the dict into an iterator. `&self` keeps the caller's
    # binding alive; the returned iterator shares the key / value
    # buffers. It walks the entries, not the slot table, which is what
    # keeps the order the entries were inserted in.
    fn iter(&self) -> DictIter<K, V> {
        DictIter {
            keys: self.keys,
            vals: self.vals,
            count: self.count,
            sizes: self.sizes,
            index: 0u64,
        }
    }
}

impl<K, V> Iterator<(K, V)> for DictIter<K, V> {
    # Advance by one entry. Returns `None` once `index` has walked
    # past `count`. Keys and values are read as copies out of the
    # buffers — like `Dict::get`, compound entries alias the stored
    # values.
    unsafe fn next(&mut self) -> Option<(K, V)> {
        if self.index >= self.count {
            Option::None
        } else {
            val i = self.index
            self.index = self.index + 1u64
            val ks: u64 = self.sizes >> 32u64
            val vs: u64 = self.sizes & 0xFFFFFFFFu64
            val k: K = __builtin_ptr_read::<K>(self.keys, i * ks)
            val v: V = __builtin_ptr_read::<V>(self.vals, i * vs)
            Option::Some((k, v))
        }
    }
}


# Iterator adapters (STDLIB-ITER-ADAPT): `map` / `filter` /
# `enumerate` on a `DictIter<K, V>`. Same design as the `VecIter`
# adapters in `core/std/collections/vec.t` — ordinary structs exposing
# `fn next(&mut self) -> Option<T>`, type params kept out of every
# field so no per-monomorph layout is needed.
#
# The adapter receiver holds a 5-leaf `DictIter` plus a 2-leaf fn
# field — 7 receiver leaves + a 2-leaf Option return exceeds the
# backend's 8-return register budget. The adapters therefore keep the
# iterator state FLAT (no nested `DictIter`) and pack `count` into
# the high 32 bits of the same field as `index` (like `sizes`):
# 4 state leaves + 2 fn leaves + 2 return leaves = 8, exactly at the
# budget. `collect` is not provided here: a `Vec<(K, V)>` return
# would blow the budget, and tuple-element Vecs are not AOT-lowerable.

struct DictMapIter<K, V, U> {
    keys: ptr,
    vals: ptr,
    count_index: u64,
    sizes: u64,
    f: fn (K, V) -> U,
}

impl<K, V, U> Iterator<U> for DictMapIter<K, V, U> {
    # Apply `f` to each `(key, value)` pair on the way out. `f` takes
    # the key and value as separate scalar args: an AOT closure cannot
    # receive a tuple parameter, so the adapter destructures the pair
    # before calling.
    unsafe fn next(&mut self) -> Option<U> {
        val count = self.count_index >> 32u64
        val index = self.count_index & 0xFFFFFFFFu64
        if index >= count {
            Option::None
        } else {
            val ks: u64 = self.sizes >> 32u64
            val vs: u64 = self.sizes & 0xFFFFFFFFu64
            val k: K = __builtin_ptr_read::<K>(self.keys, index * ks)
            val v: V = __builtin_ptr_read::<V>(self.vals, index * vs)
            self.count_index = (count << 32u64) | (index + 1u64)
            Option::Some(self.f(k, v))
        }
    }
}


impl<K, V> DictIter<K, V> {
    fn map<U>(&self, f: fn (K, V) -> U) -> DictMapIter<K, V, U> {
        DictMapIter {
            keys: self.keys,
            vals: self.vals,
            count_index: (self.count << 32u64) | self.index,
            sizes: self.sizes,
            f: f,
        }
    }
}

struct DictFilterIter<K, V> {
    keys: ptr,
    vals: ptr,
    count_index: u64,
    sizes: u64,
    pred: fn (K, V) -> bool,
}

impl<K, V> Iterator<(K, V)> for DictFilterIter<K, V> {
    # Yield only the pairs for which `pred` returns true.
    unsafe fn next(&mut self) -> Option<(K, V)> {
        loop {
            val count = self.count_index >> 32u64
            val index = self.count_index & 0xFFFFFFFFu64
            if index >= count {
                break
            }
            val ks: u64 = self.sizes >> 32u64
            val vs: u64 = self.sizes & 0xFFFFFFFFu64
            val k: K = __builtin_ptr_read::<K>(self.keys, index * ks)
            val v: V = __builtin_ptr_read::<V>(self.vals, index * vs)
            self.count_index = (count << 32u64) | (index + 1u64)
            if self.pred(k, v) {
                val r: Option<(K, V)> = Option::Some((k, v))
                return r
            }
        }
        val r: Option<(K, V)> = Option::None
        r
    }
}


impl<K, V> DictIter<K, V> {
    fn filter(&self, pred: fn (K, V) -> bool) -> DictFilterIter<K, V> {
        DictFilterIter {
            keys: self.keys,
            vals: self.vals,
            count_index: (self.count << 32u64) | self.index,
            sizes: self.sizes,
            pred: pred,
        }
    }
}
