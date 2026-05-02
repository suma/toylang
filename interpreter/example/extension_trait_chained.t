# #194: chained primitive method calls (`x.neg().neg()`).
# Step C originally restricted the JIT to receiver = bare
# Identifier; this fixture exercises a non-Identifier receiver
# (a MethodCall whose result is itself the receiver of the
# outer MethodCall). The interpreter and JIT must agree.

trait Negate {
    fn neg(self: Self) -> Self
}

impl Negate for i64 {
    fn neg(self: Self) -> Self {
        0i64 - self
    }
}

fn main() -> u64 {
    val a: i64 = 7i64
    # 7.neg().neg() = 7  (chained, no intermediate val)
    val r: i64 = a.neg().neg()
    r as u64
}
