# Debug: String array with 2 elements test

fn main() -> u64 {
    # Test explicit type annotation first
    val arr: [str; 2] = ["hello", "world"]
    
    # Array element access
    val first_len = arr[0u64].len()   # "hello".len() = 5
    val second_len = arr[1u64].len()  # "world".len() = 5
    
    first_len + second_len
}