# `Display` — how a value renders as text.
#
# Auto-loaded from `<core>/std/display.t -> ["std", "display"]`.
#
# A type that implements this controls what `print` / `println` write
# and what string interpolation splices in:
#
#     struct Point { x: i64, y: i64 }
#     impl Display for Point {
#         fn to_str(&self) -> str { "({self.x}, {self.y})" }
#     }
#
#     println(p)          # (1, 2)
#     println("at {p}")   # at (1, 2)
#
# Without an impl, a struct or enum renders structurally
# (`Point { x: 1, y: 2 }`) — useful for debugging, rarely what you
# want in output a person reads.
#
# **Dispatch is on the method, not on the impl.** The type checker
# rewrites `println(v)` to `println(v.to_str())` whenever `v`'s type has
# a `to_str` method, so an inherent `fn to_str(&self) -> str` works just
# as well as an `impl Display for`. This matches how `==` finds `eq` and
# `+` finds `add`; the trait is here so the contract has a name, so
# `--api` shows it, and so `<T: Display>` can be written.
#
# **Why `to_str` and not `to_string`.** `str` and `String` are different
# types here, and `String::to_string() -> String` already exists as
# Rust's idempotent clone. Interpolation splices `str`, so that is what
# this returns; the name says which of the two you get.
#
# A `to_str` that renders its own type — directly, or through
# interpolation of `self` — recurses forever. That is the
# implementation's business, the same as in any language with a
# user-defined `Display`.
pub trait Display {
    fn to_str(&self) -> str
}
