fn main() -> bool {
    val inferred_bool_array = [true, false, true, false]
    val comparison_array = [1u64 == 1u64, 2u64 > 3u64, 5u64 <= 5u64]
    inferred_bool_array[0u64] && comparison_array[0u64]
}