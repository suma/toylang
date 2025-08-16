# Debug: String array element count investigation

fn main() -> u64 {
    # Test: Check if both elements are parsed
    val arr = ["hello", "world"]
    
    # Only access first element to avoid runtime error
    val first = arr[0u64]
    first.len()
}