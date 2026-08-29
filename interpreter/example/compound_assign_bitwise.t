# Bitwise compound assignment (`&= |= ^= <<= >>=`).
#
# Like the arithmetic five (`+= -= *= /= %=`), these are parser-level
# desugaring: `x op= rhs` lowers to `x = x op rhs`, so the type checker
# and every backend only ever see an ordinary `Assign` of a `Binary`.
# All four LHS shapes the arithmetic operators accept work here too:
# identifier, field access, tuple access, and index.

struct Flags {
    bits: u64,
}

fn main() -> u64 {
    # Identifier LHS
    var x: u64 = 0x0Cu64
    x &= 0x0Au64      # 0x08
    x |= 0x01u64      # 0x09
    x ^= 0x0Fu64      # 0x06
    x <<= 3u64        # 0x30
    x >>= 2u64        # 0x0C

    # Field LHS
    var f = Flags { bits: 0xF0u64 }
    f.bits >>= 4u64   # 0x0F
    f.bits &= 0x0Cu64 # 0x0C

    # Tuple-access LHS
    var t = (1u64, 2u64)
    t.0 <<= 4u64      # 0x10
    t.1 |= 0x05u64    # 0x07

    # Index LHS
    var arr: [u64; 3] = [1u64, 2u64, 3u64]
    arr[2] ^= 0x01u64 # 0x02

    # Signed shifts keep the sign, and `&=` on a negative left operand
    # works on the two's-complement bits. The shift amount is `u64`
    # regardless of the left operand's type.
    var s: i64 = 0i64 - 8i64
    s >>= 1u64        # -4
    s &= 0i64 - 3i64  # -4 & -3 == -4

    x + f.bits + t.0 + t.1 + arr[2] + ((0i64 - s) as u64)
}
