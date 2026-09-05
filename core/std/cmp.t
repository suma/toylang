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

trait Ord {
    fn lt(&self, other: &Self) -> bool
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
# Three-valued underneath so a future `cmp` needs no second extern.
# **Not a collation** -- no locale, no case folding, no accents
# (STDLIB_TEXT §4).
extern fn __extern_str_cmp(a: str, b: str) -> i64 from "toylang_rt" as "toy_str_cmp"

impl Ord for str {
    fn lt(&self, other: &Self) -> bool {
        __extern_str_cmp(self, other) < 0i64
    }
}
