# Stdlib Option<T> auto-loaded from core/std/option.t.
# Exercises is_some / is_none / unwrap_or end-to-end.
# Expected exit: 152 = 42 + 99 + 1 + 10.

fn main() -> u64 {
    val o: Option<u64> = Option::Some(42u64)
    val n: Option<u64> = Option::None
    val a: u64 = o.unwrap_or(0u64)
    val b: u64 = n.unwrap_or(99u64)
    val c: u64 = if o.is_some() { 1u64 } else { 0u64 }
    val d: u64 = if n.is_none() { 10u64 } else { 0u64 }
    a + b + c + d
}
