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
# STDLIB-NUMERIC N4: the seven this module was missing.
extern fn __extern_round_f64(x: f64) -> f64
extern fn __extern_trunc_f64(x: f64) -> f64
extern fn __extern_asin_f64(x: f64) -> f64
extern fn __extern_acos_f64(x: f64) -> f64
extern fn __extern_log10_f64(x: f64) -> f64
extern fn __extern_atan2_f64(y: f64, x: f64) -> f64
extern fn __extern_hypot_f64(x: f64, y: f64) -> f64

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


# ---------------------------------------------------------------------
# Integer math (STDLIB-NUMERIC N2).
#
# Overflow **wraps**, like `+` and `*` -- the language has one rule for
# it and this does not add a second. A caller that needs to know asks
# `Checked` (`core/std/checked.t`).

# `base` raised to `exp`, by squaring. `pow(x, 0)` is 1, including for
# `x == 0`.
pub fn pow_u64(base: u64, exp: u32) -> u64 {
    var result: u64 = 1u64
    var b: u64 = base
    var e: u32 = exp
    while e > 0u32 {
        if e % 2u32 == 1u32 { result = result * b }
        b = b * b
        e = e / 2u32
    }
    result
}

pub fn pow_i64(base: i64, exp: u32) -> i64 {
    var result: i64 = 1i64
    var b: i64 = base
    var e: u32 = exp
    while e > 0u32 {
        if e % 2u32 == 1u32 { result = result * b }
        b = b * b
        e = e / 2u32
    }
    result
}

# The greatest common divisor, by Euclid. `gcd(0, 0)` is 0, and
# `gcd(x, 0)` is `x` -- the conventions that make `lcm` come out
# right.
pub fn gcd_u64(a: u64, b: u64) -> u64 {
    var x: u64 = a
    var y: u64 = b
    while y != 0u64 {
        val t: u64 = x % y
        x = y
        y = t
    }
    x
}

# The least common multiple. 0 when either side is 0; wraps like any
# other product.
pub fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0u64 || b == 0u64 { return 0u64 }
    val g: u64 = gcd_u64(a, b)
    (a / g) * b
}

# The integer square root: the largest `r` with `r*r <= x`.
#
# **Not `sqrt(x as f64) as u64`** -- that is off by one above 2^53,
# where an f64 can no longer hold every integer. Newton's method on
# integers is exact everywhere, and the postcondition says so, which
# makes `--check` an oracle for it.
#
# Only the lower bound is stated. `(result + 1) * (result + 1) > x`
# is the other half of "this is the floor", but at the top of the
# range `result + 1` is 2^32 and the square of it is 2^64, which
# wraps to 0 -- the postcondition would be false for a correct
# answer. A contract that cannot be written for every input is worse
# than one that says less.
pub fn isqrt_u64(x: u64) -> u64
    ensures result * result <= x
{
    if x < 2u64 { return x }
    # The first guess comes from the bit length, not from `x` itself.
    # Starting at `x` overflows: for `u64::MAX` the very first
    # `r + x / r` wraps to 0 and the next step divides by it. (Found
    # by `--check`, which is why the postcondition is here.)
    #
    # `2^ceil(bits/2)` is at or above the answer, so the descent below
    # is still correct, and it keeps `r + x / r` under `2r` -- no
    # overflow anywhere.
    val shift: u32 = (64u32 - x.leading_zeros() + 1u32) / 2u32
    var r: u64 = 1u64 << (shift as u64)
    var next: u64 = (r + x / r) / 2u64
    # Newton's method descends monotonically from above, so the first
    # step that does not decrease has passed the answer.
    while next < r {
        r = next
        next = (r + x / r) / 2u64
    }
    r
}

# Division and remainder that round **toward negative infinity**.
#
# The language's `/` and `%` truncate toward zero, so `-7 / 3` is -2
# and `-7 % 3` is -1. Indexing a ring buffer or a hash table wants the
# other convention, and the operators are not changing -- the names
# are how the two are told apart.
pub fn div_floor_i64(a: i64, b: i64) -> i64 {
    val q: i64 = a / b
    if (a % b != 0i64) && ((a < 0i64) != (b < 0i64)) { q - 1i64 } else { q }
}

# Always has `b`'s sign, so `mod_floor(-1, 3)` is 2.
pub fn mod_floor_i64(a: i64, b: i64) -> i64 {
    val r: i64 = a % b
    if r != 0i64 && ((r < 0i64) != (b < 0i64)) { r + b } else { r }
}

pub fn sign_i64(x: i64) -> i64 {
    if x > 0i64 { 1i64 } elif x < 0i64 { -1i64 } else { 0i64 }
}

# The average, without the overflow `(a + b) / 2` has. Rounds down.
pub fn midpoint_u64(a: u64, b: u64) -> u64 {
    if a > b { b + (a - b) / 2u64 } else { a + (b - a) / 2u64 }
}

