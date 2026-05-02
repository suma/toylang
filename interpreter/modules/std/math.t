package std.math

# A multi-segment stdlib example. Forwards to the same `__builtin_*`
# intrinsics as the single-segment `math` module — but via a deeper
# package path so users can write `import std.math; std::math::abs(x)`.

pub fn abs(x: i64) -> i64 {
    __builtin_abs(x)
}

pub fn sqrt(x: f64) -> f64 {
    __builtin_sqrt(x)
}

pub fn min_i64(a: i64, b: i64) -> i64 {
    __builtin_min(a, b)
}

pub fn max_i64(a: i64, b: i64) -> i64 {
    __builtin_max(a, b)
}
