fn test_string_len() -> u64 {
    val str1 = "hello"
    val len1 = str1.len()
    
    val str2 = "world!"
    val len2 = str2.len()
    
    val str3 = ""
    val len3 = str3.len()
    
    len1 + len2 + len3
}

fn main() -> u64 {
    test_string_len()
}