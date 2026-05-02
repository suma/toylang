# Stdlib Result<T, E> auto-loaded from core/std/result.t.
# Exercises is_ok / is_err / unwrap_or end-to-end. Uses
# `Result<u64, u64>` rather than `Result<u64, str>` because the AOT
# compiler MVP doesn't yet accept `str` enum payloads — keeps the
# fixture interpreter / JIT / AOT consistent.
# Expected exit: 152 = 42 + 99 + 1 + 10.

fn main() -> u64 {
    val ok: Result<u64, u64> = Result::Ok(42u64)
    val err: Result<u64, u64> = Result::Err(7u64)
    val a: u64 = ok.unwrap_or(0u64)
    val b: u64 = err.unwrap_or(99u64)
    val c: u64 = if ok.is_ok() { 1u64 } else { 0u64 }
    val d: u64 = if err.is_err() { 10u64 } else { 0u64 }
    a + b + c + d
}
