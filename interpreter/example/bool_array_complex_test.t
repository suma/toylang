fn main() -> bool {
    val conditions: [bool; 4] = [true, false, true, false]
    val results = [1u64 > 0u64, 2u64 < 1u64, 3u64 == 3u64, 4u64 != 4u64]
    conditions[0u64] && results[2u64]
}