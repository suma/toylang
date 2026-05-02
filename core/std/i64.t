# NOTE: no `package` line. The file's package path would be
# `std.i64`, but `i64` is a reserved primitive-type keyword the
# parser refuses to accept as a package segment. The auto-load
# integration derives the module path from the *file system*
# location (`core/std/i64.t -> ["std", "i64"]`) independently of
# any in-file `package` declaration, so dropping it costs nothing.
#
# Stdlib extension trait for `i64`. Auto-loaded from `core/std/i64.t`
# alongside `core/std/math.t`, so `n.abs()` resolves through the
# regular `method_registry` extension-trait dispatch path even
# without an explicit `import` line.
#
# The body delegates to `math::abs`, which in turn forwards to the
# runtime `__extern_abs_i64` helper (backend-specific dispatch
# tables wire up to `wrapping_abs` semantics — `i64::MIN.abs()`
# stays at `i64::MIN`). Routing through the `math` wrapper keeps
# the call shape symmetrical with the f64 side and gives `math::*`
# a single source of truth for the actual semantic.

trait Abs {
    fn abs(self: Self) -> Self
}

impl Abs for i64 {
    fn abs(self: Self) -> Self {
        math::abs(self)
    }
}
