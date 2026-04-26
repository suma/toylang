# Smoke test for JIT Phase 2c-2: ptr_read / ptr_write across all
# supported scalar types (i64, u64, bool, ptr). Each round-trip writes a
# typed value through the heap and reads it back through a typed
# annotation so the JIT can route to the right helper.
#
# Final exit code = 1 + 2 + (1 if true else 0) + 99 = 103
fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(64u64)

    __builtin_ptr_write(p, 0u64, 1i64)
    __builtin_ptr_write(p, 8u64, 2u64)
    __builtin_ptr_write(p, 16u64, true)

    val q: ptr = __builtin_heap_alloc(8u64)
    __builtin_ptr_write(p, 24u64, q)

    val read_i: i64 = __builtin_ptr_read(p, 0u64)
    val read_u: u64 = __builtin_ptr_read(p, 8u64)
    val read_b: bool = __builtin_ptr_read(p, 16u64)
    val read_p: ptr = __builtin_ptr_read(p, 24u64)

    val flag: u64 = if read_b { 1u64 } else { 0u64 }
    val ptr_check: u64 = if !__builtin_ptr_is_null(read_p) { 99u64 } else { 0u64 }

    __builtin_heap_free(q)
    __builtin_heap_free(p)

    read_i as u64 + read_u + flag + ptr_check
}
