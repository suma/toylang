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
# Why no `Option<V>` in the API: referencing `Option` from inside
# this module triggers a cross-contamination bug in the auto-load
# integration when downstream user code happens to declare its
# own `struct Option<T>` for unrelated purposes. Until that
# integration bug is fixed, the MVP API uses
# `get_or(key, default) -> V` (caller supplies the miss value)
# plus `contains_key(key) -> bool` for explicit presence probes.

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
    fn insert(self: Self, key: K, value: V) {
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
    fn remove(self: Self, key: K) -> bool {
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
