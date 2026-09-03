package std.checked

# Overflow-aware integer arithmetic (RUNTIME-TRAP).
#
# The operators themselves wrap: `a + b` on `u64::MAX` gives a
# wrapped result on every backend, deliberately, so that one
# arithmetic semantics holds regardless of build profile. Two cases
# are exceptions and stop the program instead, because their wrapped
# answer is a plausible-looking number that surfaces far from the
# mistake: `u64` subtraction below zero, and integer `/` or `%` by
# zero (plus the signed `MIN / -1` whose result is not
# representable). See `docs/language.md` -> "Numeric semantics".
#
# This module is the escape hatch for the wrapping cases: ask for the
# result as an `Option`, or ask for it clamped.
#
#     val r = a.checked_add(b)
#     match r {
#         Option::Some(v) => v,
#         Option::None => 0u64,
#     }
#
#     val c = a.saturating_sub(b)   # 0u64 rather than a huge number
#
# Note the `val` binding before the `match`: an enum returned
# directly by a call cannot be a `match` scrutinee in the compiled
# backends yet (see `docs/language.md` -> "Known limitations").
#
# One trait over `Self`, one impl per width. It used to be two traits
# naming `u64` / `i64` concretely, because `Option<Self>` as a method
# return type did not survive lowering and narrow receivers were
# missing from the dispatch tables; both were fixed on 2026-08-31, so
# the widths below are a stdlib edit with no compiler work behind
# them (RUNTIME-TRAP-NARROW).
#
# The bodies are mechanical per width — the same seven bodies with
# the width's own `MAX` / `MIN` literals substituted — so read one
# unsigned impl and one signed impl and the rest follow. Two of them
# are worth knowing:
#
#   - unsigned `checked_mul` divides `MAX` by `other` rather than
#     multiplying, so the test itself cannot overflow;
#   - signed `checked_mul` multiplies and divides back, with
#     `MIN * -1` taken out first because that division would trap.
#
# Narrow widths differ from `u64` in one place worth naming: `a - b`
# below zero **wraps** on `u8` / `u16` / `u32` where the same
# expression on `u64` traps (`docs/language.md` -> "Runtime traps"
# lists the trap for `u64` only). `checked_sub` / `saturating_sub`
# therefore report a narrow underflow that the operator would have
# silently wrapped past.

trait Checked {
    fn checked_add(self: Self, other: Self) -> Option<Self>
    fn checked_sub(self: Self, other: Self) -> Option<Self>
    fn checked_mul(self: Self, other: Self) -> Option<Self>
    fn checked_div(self: Self, other: Self) -> Option<Self>
    fn saturating_add(self: Self, other: Self) -> Self
    fn saturating_sub(self: Self, other: Self) -> Self
    fn saturating_mul(self: Self, other: Self) -> Self
}


