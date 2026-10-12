# Stdlib `Ord` trait (STDLIB-ORD): a total-order comparison contract
# for `Vec::sort` and any other generic sorting / ordering code.
#
# Auto-loaded from `<core>/std/cmp.t -> ["std", "cmp"]`. No `package`
# line — same pattern as `core/std/hash.t`.
#
# Named `cmp` rather than `ord` because a trait module is named for
# the concept, not for the trait (`clone` / `default` / `convert` /
# `hash` / `iter` already are) — see `design-docs/MODULE_SYSTEM.md`
# D2. Rust spells the same module `std::cmp`, C++ `<compare>`.
#
# Design notes:
#
# - The method is named `lt` — the same name the `<` operator
#   overload dispatches to (`struct_cmp_method_name`). A type that
#   implements `impl Ord` therefore also gets the `<` operator for
#   free (the operator table looks the method up by name, so the
#   receiver style does not matter), and a type with a hand-written
#   `lt` already satisfies the shape.
# - The receiver and the argument are both borrowed (`&self`,
#   `other: &Self`). They used to be by value, on the belief that a
#   `&u64` receiver had no way to reach the value it points at; it
#   does -- a reference to a primitive is erased to the value at the
#   boundary, so `self < other` reads exactly as it did. Borrowing is
#   what the `<` operator overload has always been documented to take
#   (`docs/language.md`), and it says the true thing about the method:
#   comparing does not consume either side.
# - `f64` compares with the native `<`; NaN is not less than anything
#   (including itself), so it stays put in a sort rather than
#   ordering — same caveat as comparing floats with `<` directly.
# - The primitive impls are trivial (`self < other`); `String`
#   compares byte-wise in `core/std/string.t` (the module that owns
#   the type, following the convention that nominal-struct impls live
#   with their type). `str` is intentionally not covered — byte-wise
#   comparison needs heap copies that the AOT backend cannot express
#   in a generic context.

# The answer of a three-way comparison (STDLIB-ORD-CMP). The variants
# carry the numbers Rust and C's `memcmp` use, so `o as i64` is -1, 0
# or 1 (ENUM-DISCRIMINANT).
enum Ordering {
    Less = -1,
    Equal = 0,
    Greater = 1,
}

impl Ordering {
    # `==` / `!=` dispatch here (OP-OVERLOAD-ENUM).
    fn eq(&self, other: &Self) -> bool {
        (self as i64) == (other as i64)
    }

    fn is_lt(&self) -> bool { (self as i64) < 0i64 }
    fn is_le(&self) -> bool { (self as i64) <= 0i64 }
    fn is_gt(&self) -> bool { (self as i64) > 0i64 }
    fn is_ge(&self) -> bool { (self as i64) >= 0i64 }
    fn is_eq(&self) -> bool { (self as i64) == 0i64 }
    fn is_ne(&self) -> bool { (self as i64) != 0i64 }

    # `Less` <-> `Greater`; `Equal` stays. Sorting descending is
    # comparing ascending and reversing the answer.
    fn reverse(&self) -> Ordering {
        match self {
            Ordering::Less => Ordering::Greater,
            Ordering::Equal => Ordering::Equal,
            Ordering::Greater => Ordering::Less,
        }
    }

    # Lexicographic chaining: `self` unless it is `Equal`, then `next`.
    # `a.x.cmp(b.x).then(a.y.cmp(b.y))` orders by `x`, ties by `y`.
    # `next` is evaluated eagerly -- there is no lazy form without a
    # closure.
    fn then(&self, next: Ordering) -> Ordering {
        match self {
            Ordering::Equal => next,
            Ordering::Less => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
        }
    }
}

impl Display for Ordering {
    fn to_str(&self) -> str {
        match self {
            Ordering::Less => "Less",
            Ordering::Equal => "Equal",
            Ordering::Greater => "Greater",
        }
    }
}

