# Always-loaded prelude. Wired up in `interpreter::lib::integrate_modules`
# (Step E of the extension-trait work). Migrates the legacy
# `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}` numeric methods onto
# extension-trait impls so `x.abs()` / `x.sqrt()` resolve through
# the same `method_registry` user-defined impls go through.
#
# The actual semantic comes from `__extern_*` extern fns; every
# backend already knows how to dispatch those:
#
#   - interpreter:  `evaluation/extern_math::build_default_registry`
#   - JIT:          `jit/eligibility::JIT_EXTERN_DISPATCH`
#   - AOT compiler: `lower/program::libm_import_name_for`
#
# Keeping the implementations behind extern fn (instead of inline
# arithmetic) preserves bit-for-bit equivalence with the legacy
# hardcoded paths — `i64::MIN.abs()` stays at `i64::MIN`
# (`wrapping_abs`), `f64.abs()` is IEEE 754 (sign-bit flip,
# preserves NaN), `f64.sqrt()` returns NaN for negative inputs.

extern fn __extern_abs_i64(x: i64) -> i64
extern fn __extern_abs_f64(x: f64) -> f64
extern fn __extern_sqrt_f64(x: f64) -> f64

trait Abs {
    fn abs(self: Self) -> Self
}

impl Abs for i64 {
    fn abs(self: Self) -> Self {
        __extern_abs_i64(self)
    }
}

impl Abs for f64 {
    fn abs(self: Self) -> Self {
        __extern_abs_f64(self)
    }
}

trait Sqrt {
    fn sqrt(self: Self) -> Self
}

impl Sqrt for f64 {
    fn sqrt(self: Self) -> Self {
        __extern_sqrt_f64(self)
    }
}
