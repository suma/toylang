// Test forward function reference
fn main() -> u64 {
    val result = helper_function(5u64)
    result
}

fn helper_function(x: u64) -> u64 {
    x + 1u64
}