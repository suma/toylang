# Demo: call the auto-loaded `math` module via the qualified
# `math::name(...)` form. Same flat function table as before
# (functions get integrated into the main program), but the call
# site spells out which module the function comes from. No
# `import` line — `core/std/math.t` is auto-loaded with `math`
# as the alias derived from the last path segment.
#
# Expected: abs(-30) = 30 → exit 30.

fn main() -> u64 {
    math::abs(-30i64) as u64
}
