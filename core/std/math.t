package std.math

# Stdlib math module. Auto-loaded from `<core>/std/math.t` so user
# programs can call `math::sin(x)` / `math::sqrt(x)` etc. without
# writing an `import` line. The synthetic `ImportDecl` inserted by
# `interpreter::lib::integrate_modules` registers `math` as the
# qualified-call alias (it derives from the *last* segment of
# `std.math`).
#
# Architecture: every f64 intrinsic delegates to a runtime extern
# fn whose name each backend resolves differently:
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
# `pub fn` wrappers keep the user-facing surface stable
# (`math::sin(x)`, `math::sqrt(x)`, …) so callers never have to type
# the `__extern_` mangled name themselves. `add` / `multiply` are
# convenience exports kept around for the module-integration
# regression tests.

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
extern fn __extern_abs_i64(x: i64) -> i64
extern fn __extern_pow_f64(base: f64, exp: f64) -> f64

pub fn abs(x: i64) -> i64 {
    # Forwards to the runtime `wrapping_abs` helper so `i64::MIN.abs()`
    # stays at `i64::MIN` (matches the legacy `BuiltinMethod::I64Abs`
    # semantics that the extension-trait migration replaced).
    __extern_abs_i64(x)
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

# Note: `math::add` / `math::multiply` are intentionally NOT
# exported. Users name their own `fn add(...)` / `fn multiply(...)`
# over custom types frequently enough that the stdlib should not
# occupy those bare slots, even though #193 / #193b's
# `(module_qualifier, name)` keying would make coexistence safe.
# Use `+` / `*` directly for numeric arithmetic.
