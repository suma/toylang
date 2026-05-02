package math

# Existing helper exports kept for backward-compat with module
# integration tests.

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b
}

# Math intrinsics — user-facing wrappers around the low-level
# `__builtin_*` symbols. The intrinsics themselves are intentionally
# kept under the `__builtin_` prefix so the global namespace stays
# clean; everything goes through `math::abs(...)` / `math::sqrt(...)`
# / `math::min_i64(...)` / etc.

pub fn abs(x: i64) -> i64 {
    __builtin_abs(x)
}

pub fn sqrt(x: f64) -> f64 {
    __builtin_sqrt(x)
}

pub fn min_i64(a: i64, b: i64) -> i64 {
    __builtin_min(a, b)
}

pub fn min_u64(a: u64, b: u64) -> u64 {
    __builtin_min(a, b)
}

pub fn max_i64(a: i64, b: i64) -> i64 {
    __builtin_max(a, b)
}

pub fn max_u64(a: u64, b: u64) -> u64 {
    __builtin_max(a, b)
}

pub fn pow(base: f64, exp: f64) -> f64 {
    __builtin_pow_f64(base, exp)
}

fn private_helper() -> u64 {
    42u64
}
