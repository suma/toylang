fn main() -> bool {
    val flags_explicit: [bool; 3] = [true, false, true]
    val flags_inferred = [true, false, true]
    flags_explicit[0u64] && flags_explicit[2u64]
}