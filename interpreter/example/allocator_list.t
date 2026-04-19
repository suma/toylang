# Allocator system — user-space List<u64>.
#
# There is no built-in List type: the language instead exposes pointer
# primitives that route through the current allocator. A user-defined
# `struct List { data: ptr, len: u64, cap: u64 }` plus an impl block is
# enough to get a dynamic array whose buffer grows via heap_realloc.
# Because every heap call uses the innermost allocator stack entry,
# wrapping the code in `with allocator = arena { ... }` automatically
# directs every push through the arena.
#
# The impl below is purely functional — each push returns a fresh List
# referring to the reallocated buffer — because method bodies cannot yet
# mutate `self` fields in place. An imperative variant becomes possible
# once field-assignment lands as a language feature.
#
# Run: cargo run example/allocator_list.t
# Expected result: UInt64(60)

struct List {
    data: ptr,
    len: u64,
    cap: u64,
}

impl List {
    fn push(self: Self, value: u64) -> Self {
        var new_cap: u64 = self.cap
        if self.cap == 0u64 {
            new_cap = 8u64
        } elif self.len >= self.cap {
            new_cap = self.cap * 2u64
        }
        var new_data: ptr = self.data
        if new_cap != self.cap {
            new_data = __builtin_heap_realloc(self.data, new_cap * 8u64)
        }
        __builtin_ptr_write(new_data, self.len * 8u64, value)
        List { data: new_data, len: self.len + 1u64, cap: new_cap }
    }

    fn get(self: Self, index: u64) -> u64 {
        __builtin_ptr_read(self.data, index * 8u64)
    }
}

fn make_list() -> List {
    List { data: __builtin_heap_alloc(0u64), len: 0u64, cap: 0u64 }
}

fn main() -> u64 {
    val arena = __builtin_arena_allocator()
    with allocator = arena {
        val list = make_list().push(10u64).push(20u64).push(30u64)
        list.get(0u64) + list.get(1u64) + list.get(2u64)
    }
}
