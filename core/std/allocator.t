# Stdlib `trait Alloc` + wrapper structs.
#
# `trait Alloc` provides the user-facing malloc / realloc /
# free interface. Implementations delegate to the underlying
# runtime allocator handle via `with allocator = self.h { ... }`,
# routing each call through the active-allocator stack so
# `__builtin_heap_alloc` dispatches to the right backend.
#
# Name split: `Allocator` (primitive runtime handle, current
# behaviour) vs `Alloc` (this trait). The two never collide:
#   - `Allocator` appears in type annotations, `<T: Allocator>`
#     bounds, and as the return type of every
#     `__builtin_*_allocator()` builtin.
#   - `Alloc` is purely a stdlib trait — declarations / impls
#     here, dispatch through the regular method-call path.
#
# `with allocator = arena { ... }` (passing a wrapper struct) is
# supported by extending `with` to accept any struct that impls
# `Alloc` and auto-extracting its single `Allocator`-typed field.

pub trait Alloc {
    fn alloc(&self, size: u64) -> ptr
    fn free(&self, p: ptr)
    fn realloc(&self, p: ptr, new_size: u64) -> ptr
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
    fn alloc(&self, size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_alloc(size)
        }
    }

    fn free(&self, p: ptr) {
        with allocator = self.h {
            __builtin_heap_free(p)
        }
    }

    fn realloc(&self, p: ptr, new_size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_realloc(p, new_size)
        }
    }
}

# ---- Arena allocator (bulk-free on `drop`) ----
pub struct Arena {
    h: Allocator,
}

impl Arena {
    fn new() -> Self {
        Arena { h: __builtin_arena_allocator() }
    }
}

# Phase 5: `Drop` impl. Releases every allocation tracked by this
# arena. Idempotent; the arena handle remains valid for further
# use. The temporary form
# `with allocator = Arena::new() { ... }` calls this implicitly
# at scope exit; named bindings (`val a = Arena::new()` then
# `a.drop()`) call it explicitly.
impl Drop for Arena {
    fn drop(&mut self) {
        __builtin_arena_drop(self.h)
    }
}

impl Alloc for Arena {
    fn alloc(&self, size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_alloc(size)
        }
    }

    fn free(&self, p: ptr) {
        # Arena `free` is a no-op at the runtime level (the
        # registry slot ignores per-pointer frees); routing
        # through the active stack preserves that semantics.
        with allocator = self.h {
            __builtin_heap_free(p)
        }
    }

    fn realloc(&self, p: ptr, new_size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_realloc(p, new_size)
        }
    }
}

# ---- Fixed-buffer allocator (capacity-limited) ----
pub struct FixedBuffer {
    h: Allocator,
    cap: u64,
}

impl FixedBuffer {
    fn new(capacity: u64) -> Self {
        FixedBuffer {
            h: __builtin_fixed_buffer_allocator(capacity),
            cap: capacity,
        }
    }

    fn capacity(&self) -> u64 {
        self.cap
    }
}

# Phase 5: `Drop` impl. Releases every allocation tracked by this
# fixed_buffer slot and resets the quota. The temporary form
# `with allocator = FixedBuffer::new(cap) { ... }` calls this
# implicitly at scope exit.
impl Drop for FixedBuffer {
    fn drop(&mut self) {
        __builtin_fixed_buffer_drop(self.h)
    }
}

impl Alloc for FixedBuffer {
    fn alloc(&self, size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_alloc(size)
        }
    }

    fn free(&self, p: ptr) {
        with allocator = self.h {
            __builtin_heap_free(p)
        }
    }

    fn realloc(&self, p: ptr, new_size: u64) -> ptr {
        with allocator = self.h {
            __builtin_heap_realloc(p, new_size)
        }
    }
}
