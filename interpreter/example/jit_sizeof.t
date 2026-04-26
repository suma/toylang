# Smoke test for JIT Phase 2g: __builtin_sizeof on scalar types.
# Expected exit: 8 + 8 + 1 + 8 = 25
fn main() -> u64 {
    val si: u64 = __builtin_sizeof(0i64)
    val su: u64 = __builtin_sizeof(0u64)
    val sb: u64 = __builtin_sizeof(true)
    val sp: u64 = __builtin_sizeof(__builtin_heap_alloc(0u64))
    si + su + sb + sp
}
