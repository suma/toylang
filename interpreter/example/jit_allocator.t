# Smoke test for JIT Phase 2e: `with allocator = ...` blocks.
# heap_alloc inside the `with` block dispatches through the arena
# allocator on top of the JIT runtime's active stack; everything
# outside still uses the global allocator. The matching pop runs at
# the end of the body.
#
# Expected exit: 12345
fn main() -> u64 {
    val arena = __builtin_arena_allocator()
    val total: u64 = with allocator = arena {
        val p: ptr = __builtin_heap_alloc(8u64)
        __builtin_ptr_write(p, 0u64, 12345u64)
        val x: u64 = __builtin_ptr_read(p, 0u64)
        x
    }
    total
}
