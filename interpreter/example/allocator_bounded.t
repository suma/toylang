# Allocator system — bounded generic + ambient sugar + auto-default.
#
# `store` is generic over any `A: Allocator`, uses that allocator inside a
# `with` block, and returns the round-tripped value. The caller shows two
# equivalent ways to satisfy the Allocator parameter:
#
#   1. `store(100u64, ambient)` — `ambient` is sugar for the current allocator.
#   2. `store(200u64)`          — the type checker auto-injects the current
#                                  allocator because the omitted trailing
#                                  parameter is bounded by Allocator.
#
# Both calls happen inside a `with allocator = arena { ... }` block, so the
# "current allocator" they see is the arena, not the global default.
#
# Run: cargo run example/allocator_bounded.t
# Expected result: UInt64(300)

fn store<A: Allocator>(x: u64, a: A) -> u64 {
    with allocator = a {
        val p = __builtin_heap_alloc(8u64)
        __builtin_ptr_write(p, 0u64, x)
        __builtin_ptr_read(p, 0u64)
    }
}

fn main() -> u64 {
    val arena = __builtin_arena_allocator()
    with allocator = arena {
        val a = store(100u64, ambient)
        val b = store(200u64)
        a + b
    }
}
