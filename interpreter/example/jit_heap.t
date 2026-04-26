# Smoke test for JIT Phase 2c: heap_alloc / heap_free / heap_realloc /
# ptr_is_null / mem_set. ptr_read / ptr_write are still unsupported in JIT
# so we don't read back the bytes; we only check pointer-shape invariants
# and exit code.
#
# Expected:
#   alloc -> non-null
#   alloc(0) -> null
#   realloc(p, 128) -> non-null
#   set + free succeeds
# Exit code: 42
fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(64u64)
    val p_ok: bool = !__builtin_ptr_is_null(p)
    __builtin_mem_set(p, 0u64, 64u64)

    val q: ptr = __builtin_heap_realloc(p, 128u64)
    val q_ok: bool = !__builtin_ptr_is_null(q)

    __builtin_heap_free(q)

    val zero: ptr = __builtin_heap_alloc(0u64)
    val zero_null: bool = __builtin_ptr_is_null(zero)

    if p_ok && q_ok && zero_null {
        42u64
    } else {
        0u64
    }
}