# ---------------------------------------------------------------------
# `min` / `max` / `clamp` (STDLIB-NUMERIC N3).
#
# Free functions per width rather than methods on `Ord`, because a
# trait method returning `Self` could not be called through a bound
# until STDLIB-TRAIT-BASE B1 -- that hole is closed now, so these can
# move onto `Ord` as default bodies. When they do, **these stay**: the
# call sites should not have to change.
#
# `clamp` panics when the bounds are the wrong way round, rather than
# quietly picking one.

# `min_u64` / `max_u64` / `min_i64` / `max_i64` are declared above --
# they predate this block, and are the shape the rest copies.
pub fn min_u8(a: u8, b: u8) -> u8 { if a < b { a } else { b } }
pub fn max_u8(a: u8, b: u8) -> u8 { if a > b { a } else { b } }
pub fn clamp_u8(x: u8, lo: u8, hi: u8) -> u8 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn min_u16(a: u16, b: u16) -> u16 { if a < b { a } else { b } }
pub fn max_u16(a: u16, b: u16) -> u16 { if a > b { a } else { b } }
pub fn clamp_u16(x: u16, lo: u16, hi: u16) -> u16 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn min_u32(a: u32, b: u32) -> u32 { if a < b { a } else { b } }
pub fn max_u32(a: u32, b: u32) -> u32 { if a > b { a } else { b } }
pub fn clamp_u32(x: u32, lo: u32, hi: u32) -> u32 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn clamp_u64(x: u64, lo: u64, hi: u64) -> u64 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn min_i8(a: i8, b: i8) -> i8 { if a < b { a } else { b } }
pub fn max_i8(a: i8, b: i8) -> i8 { if a > b { a } else { b } }
pub fn clamp_i8(x: i8, lo: i8, hi: i8) -> i8 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn min_i16(a: i16, b: i16) -> i16 { if a < b { a } else { b } }
pub fn max_i16(a: i16, b: i16) -> i16 { if a > b { a } else { b } }
pub fn clamp_i16(x: i16, lo: i16, hi: i16) -> i16 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn min_i32(a: i32, b: i32) -> i32 { if a < b { a } else { b } }
pub fn max_i32(a: i32, b: i32) -> i32 { if a > b { a } else { b } }
pub fn clamp_i32(x: i32, lo: i32, hi: i32) -> i32 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}
pub fn clamp_i64(x: i64, lo: i64, hi: i64) -> i64 {
    assert(lo <= hi, "clamp: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}

# f64 `min` / `max` put NaN **last**, matching IEEE 754's `minNum`:
# `min(NaN, 1.0)` is 1.0. Written out because `<` alone propagates the
# NaN instead.
pub fn min_f64(a: f64, b: f64) -> f64 {
    if a != a { return b }
    if b != b { return a }
    if a < b { a } else { b }
}

pub fn max_f64(a: f64, b: f64) -> f64 {
    if a != a { return b }
    if b != b { return a }
    if a > b { a } else { b }
}

pub fn clamp_f64(x: f64, lo: f64, hi: f64) -> f64 {
    assert(lo <= hi, "clamp_f64: the low bound is above the high one")
    if x < lo { lo } elif x > hi { hi } else { x }
}

# ---------------------------------------------------------------------
# The rest of f64 (STDLIB-NUMERIC N4).

# Halves round away from zero (libm's `round`), not to even. There is
# deliberately no banker's-rounding twin: with two of them a caller
# cannot tell which they got.
pub fn round(x: f64) -> f64 { __extern_round_f64(x) }
pub fn trunc(x: f64) -> f64 { __extern_trunc_f64(x) }
pub fn asin(x: f64) -> f64 { __extern_asin_f64(x) }
pub fn acos(x: f64) -> f64 { __extern_acos_f64(x) }
pub fn log10(x: f64) -> f64 { __extern_log10_f64(x) }

# The angle of `(x, y)` from the positive x axis, in (-pi, pi]. The
# argument order is libm's: `y` first.
pub fn atan2(y: f64, x: f64) -> f64 { __extern_atan2_f64(y, x) }

# `sqrt(x*x + y*y)`, without the overflow that spelling has.
pub fn hypot(x: f64, y: f64) -> f64 { __extern_hypot_f64(x, y) }

# Classification, in pure toylang -- each is one comparison.
#
# A NaN is the only value that is not equal to itself, which is both
# the definition and the test.
pub fn is_nan(x: f64) -> bool { x != x }

pub fn is_infinite(x: f64) -> bool {
    x == limits::f64_inf() || x == limits::f64_neg_inf()
}

pub fn is_finite(x: f64) -> bool {
    if is_nan(x) { false } else { is_infinite(x) == false }
}
