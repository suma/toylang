// Test mutual recursion between functions
fn is_even(n: u64) -> bool {
    if n == 0u64 {
        true
    } else {
        is_odd(n - 1u64)
    }
}

fn is_odd(n: u64) -> bool {
    if n == 0u64 {
        false
    } else {
        is_even(n - 1u64)
    }
}

fn main() -> u64 {
    val result = is_even(4u64)
    if result {
        1u64
    } else {
        0u64
    }
}