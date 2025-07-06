fn main() -> i64 {
    val a: [i64; 3] = [1, 2, 3]
    val b = a[0u64]  # This should be i64, not u64
    b
}