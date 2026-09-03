# The extreme values of every numeric type (STDLIB-NUMERIC §4).
#
# **Functions, not constants.** A module's top-level `const` is not
# visible from another module (todo.md MODULE-CONST), so a constant
# here could not be read by the code that needs it -- which is all of
# it. `core/std/poll.t` made `interest_read()` a function for the same
# reason.
#
# The point is that the values are written **once**. `checked.t` used
# to spell `255u8` and `18446744073709551615u64` inline, sixteen
# limits across eight widths, each appearing in three or four
# arithmetic guards -- and there was no other way, because a program
# could not name `u64::MAX` at all.

# ---- unsigned ----------------------------------------------------
pub fn u8_max() -> u8 { 255u8 }
pub fn u8_min() -> u8 { 0u8 }
pub fn u16_max() -> u16 { 65535u16 }
pub fn u16_min() -> u16 { 0u16 }
pub fn u32_max() -> u32 { 4294967295u32 }
pub fn u32_min() -> u32 { 0u32 }
pub fn u64_max() -> u64 { 18446744073709551615u64 }
pub fn u64_min() -> u64 { 0u64 }

# ---- signed ------------------------------------------------------
pub fn i8_max() -> i8 { 127i8 }
pub fn i8_min() -> i8 { -128i8 }
pub fn i16_max() -> i16 { 32767i16 }
pub fn i16_min() -> i16 { -32768i16 }
pub fn i32_max() -> i32 { 2147483647i32 }
pub fn i32_min() -> i32 { -2147483648i32 }
pub fn i64_max() -> i64 { 9223372036854775807i64 }
pub fn i64_min() -> i64 { -9223372036854775808i64 }

# ---- f64 ---------------------------------------------------------
pub fn f64_max() -> f64 { 179769313486231570000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f64 }

# The smallest positive *normal* f64, not the smallest positive value:
# subnormals go lower, and a program comparing against "the smallest
# number" almost always means this one.
pub fn f64_min_positive() -> f64 { 0.000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000022250738585072014f64 }

# The gap between 1.0 and the next representable f64 -- the unit in
# the last place at 1.0, which is what a tolerance is usually written
# against.
pub fn f64_epsilon() -> f64 { 0.0000000000000002220446049250313f64 }

# ---- values with no literal --------------------------------------
#
# These are the only way to *write* an infinity or a NaN. `1f64 / 0f64`
# and `0f64 / 0f64` produce them, but a reader has to work out that
# that is what was meant -- and division by zero is a trap for
# integers, so the spelling reads like a bug.
pub fn f64_inf() -> f64 { 1f64 / 0f64 }
pub fn f64_neg_inf() -> f64 { -1f64 / 0f64 }

# Every NaN compares false against everything, itself included, so
# `x != x` is the test (`math::is_nan`).
pub fn f64_nan() -> f64 { 0f64 / 0f64 }

pub fn f32_max() -> f32 { 340282350000000000000000000000000000000f32 }
pub fn f32_min_positive() -> f32 { 0.00000000000000000000000000000000000001175494f32 }
pub fn f32_epsilon() -> f32 { 0.00000011920929f32 }
pub fn f32_inf() -> f32 { 1f32 / 0f32 }
pub fn f32_neg_inf() -> f32 { -1f32 / 0f32 }
pub fn f32_nan() -> f32 { 0f32 / 0f32 }
