# Allocator system — basic usage.
#
# Creates an arena allocator, enters a `with` block so heap operations
# route through it, then performs a simple heap_alloc + ptr_write +
# ptr_read round-trip. When the arena is dropped at block exit, all
# outstanding allocations are released in bulk.
#
# Run: cargo run example/allocator_basic.t
# Expected result: UInt64(42)

fn main() -> u64 {
    val arena = __builtin_arena_allocator()
    with allocator = arena {
        val p = __builtin_heap_alloc(16u64)
        __builtin_ptr_write(p, 0u64, 42u64)
        __builtin_ptr_read(p, 0u64)
    }
}
