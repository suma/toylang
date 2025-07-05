fn main() -> u64 {
    val a: [u64; 5] = [1u64, 2u64, 3u64, 4u64, 5u64]
    val i = 2  # 変数 i は自動でu64に推論される
    a[i]
}