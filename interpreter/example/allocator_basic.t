# Allocator system — basic usage.
#
# Constructs an `Arena` (toylang stdlib wrapper), allocates a pointer
# through it, writes / reads a value, and lets `Drop` fire at scope
# exit to bulk-free every tracked allocation.
#
# Run: cargo run example/allocator_basic.t
# Expected result: UInt64(42)

fn main() -> u64 {
    val arena = Arena::new()
    val p: ptr = arena.alloc(16u64)
    __builtin_ptr_write(p, 0u64, 42u64)
    val v: u64 = __builtin_ptr_read(p, 0u64)
    v
}
