# Debug: Explicit type annotation with string array

fn main() -> u64 {
    # Test with explicit size annotation
    val arr: [str; 2] = ["hello", "world"]
    
    # Try to access both elements
    val first = arr[0u64].len()
    # Skip second element for now to avoid error
    first
}