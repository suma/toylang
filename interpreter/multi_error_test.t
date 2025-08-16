fn main() -> u64 {
    val x: i64 = 5u64  # Type error: u64 to i64
    val y = undefined_var  # Undefined variable error
    missing_paren + 123)  # Parse error: unmatched parenthesis
    10u64
}