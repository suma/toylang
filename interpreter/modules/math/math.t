package math

# Existing helper exports kept for backward-compat with module
# integration tests.

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b
}

# ---------------------------------------------------------------------
# Rust-`core`-style stdlib bridge.
#
# The intrinsics are declared as `extern fn` with the canonical
# `__extern_*_f64` names. Each backend resolves them differently:
#
# - interpreter: dispatched by the registry in
#   `interpreter::evaluation::extern_math::build_default_registry`.
# - JIT (cranelift): routed by
#   `interpreter::jit::eligibility::JIT_EXTERN_DISPATCH` to either an
#   existing runtime helper (sin/cos/tan/log/log2/exp/pow) or a
#   native cranelift instruction (sqrt/floor/ceil/abs).
# - AOT compiler: re-declared as a `Linkage::Import` cranelift
#   function pointing at the matching libm symbol via
#   `compiler::lower::program::libm_import_name_for`.
#
# The wrapper `pub fn` layer keeps the user-facing surface stable
# (`math::sin(x)`, `math::sqrt(x)`, …) so callers never need to type
# the `__extern_` mangled name themselves.

extern fn __extern_sin_f64(x: f64) -> f64
extern fn __extern_cos_f64(x: f64) -> f64
extern fn __extern_tan_f64(x: f64) -> f64
extern fn __extern_log_f64(x: f64) -> f64
extern fn __extern_log2_f64(x: f64) -> f64
extern fn __extern_exp_f64(x: f64) -> f64
extern fn __extern_floor_f64(x: f64) -> f64
extern fn __extern_ceil_f64(x: f64) -> f64
extern fn __extern_sqrt_f64(x: f64) -> f64
extern fn __extern_abs_f64(x: f64) -> f64
extern fn __extern_pow_f64(base: f64, exp: f64) -> f64

pub fn abs(x: i64) -> i64 {
    # Integer abs stays on the legacy `__builtin_abs` polymorphic
    # intrinsic for now — Phase 5 will move `Abs` / `Min` / `Max`
    # to the same extern-fn machinery as the f64 family.
    __builtin_abs(x)
}

pub fn fabs(x: f64) -> f64 {
    __extern_abs_f64(x)
}

pub fn sqrt(x: f64) -> f64 {
    __extern_sqrt_f64(x)
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
    __extern_pow_f64(base, exp)
}

# f64 transcendentals + rounding — same shape across all entries:
# the body forwards to the corresponding `__extern_*_f64`
# declaration above. The backend dispatch handles the rest.

pub fn sin(x: f64) -> f64 {
    __extern_sin_f64(x)
}

pub fn cos(x: f64) -> f64 {
    __extern_cos_f64(x)
}

pub fn tan(x: f64) -> f64 {
    __extern_tan_f64(x)
}

pub fn log(x: f64) -> f64 {
    __extern_log_f64(x)
}

pub fn log2(x: f64) -> f64 {
    __extern_log2_f64(x)
}

pub fn exp(x: f64) -> f64 {
    __extern_exp_f64(x)
}

pub fn floor(x: f64) -> f64 {
    __extern_floor_f64(x)
}

pub fn ceil(x: f64) -> f64 {
    __extern_ceil_f64(x)
}

fn private_helper() -> u64 {
    42u64
}
