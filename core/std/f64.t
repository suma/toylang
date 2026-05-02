# NOTE: no `package` line — `f64` is a reserved primitive-type
# keyword the parser refuses to accept as a package segment. The
# auto-load integration derives the module path from the file
# system (`core/std/f64.t -> ["std", "f64"]`) so the missing
# `package` declaration costs nothing.
#
# Stdlib extension trait for `f64`. Auto-loaded from `core/std/f64.t`
# alongside `core/std/math.t`, so `x.abs()` / `x.sqrt()` resolve
# through the regular `method_registry` extension-trait dispatch
# path even without an explicit `import` line.
#
# `Abs` shares the trait declaration with `core/std/i64.t` —
# trait declarations are global to the program, not per-module, so
# the second `impl Abs for f64` block here adds an additional
# implementing type without redeclaring the trait. (The auto-load
# integration order is path-sorted, so `i64.t` is integrated
# before `f64.t`, registering `Abs` first.)
#
# Bodies forward to `math::*` so the f64 numeric stdlib has a
# single source of truth for the underlying semantics
# (`math::fabs` is IEEE 754 — preserves NaN, flips the sign bit;
# `math::sqrt` returns NaN for negative inputs).

impl Abs for f64 {
    fn abs(self: Self) -> Self {
        math::fabs(self)
    }
}

trait Sqrt {
    fn sqrt(self: Self) -> Self
}

impl Sqrt for f64 {
    fn sqrt(self: Self) -> Self {
        math::sqrt(self)
    }
}
