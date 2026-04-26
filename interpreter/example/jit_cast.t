# Smoke test for JIT Phase 2a: explicit i64 ↔ u64 casts. Returns 7
# to make verifying easy via shell exit code.
fn main() -> u64 {
    val signed: i64 = 10i64
    val unsigned: u64 = signed as u64    # i64 -> u64
    val back: i64 = unsigned as i64      # u64 -> i64
    val identity: u64 = unsigned as u64  # identity, also legal
    val total: u64 = unsigned - back as u64 + identity - 3u64
    total
}
