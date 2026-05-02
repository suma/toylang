# #195: generic `extern fn` parses, type-checks, and dispatches via
# the runtime extern registry. The interpreter's registry is keyed
# by literal name (no monomorph), so a single identity closure
# satisfies every T the type-checker accepts.

extern fn __extern_test_identity<T>(x: T) -> T

fn main() -> u64 {
    # Same generic extern instantiated at three different T,
    # each routed to the type-erased registry impl.
    val a: u64 = __extern_test_identity(7u64)
    val b: i64 = __extern_test_identity(-3i64)
    val c: bool = __extern_test_identity(true)
    if c {
        a + (b * (0i64 - 1i64)) as u64
    } else {
        0u64
    }
}
