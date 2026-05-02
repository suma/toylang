# Demo: import the `math` module and call its `pub fn add` via the
# qualified `math::add(...)` form. Same flat function table as before
# (functions get integrated into the main program), but the call site
# spells out which module the function comes from.
#
# Expected: 10 + 20 = 30 → exit 30.

fn main() -> u64 {
    math::add(10u64, 20u64)
}
