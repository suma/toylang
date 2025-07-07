fn test_comprehensive_string_len() -> u64 {
    # Test various string lengths
    val empty = ""
    val short = "hi"
    val medium = "hello world"
    val long = "this is a longer string with more characters"
    
    # Test expressions with string len
    val sum = empty.len() + short.len() + medium.len() + long.len()
    
    # Test comparison with string len
    if long.len() > short.len() {
        sum + 1u64
    } else {
        sum
    }
}

fn main() -> u64 {
    test_comprehensive_string_len()
}