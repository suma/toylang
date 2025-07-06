fn main() -> u64 {
    val a: [u64; 3] = [1, 2, 3]  # Should infer 1, 2, 3 as u64
    a[0u64] + a[1u64] + a[2u64]
}