impl Checked for u8 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # `MAX - other` cannot underflow: `other` is itself a u8.
        if self > limits::u8_max() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if self < other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        if other == 0u8 {
            Option::Some(0u8)
        } elif self > limits::u8_max() / other {
            Option::None
        } else {
            Option::Some(self * other)
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0u8 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if self > limits::u8_max() - other {
            limits::u8_max()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if self < other {
            0u8
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        if other == 0u8 {
            0u8
        } elif self > limits::u8_max() / other {
            limits::u8_max()
        } else {
            self * other
        }
    }
}

impl Checked for u16 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # `MAX - other` cannot underflow: `other` is itself a u16.
        if self > limits::u16_max() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if self < other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        if other == 0u16 {
            Option::Some(0u16)
        } elif self > limits::u16_max() / other {
            Option::None
        } else {
            Option::Some(self * other)
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0u16 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if self > limits::u16_max() - other {
            limits::u16_max()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if self < other {
            0u16
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        if other == 0u16 {
            0u16
        } elif self > limits::u16_max() / other {
            limits::u16_max()
        } else {
            self * other
        }
    }
}

impl Checked for u32 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # `MAX - other` cannot underflow: `other` is itself a u32.
        if self > limits::u32_max() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if self < other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        if other == 0u32 {
            Option::Some(0u32)
        } elif self > limits::u32_max() / other {
            Option::None
        } else {
            Option::Some(self * other)
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0u32 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if self > limits::u32_max() - other {
            limits::u32_max()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if self < other {
            0u32
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        if other == 0u32 {
            0u32
        } elif self > limits::u32_max() / other {
            limits::u32_max()
        } else {
            self * other
        }
    }
}

impl Checked for u64 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # `MAX - other` cannot underflow: `other` is itself a u64.
        if self > limits::u64_max() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if self < other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        if other == 0u64 {
            Option::Some(0u64)
        } elif self > limits::u64_max() / other {
            Option::None
        } else {
            Option::Some(self * other)
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0u64 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if self > limits::u64_max() - other {
            limits::u64_max()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if self < other {
            0u64
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        if other == 0u64 {
            0u64
        } elif self > limits::u64_max() / other {
            limits::u64_max()
        } else {
            self * other
        }
    }
}

impl Checked for i8 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # Each bound is computed on the side that cannot overflow:
        # `MAX - other` only when `other` is positive, `MIN - other`
        # only when it is negative.
        if other > 0i8 && self > limits::i8_max() - other {
            Option::None
        } elif other < 0i8 && self < limits::i8_min() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if other < 0i8 && self > limits::i8_max() + other {
            Option::None
        } elif other > 0i8 && self < limits::i8_min() + other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        # `MIN * -1` is the one product the division test below
        # cannot check, because the check itself would divide `MIN`
        # by `-1` and trap.
        if other == 0i8 {
            Option::Some(0i8)
        } elif self == limits::i8_min() && other == -1i8 {
            Option::None
        } else {
            val product = self * other
            if product / other == self {
                Option::Some(product)
            } else {
                Option::None
            }
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0i8 {
            Option::None
        } elif self == limits::i8_min() && other == -1i8 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if other > 0i8 && self > limits::i8_max() - other {
            limits::i8_max()
        } elif other < 0i8 && self < limits::i8_min() - other {
            limits::i8_min()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if other < 0i8 && self > limits::i8_max() + other {
            limits::i8_max()
        } elif other > 0i8 && self < limits::i8_min() + other {
            limits::i8_min()
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        # Neither operand is zero past the first arm, so the sign of
        # the true product decides which bound an overflow clamps to.
        if other == 0i8 {
            0i8
        } elif self == limits::i8_min() && other == -1i8 {
            limits::i8_max()
        } else {
            val product = self * other
            if product / other == self {
                product
            } elif (self < 0i8 && other < 0i8) || (self > 0i8 && other > 0i8) {
                limits::i8_max()
            } else {
                limits::i8_min()
            }
        }
    }
}

impl Checked for i16 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # Each bound is computed on the side that cannot overflow:
        # `MAX - other` only when `other` is positive, `MIN - other`
        # only when it is negative.
        if other > 0i16 && self > limits::i16_max() - other {
            Option::None
        } elif other < 0i16 && self < limits::i16_min() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if other < 0i16 && self > limits::i16_max() + other {
            Option::None
        } elif other > 0i16 && self < limits::i16_min() + other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        # `MIN * -1` is the one product the division test below
        # cannot check, because the check itself would divide `MIN`
        # by `-1` and trap.
        if other == 0i16 {
            Option::Some(0i16)
        } elif self == limits::i16_min() && other == -1i16 {
            Option::None
        } else {
            val product = self * other
            if product / other == self {
                Option::Some(product)
            } else {
                Option::None
            }
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0i16 {
            Option::None
        } elif self == limits::i16_min() && other == -1i16 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if other > 0i16 && self > limits::i16_max() - other {
            limits::i16_max()
        } elif other < 0i16 && self < limits::i16_min() - other {
            limits::i16_min()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if other < 0i16 && self > limits::i16_max() + other {
            limits::i16_max()
        } elif other > 0i16 && self < limits::i16_min() + other {
            limits::i16_min()
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        # Neither operand is zero past the first arm, so the sign of
        # the true product decides which bound an overflow clamps to.
        if other == 0i16 {
            0i16
        } elif self == limits::i16_min() && other == -1i16 {
            limits::i16_max()
        } else {
            val product = self * other
            if product / other == self {
                product
            } elif (self < 0i16 && other < 0i16) || (self > 0i16 && other > 0i16) {
                limits::i16_max()
            } else {
                limits::i16_min()
            }
        }
    }
}

impl Checked for i32 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # Each bound is computed on the side that cannot overflow:
        # `MAX - other` only when `other` is positive, `MIN - other`
        # only when it is negative.
        if other > 0i32 && self > limits::i32_max() - other {
            Option::None
        } elif other < 0i32 && self < limits::i32_min() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if other < 0i32 && self > limits::i32_max() + other {
            Option::None
        } elif other > 0i32 && self < limits::i32_min() + other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        # `MIN * -1` is the one product the division test below
        # cannot check, because the check itself would divide `MIN`
        # by `-1` and trap.
        if other == 0i32 {
            Option::Some(0i32)
        } elif self == limits::i32_min() && other == -1i32 {
            Option::None
        } else {
            val product = self * other
            if product / other == self {
                Option::Some(product)
            } else {
                Option::None
            }
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0i32 {
            Option::None
        } elif self == limits::i32_min() && other == -1i32 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if other > 0i32 && self > limits::i32_max() - other {
            limits::i32_max()
        } elif other < 0i32 && self < limits::i32_min() - other {
            limits::i32_min()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if other < 0i32 && self > limits::i32_max() + other {
            limits::i32_max()
        } elif other > 0i32 && self < limits::i32_min() + other {
            limits::i32_min()
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        # Neither operand is zero past the first arm, so the sign of
        # the true product decides which bound an overflow clamps to.
        if other == 0i32 {
            0i32
        } elif self == limits::i32_min() && other == -1i32 {
            limits::i32_max()
        } else {
            val product = self * other
            if product / other == self {
                product
            } elif (self < 0i32 && other < 0i32) || (self > 0i32 && other > 0i32) {
                limits::i32_max()
            } else {
                limits::i32_min()
            }
        }
    }
}

impl Checked for i64 {
    fn checked_add(self: Self, other: Self) -> Option<Self> {
        # Each bound is computed on the side that cannot overflow:
        # `MAX - other` only when `other` is positive, `MIN - other`
        # only when it is negative.
        if other > 0i64 && self > limits::i64_max() - other {
            Option::None
        } elif other < 0i64 && self < limits::i64_min() - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<Self> {
        if other < 0i64 && self > limits::i64_max() + other {
            Option::None
        } elif other > 0i64 && self < limits::i64_min() + other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<Self> {
        # `MIN * -1` is the one product the division test below
        # cannot check, because the check itself would divide `MIN`
        # by `-1` and trap.
        if other == 0i64 {
            Option::Some(0i64)
        } elif self == limits::i64_min() && other == -1i64 {
            Option::None
        } else {
            val product = self * other
            if product / other == self {
                Option::Some(product)
            } else {
                Option::None
            }
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<Self> {
        if other == 0i64 {
            Option::None
        } elif self == limits::i64_min() && other == -1i64 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if other > 0i64 && self > limits::i64_max() - other {
            limits::i64_max()
        } elif other < 0i64 && self < limits::i64_min() - other {
            limits::i64_min()
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if other < 0i64 && self > limits::i64_max() + other {
            limits::i64_max()
        } elif other > 0i64 && self < limits::i64_min() + other {
            limits::i64_min()
        } else {
            self - other
        }
    }

    fn saturating_mul(self: Self, other: Self) -> Self {
        # Neither operand is zero past the first arm, so the sign of
        # the true product decides which bound an overflow clamps to.
        if other == 0i64 {
            0i64
        } elif self == limits::i64_min() && other == -1i64 {
            limits::i64_max()
        } else {
            val product = self * other
            if product / other == self {
                product
            } elif (self < 0i64 && other < 0i64) || (self > 0i64 && other > 0i64) {
                limits::i64_max()
            } else {
                limits::i64_min()
            }
        }
    }
}

