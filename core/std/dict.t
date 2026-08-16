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
    fn insert(&mut self, key: K, value: V) {
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
    fn get_or(self: Self, key: K, default: V) -> V {
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
    fn get(self: Self, key: K) -> Option<V> {
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

    fn contains_key(self: Self, key: K) -> bool {
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
    fn remove(&mut self, key: K) -> bool {
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
# yields `(key, value)` tuples in insertion order. Same structural
# protocol as `Vec::iter` — a `next(&mut self) -> Option<(K, V)>`
# method, no `trait Iterator` impl required. `K` / `V` appear in no
# field of the iterator (like `Box<T>`), so it needs no instantiation
# of its own.
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
    fn next(&mut self) -> Option<(K, V)> {
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
