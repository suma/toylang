# Debug: Test if strings parse correctly when separate

fn main() -> u64 {
    # Test individual string variables
    val str1 = "hello"
    val str2 = "world"
    
    # Check lengths
    val len1 = str1.len()  # Should be 5
    val len2 = str2.len()  # Should be 5
    
    len1 + len2  # Should be 10
}