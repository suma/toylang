fn main() -> i64 {
    val a: [i64; 3] = [1, 2, 3]  # Should infer 1, 2, 3 as i64
    a[0u64] + a[1u64] + a[2u64]
}