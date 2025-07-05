fn main() -> u64 {
    val a: [u64; 4] = [10u64, 20u64, 30u64, 40u64]
    val base = 1
    a[base + 1]  # 式 base + 1 は自動でu64に推論される
}