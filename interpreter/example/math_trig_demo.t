# Demo: f64 trig + log + exp + floor/ceil through the `math`
# module wrappers.
#
# sin(π/2) = 1, cos(0) = 1, exp(0) = 1, log(e) ≈ 1,
# log2(8) = 3, floor(3.7) = 3, ceil(3.2) = 4. Total = 13.

fn main() -> u64 {
    val pi: f64 = 3.14159265358979f64
    val s: f64 = math::sin(pi / 2f64)
    val c: f64 = math::cos(0f64)
    val e: f64 = math::exp(0f64)
    val l: f64 = math::log(2.718281828f64)
    val l2: f64 = math::log2(8f64)
    val fl: f64 = math::floor(3.7f64)
    val ce: f64 = math::ceil(3.2f64)
    s as u64 + c as u64 + e as u64 + l as u64 + l2 as u64 + fl as u64 + ce as u64
}
