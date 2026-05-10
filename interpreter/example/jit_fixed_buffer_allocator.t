# JIT smoke test for the `FixedBuffer` stdlib wrapper.
# Allocates a 64-byte fixed-buffer allocator, exercises a successful
# 8-byte allocation followed by an overflow allocation that must
# return the null pointer (0). Quota enforcement happens entirely in
# toylang (`FixedBuffer::alloc` checks `used_bytes + size > cap`).
#
# Expected: 1 + 0 + 7 = 8 → exit 8.
fn run_with(fb: FixedBuffer) -> u64 {
    val p = fb.alloc(8u64)
    val ok = if __builtin_ptr_is_null(p) {
        0u64
    } else {
        fb.free(p)
        1u64
    }
    # Quota is 64 bytes; ask for 1024 — must fail.
    val q = fb.alloc(1024u64)
    val overflow = if __builtin_ptr_is_null(q) {
        0u64
    } else {
        fb.free(q)
        1u64
    }
    ok + overflow + 7u64
}

fn main() -> u64 {
    val fb = FixedBuffer::new(64u64)
    val r: u64 = run_with(fb)
    fb.drop()
    r
}
