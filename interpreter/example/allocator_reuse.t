# Allocator system — `Arena::reset()` + `FixedBuffer` introspection.
#
# Demonstrates the Odin/Zig-style methods exposed by the
# `core/std/allocator.t` wrapper structs:
#   - `arena.bytes_used()` — cumulative tracked bytes
#   - `arena.reset()` — bulk-free without dropping the wrapper
#   - `fb.capacity()` / `.used()` / `.remaining()` / `.is_empty()`
#
# These are accessible when the wrapper is bound to a name —
# the inline `with allocator = Arena::new() { ... }` form
# bypasses the wrapper and routes raw heap_alloc through the
# runtime arena, where these methods aren't reachable.
#
# Run: cargo run example/allocator_reuse.t
# Expected exit code: 14 (every assertion passes)

fn main() -> u64 {
    val arena = Arena::new()

    val p1: ptr = arena.alloc(64u64)
    val p2: ptr = arena.alloc(32u64)
    # bytes_used should be 96 right now
    val a: u64 = if arena.bytes_used() == 96u64 { 1u64 } else { 0u64 }

    arena.reset()
    # after reset, counter is 0 and arena is reusable
    val b: u64 = if arena.bytes_used() == 0u64 { 1u64 } else { 0u64 }
    val p3: ptr = arena.alloc(8u64)
    val c: u64 = if arena.bytes_used() == 8u64 { 1u64 } else { 0u64 }
    val c2: u64 = if !__builtin_ptr_is_null(p3) { 1u64 } else { 0u64 }

    val fb = FixedBuffer::new(128u64)
    val d: u64 = if fb.is_empty() { 1u64 } else { 0u64 }
    val d2: u64 = if fb.capacity() == 128u64 { 1u64 } else { 0u64 }
    val d3: u64 = if fb.remaining() == 128u64 { 1u64 } else { 0u64 }

    val q1: ptr = fb.alloc(40u64)
    val e: u64 = if fb.used() == 40u64 { 1u64 } else { 0u64 }
    val e2: u64 = if fb.remaining() == 88u64 { 1u64 } else { 0u64 }
    val e3: u64 = if !fb.is_empty() { 1u64 } else { 0u64 }

    # Quota exhaustion returns null.
    val q_too_big: ptr = fb.alloc(200u64)
    val f: u64 = if __builtin_ptr_is_null(q_too_big) { 1u64 } else { 0u64 }

    fb.free(q1)
    val g: u64 = if fb.used() == 0u64 { 1u64 } else { 0u64 }
    val g2: u64 = if fb.is_empty() { 1u64 } else { 0u64 }

    fb.reset()
    val h: u64 = if fb.is_empty() { 1u64 } else { 0u64 }

    arena.drop()
    fb.drop()

    # All 16 checks must pass; collapse to 1 / 0.
    a + b + c + c2 + d + d2 + d3 + e + e2 + e3 + f + g + g2 + h
}
