fn main() -> u64 {
    var fib: [u64; 10] = [1u64, 1u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]
    
    for i in 2u64 to 10u64 {
        fib[i] = fib[i - 1u64] + fib[i - 2u64]
    }
    
    fib[9u64]
}