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
# The traits are split per type rather than written once over `Self`
# for a reason that no longer holds: `Option<Self>` used to be
# rejected as a method return type by the AOT / JIT lanes, so naming
# the concrete payload was the only way to keep all three backends on
# one path. Since 2026-08-31 a single `trait Checked { fn checked_add(
# self: Self, other: Self) -> Option<Self> }` with one impl per width
# lowers on every backend, and narrow receivers dispatch too. Merging
# the two traits and covering u8..i32 is now a stdlib edit with no
# compiler work behind it (todo: RUNTIME-TRAP-NARROW). Widths other
# than 64-bit are still not covered.

trait CheckedU64 {
    fn checked_add(self: Self, other: Self) -> Option<u64>
    fn checked_sub(self: Self, other: Self) -> Option<u64>
    fn checked_mul(self: Self, other: Self) -> Option<u64>
    fn checked_div(self: Self, other: Self) -> Option<u64>
    fn saturating_add(self: Self, other: Self) -> Self
    fn saturating_sub(self: Self, other: Self) -> Self
    fn saturating_mul(self: Self, other: Self) -> Self
}

impl CheckedU64 for u64 {
    fn checked_add(self: Self, other: Self) -> Option<u64> {
        # `MAX - other` cannot underflow: `other` is itself a u64.
        if self > 18446744073709551615u64 - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<u64> {
        if self < other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<u64> {
        if other == 0u64 {
            Option::Some(0u64)
        } elif self > 18446744073709551615u64 / other {
            Option::None
        } else {
            Option::Some(self * other)
        }
    }

    fn checked_div(self: Self, other: Self) -> Option<u64> {
        if other == 0u64 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if self > 18446744073709551615u64 - other {
            18446744073709551615u64
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
        } elif self > 18446744073709551615u64 / other {
            18446744073709551615u64
        } else {
            self * other
        }
    }
}

trait CheckedI64 {
    fn checked_add(self: Self, other: Self) -> Option<i64>
    fn checked_sub(self: Self, other: Self) -> Option<i64>
    fn checked_mul(self: Self, other: Self) -> Option<i64>
    fn checked_div(self: Self, other: Self) -> Option<i64>
    fn saturating_add(self: Self, other: Self) -> Self
    fn saturating_sub(self: Self, other: Self) -> Self
}

impl CheckedI64 for i64 {
    fn checked_add(self: Self, other: Self) -> Option<i64> {
        # Each bound is computed on the side that cannot overflow:
        # `MAX - other` only when `other` is positive, `MIN - other`
        # only when it is negative.
        if other > 0i64 && self > 9223372036854775807i64 - other {
            Option::None
        } elif other < 0i64 && self < -9223372036854775808i64 - other {
            Option::None
        } else {
            Option::Some(self + other)
        }
    }

    fn checked_sub(self: Self, other: Self) -> Option<i64> {
        if other < 0i64 && self > 9223372036854775807i64 + other {
            Option::None
        } elif other > 0i64 && self < -9223372036854775808i64 + other {
            Option::None
        } else {
            Option::Some(self - other)
        }
    }

    fn checked_mul(self: Self, other: Self) -> Option<i64> {
        # `MIN * -1` is the one product the division test below
        # cannot check, because the check itself would divide `MIN`
        # by `-1` and trap.
        if other == 0i64 {
            Option::Some(0i64)
        } elif self == -9223372036854775808i64 && other == -1i64 {
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

    fn checked_div(self: Self, other: Self) -> Option<i64> {
        if other == 0i64 {
            Option::None
        } elif self == -9223372036854775808i64 && other == -1i64 {
            Option::None
        } else {
            Option::Some(self / other)
        }
    }

    fn saturating_add(self: Self, other: Self) -> Self {
        if other > 0i64 && self > 9223372036854775807i64 - other {
            9223372036854775807i64
        } elif other < 0i64 && self < -9223372036854775808i64 - other {
            -9223372036854775808i64
        } else {
            self + other
        }
    }

    fn saturating_sub(self: Self, other: Self) -> Self {
        if other < 0i64 && self > 9223372036854775807i64 + other {
            9223372036854775807i64
        } elif other > 0i64 && self < -9223372036854775808i64 + other {
            -9223372036854775808i64
        } else {
            self - other
        }
    }
}
