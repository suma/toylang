# NOTE: no `package` line. The file's package path would be
# `std.num`, which the parser would accept — but every other core
# module omits the declaration and lets the auto-load integration
# derive the path from the file system location
# (`core/std/num.t -> ["std", "num"]`), so this one does too.
#
# Stdlib extension traits for the numeric primitives. Auto-loaded
# alongside `core/std/math.t`, so `n.abs()` / `x.sqrt()` resolve
# through the regular `method_registry` extension-trait dispatch
# path even without an explicit `import` line.
#
# **One file for the trait and every impl of it.** Until 2026-09-04
# `trait Abs` was declared in `core/std/i64.t` and one of its two
# impls lived in `core/std/f64.t`, which made the pair depend on the
# auto-load integration order (path-sorted, so `f64` came before
# `i64`) — renaming either file would have moved an `impl` ahead of
# its own `trait`. Declaring both traits next to their impls removes
# the ordering dependency; see `design-docs/MODULE_SYSTEM.md` P1.
#
# The bodies delegate to `math::*`, which forwards to the runtime
# `__extern_*` helpers (backend-specific dispatch tables wire up to
# `wrapping_abs` semantics — `i64::MIN.abs()` stays at `i64::MIN`;
# `math::fabs` is IEEE 754, preserving NaN and flipping the sign
# bit; `math::sqrt` returns NaN for negative inputs). Routing
# through the `math` wrappers keeps one source of truth for the
# actual semantics.

trait Abs {
    fn abs(self: Self) -> Self
}

impl Abs for i64 {
    fn abs(self: Self) -> Self {
        math::abs(self)
    }
}

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