# `lt` is the one method an impl must write; the rest are derived from
# it (STDLIB-ORD-CMP). Keeping `lt` as the required method -- rather
# than `cmp`, as Rust does -- means every `impl Ord` written before
# `Ordering` existed still conforms, and `lt` is also what the `<`
# operator dispatches to. A type whose comparison is naturally
# three-valued (`str`, below) overrides `cmp` so it is not computed
# from two `lt` calls.
#
# The derived methods are also the operator names: a struct with
# `impl Ord` gets `<=` / `>` / `>=` (`le` / `gt` / `ge`) along with `<`.
trait Ord {
    fn lt(&self, other: &Self) -> bool

    fn cmp(&self, other: &Self) -> Ordering {
        if self.lt(other) {
            Ordering::Less
        } elif other.lt(self) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }

    fn le(&self, other: &Self) -> bool {
        !other.lt(self)
    }

    fn gt(&self, other: &Self) -> bool {
        other.lt(self)
    }

    fn ge(&self, other: &Self) -> bool {
        !self.lt(other)
    }
}

# Generic `min` / `max` / `clamp` over `Ord` (STDLIB-ORD-CMP). These are
# what `math.t`'s per-width `min_u8` ... `clamp_i64` predate; those stay
# so their callers need not change.
#
# Ties follow Rust: `min` returns the first argument, `max` the second,
# so `min(a, b)` and `max(a, b)` together always hand back both values.
#
# Both arguments are taken by value. For an owning `T` (`String`) the
# call frees the ones it does not return (CALLEE-DROP-GENERIC: the body
# only reads a scalar off `b` and lends `a` to `lt`). Compare through
# `cmp` / `lt` and keep both when that matters.
pub fn min<T: Ord>(a: T, b: T) -> T {
    if b.lt(a) { b } else { a }
}

pub fn max<T: Ord>(a: T, b: T) -> T {
    if b.lt(a) { a } else { b }
}

# `x` pulled into `[lo, hi]`. Panics when the bounds are the wrong way
# round rather than quietly picking one (the `math::clamp_*` rule).
pub fn clamp<T: Ord>(x: T, lo: T, hi: T) -> T {
    assert(!hi.lt(lo), "clamp: the low bound is above the high one")
    if x.lt(lo) {
        lo
    } elif hi.lt(x) {
        hi
    } else {
        x
    }
}

# Unsigned widths — native `<`.
impl Ord for u64 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for u32 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for u16 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for u8 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}

# Signed widths — native `<`.
impl Ord for i64 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for i32 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for i16 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}
impl Ord for i8 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}

# f64 — native `<`. NaN compares false in every direction, so it
# never moves during a sort (see the file header).
impl Ord for f64 {
    fn lt(&self, other: &Self) -> bool {
        self < other
    }
}

# bool — `false < true`, the canonical two-value total order.
impl Ord for bool {
    fn lt(&self, other: &Self) -> bool {
        if self { false } else { other }
    }
}

# str — byte order, which over UTF-8 is codepoint order.
#
# The comparison is an extern for the same reason `Hash for str` is
# (`core/std/hash.t`): `str::as_ptr()` allocates on the tree-walker, so
# a byte loop written here would allocate once per comparison and a
# `Vec<str>::sort()` would allocate O(n log n) times.
#
# Three-valued underneath, so `cmp` needs no second extern.
# **Not a collation** -- no locale, no case folding, no accents
# (STDLIB_TEXT §4).
extern fn __extern_str_cmp(a: str, b: str) -> i64 from "toylang_rt" as "toy_str_cmp"

impl Ord for str {
    fn lt(&self, other: &Self) -> bool {
        __extern_str_cmp(self, other) < 0i64
    }

    # One extern call instead of the two `lt`s the default makes.
    fn cmp(&self, other: &Self) -> Ordering {
        val c: i64 = __extern_str_cmp(self, other)
        if c < 0i64 {
            Ordering::Less
        } elif c > 0i64 {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}
