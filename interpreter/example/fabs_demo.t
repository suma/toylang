# f64.abs() (= C's `fabs`) — IEEE 754 absolute value. Works as both
# a value method (`x.abs()`) and as the polymorphic `__builtin_abs`
# intrinsic. Same `__builtin_abs` symbol handles i64 by dispatching
# on the operand type.
#
# Expected: |-5| + (|-2.5| * 4) = 5 + 10 = 15 → exit 15.
fn main() -> u64 {
    val x: f64 = -5f64
    val y: f64 = -2.5f64
    x.abs() as u64 + (y.abs() * 4f64) as u64
}
