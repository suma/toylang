# Allocator system — user-space List<u64>.
#
# There is no built-in List type: the language exposes pointer primitives
# that route through the current allocator, and a user-defined
# `struct List { data: ptr, len: u64, cap: u64 }` plus an impl block is
# enough to get a dynamic array whose buffer grows via heap_realloc.
# Wrapping the code in `with allocator = arena { ... }` automatically
# directs every push through the arena.
#
# `push` mutates the struct in place via field assignment — `self.len`,
# `self.cap`, and `self.data` are all updated so the caller sees the
# growth without having to rebind.
#
# Run: cargo run example/allocator_list.t
# Expected result: UInt64(60)

struct List {
    data: ptr,
    len: u64,
    cap: u64,
}

impl List {
    fn push(self: Self, value: u64) -> u64 {
        if self.cap == 0u64 {
            self.cap = 8u64
            self.data = __builtin_heap_realloc(self.data, self.cap * 8u64)
        } elif self.len >= self.cap {
            self.cap = self.cap * 2u64
            self.data = __builtin_heap_realloc(self.data, self.cap * 8u64)
        }
        __builtin_ptr_write(self.data, self.len * 8u64, value)
        self.len = self.len + 1u64
        self.len
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
        var list = make_list()
        list.push(10u64)
        list.push(20u64)
        list.push(30u64)
        list.get(0u64) + list.get(1u64) + list.get(2u64)
    }
}
