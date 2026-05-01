# JIT smoke test for `__builtin_fixed_buffer_allocator(capacity)`.
# Allocates a 64-byte fixed-buffer allocator, swaps it in via `with`,
# and exercises a successful 8-byte allocation followed by an
# overflow allocation that must return the null pointer (0).
#
# Expected: 1 + 0 + 7 = 8 → exit 8.
fn run_with(fb: Allocator) -> u64 {
    with allocator = fb {
        val p = __builtin_heap_alloc(8u64)
        val ok = if __builtin_ptr_is_null(p) {
            0u64
        } else {
            __builtin_heap_free(p)
            1u64
        }
        # Quota is 64 bytes; ask for 1024 — must fail.
        val q = __builtin_heap_alloc(1024u64)
        val overflow = if __builtin_ptr_is_null(q) {
            0u64
        } else {
            __builtin_heap_free(q)
            1u64
        }
        ok + overflow + 7u64
    }
}

fn main() -> u64 {
    val fb = __builtin_fixed_buffer_allocator(64u64)
    run_with(fb)
}
