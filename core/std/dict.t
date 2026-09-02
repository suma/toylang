# NOTE: no `package` line — same reason as `core/std/i64.t` /
# `core/std/f64.t` / `core/std/hash.t`. Auto-load derives the
# module path from the file system (`core/std/dict.t -> ["std",
# "dict"]`).
#
# Stdlib `Dict<K, V>` — user-space hash-table-shaped collection
# implemented entirely on top of the language's pointer
# primitives (`__builtin_heap_alloc` / `__builtin_heap_realloc` /
# `__builtin_ptr_read` / `__builtin_ptr_write` /
# `__builtin_sizeof`). No special-casing in the parser, the type
# checker, or any backend.
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

struct Dict<K, V> {
    keys: ptr,
    vals: ptr,
    count: u64,
    cap: u64,
    key_size: u64,
    val_size: u64,
}

impl<K, V> Dict<K, V> {
    fn new() -> Self {
        Dict {
            keys: __builtin_heap_alloc(0u64),
            vals: __builtin_heap_alloc(0u64),
            count: 0u64,
            cap: 0u64,
            key_size: 0u64,
            val_size: 0u64,
        }
    }

    # Insert or update. Linear scan; on hit, overwrite the
    # value and return. On miss, fall through to the grow +
    # append path below. The early `return` from inside the
    # while loop relies on the DICT-RETURN-WHILE fix to the
    # interpreter loop evaluator (`88d9af6` predecessor).
    unsafe fn insert(&mut self, key: K, value: V) {
        if self.key_size == 0u64 {
            self.key_size = __builtin_sizeof(key)
            self.val_size = __builtin_sizeof(value)
        }
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                __builtin_ptr_write(self.vals, i * self.val_size, value)
                return
            }
            i = i + 1u64
        }
        if self.cap == 0u64 {
            self.cap = 4u64
            self.keys = __builtin_heap_realloc(self.keys, self.cap * self.key_size)
            self.vals = __builtin_heap_realloc(self.vals, self.cap * self.val_size)
        } elif self.count >= self.cap {
            self.cap = self.cap * 2u64
            self.keys = __builtin_heap_realloc(self.keys, self.cap * self.key_size)
            self.vals = __builtin_heap_realloc(self.vals, self.cap * self.val_size)
        }
        __builtin_ptr_write(self.keys, self.count * self.key_size, key)
        __builtin_ptr_write(self.vals, self.count * self.val_size, value)
        self.count = self.count + 1u64
    }

    # Look up `key`; on hit return the stored value, on miss
    # return `default`. Early-return from the loop body now
    # works (DICT-RETURN-WHILE).
    unsafe fn get_or(self: Self, key: K, default: V) -> V {
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                val v: V = __builtin_ptr_read(self.vals, i * self.val_size)
                return v
            }
            i = i + 1u64
        }
        default
    }

    # Option-returning lookup. Returns `Option::Some(v)` on hit,
    # `Option::None` on miss.
    unsafe fn get(self: Self, key: K) -> Option<V> {
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                val v: V = __builtin_ptr_read(self.vals, i * self.val_size)
                return Option::Some(v)
            }
            i = i + 1u64
        }
        Option::None
    }

    unsafe fn contains_key(self: Self, key: K) -> bool {
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                return true
            }
            i = i + 1u64
        }
        false
    }

    fn size(self: Self) -> u64 {
        self.count
    }

    # Remove `key` if present. On hit: swap-remove with the
    # last slot and return true. On miss: return false.
    #
    # The swap is what breaks iteration order (see the `DictIter`
    # header below): the last entry lands in the removed key's
    # position rather than everything after it shifting down.
    # Shifting would add an O(n) move to the O(n) search; the
    # ordering cost goes away with the entries/slots layout in
    # `design-docs/COLLECTIONS.md`, not by shifting here.
    unsafe fn remove(&mut self, key: K) -> bool {
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                val last_idx: u64 = self.count - 1u64
                if i != last_idx {
                    val last_k: K = __builtin_ptr_read(self.keys, last_idx * self.key_size)
                    val last_v: V = __builtin_ptr_read(self.vals, last_idx * self.val_size)
                    __builtin_ptr_write(self.keys, i * self.key_size, last_k)
                    __builtin_ptr_write(self.vals, i * self.val_size, last_v)
                }
                self.count = last_idx
                return true
            }
            i = i + 1u64
        }
        false
    }
}

# Iterator-protocol support (STDLIB-ITER): `for kv in d.iter() { ... }`
# yields `(key, value)` tuples in the order the entries sit in the
# parallel arrays. That is insertion order *until a key is removed*:
# `remove` above swap-removes, moving the last entry into the hole, so
# a deletion reorders the survivors (insert 1, 2, 3 then remove 1 and
# the iteration yields 3, 2). Callers must not depend on the order of
# a dict that has had a removal; making insertion order a guarantee
# that survives `remove` is phase C1 of
# `design-docs/COLLECTIONS.md`.
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

impl<K, V> Dict<K, V> {
    # Borrow the dict into an iterator. `&self` keeps the caller's
    # binding alive; the returned iterator shares the key / value
    # buffers.
    fn iter(&self) -> DictIter<K, V> {
        val sizes: u64 = (self.key_size << 32u64) | self.val_size
        DictIter {
            keys: self.keys,
            vals: self.vals,
            count: self.count,
            sizes: sizes,
            index: 0u64,
        }
    }
}

impl<K, V> DictIter<K, V> {
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
            val k: K = __builtin_ptr_read(self.keys, i * ks)
            val v: V = __builtin_ptr_read(self.vals, i * vs)
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

impl<K, V, U> DictMapIter<K, V, U> {
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
            val k: K = __builtin_ptr_read(self.keys, index * ks)
            val v: V = __builtin_ptr_read(self.vals, index * vs)
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

impl<K, V> DictFilterIter<K, V> {
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
            val k: K = __builtin_ptr_read(self.keys, index * ks)
            val v: V = __builtin_ptr_read(self.vals, index * vs)
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
