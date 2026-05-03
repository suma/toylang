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
#
# Pre-existing language quirk worked around: `return` inside a
# `while` loop body silently fails to exit the enclosing
# function. Each method below uses a `found` / `replaced` flag
# plus manual loop-exit (`i = self.count`) to leave the loop,
# then handles the rest of the function body after the loop.

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

    fn insert(self: Self, key: K, value: V) {
        if self.key_size == 0u64 {
            self.key_size = __builtin_sizeof(key)
            self.val_size = __builtin_sizeof(value)
        }
        var replaced: bool = false
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                __builtin_ptr_write(self.vals, i * self.val_size, value)
                replaced = true
                i = self.count
            } else {
                i = i + 1u64
            }
        }
        if replaced {
            # Update done.
        } else {
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
    }

    fn get_or(self: Self, key: K, default: V) -> V {
        var found: bool = false
        var found_idx: u64 = 0u64
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                found = true
                found_idx = i
                i = self.count
            } else {
                i = i + 1u64
            }
        }
        if found {
            val v: V = __builtin_ptr_read(self.vals, found_idx * self.val_size)
            v
        } else {
            default
        }
    }

    fn contains_key(self: Self, key: K) -> bool {
        var found: bool = false
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                found = true
                i = self.count
            } else {
                i = i + 1u64
            }
        }
        found
    }

    fn size(self: Self) -> u64 {
        self.count
    }

    fn remove(self: Self, key: K) -> bool {
        var found: bool = false
        var found_idx: u64 = 0u64
        var i: u64 = 0u64
        while i < self.count {
            val existing: K = __builtin_ptr_read(self.keys, i * self.key_size)
            if existing == key {
                found = true
                found_idx = i
                i = self.count
            } else {
                i = i + 1u64
            }
        }
        if found {
            val last_idx: u64 = self.count - 1u64
            if found_idx != last_idx {
                val last_k: K = __builtin_ptr_read(self.keys, last_idx * self.key_size)
                val last_v: V = __builtin_ptr_read(self.vals, last_idx * self.val_size)
                __builtin_ptr_write(self.keys, found_idx * self.key_size, last_k)
                __builtin_ptr_write(self.vals, found_idx * self.val_size, last_v)
            }
            self.count = last_idx
            true
        } else {
            false
        }
    }
}
