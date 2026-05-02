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

pub fn fabs(x: f64) -> f64 {
    # C-style fabs: IEEE 754 absolute value (sign-bit flip,
    # preserves NaN). The polymorphic `__builtin_abs` dispatches
    # on the operand type, so we just forward.
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

# f64 transcendentals — forward to libm in the AOT compiler / a
# Rust shim in the JIT. cranelift has no native opcodes for sin /
# cos / tan / log / log2 / exp, so each call hits the runtime
# helper at execution time.

pub fn sin(x: f64) -> f64 {
    __builtin_sin_f64(x)
}

pub fn cos(x: f64) -> f64 {
    __builtin_cos_f64(x)
}

pub fn tan(x: f64) -> f64 {
    __builtin_tan_f64(x)
}

pub fn log(x: f64) -> f64 {
    __builtin_log_f64(x)
}

pub fn log2(x: f64) -> f64 {
    __builtin_log2_f64(x)
}

pub fn exp(x: f64) -> f64 {
    __builtin_exp_f64(x)
}

# f64 rounding — both lower to cranelift's native `floor` /
# `ceil` instructions on every supported ISA.

pub fn floor(x: f64) -> f64 {
    __builtin_floor_f64(x)
}

pub fn ceil(x: f64) -> f64 {
    __builtin_ceil_f64(x)
}

fn private_helper() -> u64 {
    42u64
}
