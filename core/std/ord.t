# Stdlib `Ord` trait (STDLIB-ORD): a total-order comparison contract
# for `Vec::sort` and any other generic sorting / ordering code.
#
# Auto-loaded from `<core>/std/ord.t -> ["std", "ord"]`. No `package`
# line — same pattern as `core/std/hash.t`.
#
# Design notes:
#
# - The method is named `lt` — the same name the `<` operator
#   overload dispatches to (`struct_cmp_method_name`). A type that
#   implements `impl Ord` therefore also gets the `<` operator for
#   free (the operator table looks the method up by name, so the
#   receiver style does not matter), and a type with a hand-written
#   `lt` already satisfies the shape.
# - The receiver is `self: Self` (by value) rather than `&self`
#   because primitives cannot be dereferenced in this language — a
#   `&u64` receiver has no way to reach the value it points at. The
#   alias-based compound semantics mean a by-value receiver does not
#   consume the caller's binding (`key.lt(other)` leaves `key`
#   usable), so `Vec::sort` can call it repeatedly.
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
    fn lt(self: Self, other: Self) -> bool
}

# Unsigned widths — native `<`.
impl Ord for u64 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for u32 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for u16 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for u8 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}

# Signed widths — native `<`.
impl Ord for i64 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for i32 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for i16 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}
impl Ord for i8 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}

# f64 — native `<`. NaN compares false in every direction, so it
# never moves during a sort (see the file header).
impl Ord for f64 {
    fn lt(self: Self, other: Self) -> bool {
        self < other
    }
}

# bool — `false < true`, the canonical two-value total order.
impl Ord for bool {
    fn lt(self: Self, other: Self) -> bool {
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
    fn lt(self: Self, other: Self) -> bool {
        __extern_str_cmp(self, other) < 0i64
    }
}
