# The `null` literal is reserved but has no working semantics in any
# backend: it parses and type-checks (a `null` in a typed position
# takes that position's type), but evaluating it stops the program.
# There is no universal `is_null()` either. Model absence with
# `Option<T>` and test raw pointers with `__builtin_ptr_is_null`.
fn main() -> u64 {
    val p: ptr = __builtin_null_ptr()
    if !__builtin_ptr_is_null(p) { return 1u64 }
    val none: Option<i64> = Option::None
    if !none.is_none() { return 2u64 }
    val some: Option<i64> = Option::Some(7i64)
    if some.is_none() { return 3u64 }
    42u64
}