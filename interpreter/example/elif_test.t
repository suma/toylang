fn main() -> u64 {
    val x = 15u64
    
    if x < 10u64 {
        1u64
    } elif x >= 10u64 && x < 20u64 {
        2u64
    } elif x >= 20u64 {
        3u64
    } else {
        0u64
    }
}