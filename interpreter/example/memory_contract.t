/*
 * Asserting on memory (MEMORY_PROFILING M4).
 *
 * The allocation counters are readable from the program, so a
 * `ensures` clause or a `test` block can say what a function is
 * allowed to do to the heap. The names and meanings are exactly the
 * ones `--profile=mem` reports; the numbers start at zero for each
 * run and need no profiling flag to be truthful.
 *
 *   cargo run -q -p interpreter -- interpreter/example/memory_contract.t
 *   cargo run -q -p interpreter -- --test interpreter/example/memory_contract.t
 *   cargo run -q -p interpreter -- --check interpreter/example/memory_contract.t
 *
 * The `requires` clauses on `scratch` / `keep` exist so `--check`
 * can find inputs the bodies actually handle: `__builtin_heap_alloc`
 * answers an unsatisfiable size with the null pointer (like libc
 * `malloc`), so a contract over memory must bound what it accepts.
 */

# Takes a scratch buffer and gives it back. The contract says so:
# whatever this leaves behind, it is not bytes. The buffer must be
# able to hold a `u64`: `__builtin_ptr_write` stores 8 bytes at the
# offset, and the interpreter rejects a write that would run past the
# block (AOT would silently overflow — the strict side wins here).
unsafe fn scratch(size: u64) -> u64
    requires size >= 8u64
    requires size <= 4096u64
    ensures __builtin_live_bytes() == 0u64
{
    val p: ptr = __builtin_heap_alloc(size)
    __builtin_ptr_write(p, 0u64, size)
    val stored: u64 = __builtin_ptr_read(p, 0u64)
    __builtin_heap_free(p)
    stored
}

# A bound rather than an exact figure: this one keeps its buffer for
# the caller, but promises not to be extravagant about it.
fn keep(size: u64) -> ptr
    requires size > 0u64
    requires size <= 256u64
    ensures __builtin_live_bytes() <= 256u64
{
    __builtin_heap_alloc(size)
}

test "scratch work leaves nothing behind" {
    val before: u64 = __builtin_live_bytes()
    val r: u64 = scratch(128u64)
    assert_eq(__builtin_live_bytes(), before)
    assert_eq(r, 128u64)
}

test "one allocation is one allocation, however it is resized" {
    val p: ptr = __builtin_heap_alloc(16u64)
    val q: ptr = __builtin_heap_realloc(p, 64u64)
    # A resize is one request, not a free plus an allocate — an
    # allocator that grew the block in place would report the same.
    assert_eq(__builtin_alloc_count(), 1u64)
    assert_eq(__builtin_realloc_count(), 1u64)
    assert_eq(__builtin_live_bytes(), 64u64)
    __builtin_heap_free(q)
    assert_eq(__builtin_live_bytes(), 0u64)
}

test "the peak outlives the bytes that made it" {
    val p: ptr = __builtin_heap_alloc(512u64)
    __builtin_heap_free(p)
    assert_eq(__builtin_live_bytes(), 0u64)
    assert_eq(__builtin_peak_live_bytes(), 512u64)
    assert_eq(__builtin_cumulative_bytes(), 512u64)
}

fn main() -> u64 {
    val a: u64 = scratch(64u64)
    val p: ptr = keep(32u64)
    println(__builtin_live_bytes())
    __builtin_heap_free(p)
    a
}
