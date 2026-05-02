package std.math

# A multi-segment stdlib example. The f64 intrinsics now route
# through `extern fn` declarations (the same pattern used by the
# single-segment `math` module); abs / min / max stay on the
# legacy `__builtin_*` polymorphic intrinsics until Phase 5
# moves them onto the same extern-fn machinery.

extern fn __extern_sqrt_f64(x: f64) -> f64

pub fn abs(x: i64) -> i64 {
    __builtin_abs(x)
}

pub fn sqrt(x: f64) -> f64 {
    __extern_sqrt_f64(x)
}

pub fn min_i64(a: i64, b: i64) -> i64 {
    __builtin_min(a, b)
}

pub fn max_i64(a: i64, b: i64) -> i64 {
    __builtin_max(a, b)
}
