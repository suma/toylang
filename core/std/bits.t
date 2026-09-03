# Bit operations (STDLIB-NUMERIC N1).
#
# **Five externs, not seventy-two.** Nine operations across eight
# widths would be 72 boundary crossings to declare and keep in step;
# instead the primitives work on `u64` and each width corrects the
# answer with a shift. `u8::leading_zeros` is `clz(x as u64) - 56`.
#
# They are externs rather than a toylang loop because the loop is
# ~400 µs per call on the interpreter against ~6.7 µs for a crossing
# -- sixty times worse -- and externs rather than IR instructions
# because an extern is one implementation shared by four lanes while
# an IR instruction is three. Cranelift does have `popcnt` and `clz`
# as single instructions, so an AOT build pays a ~5 ns call it need
# not; that is a trade to revisit with a measurement, the way
# SIMD-VM-SLOT decided, rather than up front.
#
# **Zero is defined**, not undefined as it is in the hardware:
# `leading_zeros(0)` and `trailing_zeros(0)` are the width.
#
# Signed widths answer about the bit pattern, not the value:
# `(-1i8).popcount()` is 8.

extern fn __extern_bits_popcount(x: u64) -> u32 from "toylang_rt" as "toy_bits_popcount"
extern fn __extern_bits_clz(x: u64) -> u32 from "toylang_rt" as "toy_bits_clz"
extern fn __extern_bits_ctz(x: u64) -> u32 from "toylang_rt" as "toy_bits_ctz"
extern fn __extern_bits_reverse(x: u64) -> u64 from "toylang_rt" as "toy_bits_reverse"
extern fn __extern_bits_swap_bytes(x: u64) -> u64 from "toylang_rt" as "toy_bits_swap_bytes"

pub trait Bits {
    fn popcount(self: Self) -> u32
    # The width when `self` is 0, not undefined.
    fn leading_zeros(self: Self) -> u32
    fn trailing_zeros(self: Self) -> u32
    fn rotate_left(self: Self, n: u32) -> Self
    fn rotate_right(self: Self, n: u32) -> Self
    fn reverse_bits(self: Self) -> Self
    # A no-op at 8 bits; the identity is kept so generic code need not
    # special-case the width.
    fn swap_bytes(self: Self) -> Self
    # False for 0: zero is not a power of two.
    fn is_power_of_two(self: Self) -> bool
    # 1 for an input of 0. Panics rather than wrapping when the answer
    # does not fit -- the same treatment an out-of-range index gets.
    fn next_power_of_two(self: Self) -> Self
}


impl Bits for u8 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount(self as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz(self as u64) - 56u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz(self as u64)
        if z > 8u32 { 8u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 8u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v << k) | (v >> (8u64 - k))) as u8) as u8
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 8u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v >> k) | (v << (8u64 - k))) as u8) as u8
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse(self as u64) >> 56u64) as u8) as u8 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes(self as u64) >> 56u64) as u8) as u8 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = self as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = self as u64
        if v <= 1u64 { return 1u8 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 8u32 { panic("u8::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u8) as u8
    }
}

impl Bits for u16 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount(self as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz(self as u64) - 48u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz(self as u64)
        if z > 16u32 { 16u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 16u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v << k) | (v >> (16u64 - k))) as u16) as u16
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 16u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v >> k) | (v << (16u64 - k))) as u16) as u16
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse(self as u64) >> 48u64) as u16) as u16 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes(self as u64) >> 48u64) as u16) as u16 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = self as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = self as u64
        if v <= 1u64 { return 1u16 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 16u32 { panic("u16::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u16) as u16
    }
}

impl Bits for u32 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount(self as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz(self as u64) - 32u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz(self as u64)
        if z > 32u32 { 32u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 32u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v << k) | (v >> (32u64 - k))) as u32) as u32
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 32u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v >> k) | (v << (32u64 - k))) as u32) as u32
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse(self as u64) >> 32u64) as u32) as u32 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes(self as u64) >> 32u64) as u32) as u32 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = self as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = self as u64
        if v <= 1u64 { return 1u32 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 32u32 { panic("u32::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u32) as u32
    }
}

impl Bits for u64 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount(self as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz(self as u64) }
    fn trailing_zeros(self: Self) -> u32 { __extern_bits_ctz(self as u64) }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 64u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v << k) | (v >> (64u64 - k))) as u64) as u64
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 64u32) as u64
        if k == 0u64 { return self }
        val v: u64 = self as u64
        (((v >> k) | (v << (64u64 - k))) as u64) as u64
    }
    fn reverse_bits(self: Self) -> Self { (__extern_bits_reverse(self as u64) as u64) as u64 }
    fn swap_bytes(self: Self) -> Self { (__extern_bits_swap_bytes(self as u64) as u64) as u64 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = self as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = self as u64
        if v <= 1u64 { return 1u64 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 64u32 { panic("u64::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u64) as u64
    }
}

impl Bits for i8 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount((self as u8) as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz((self as u8) as u64) - 56u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz((self as u8) as u64)
        if z > 8u32 { 8u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 8u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u8) as u64
        (((v << k) | (v >> (8u64 - k))) as u8) as i8
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 8u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u8) as u64
        (((v >> k) | (v << (8u64 - k))) as u8) as i8
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse((self as u8) as u64) >> 56u64) as u8) as i8 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes((self as u8) as u64) >> 56u64) as u8) as i8 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = (self as u8) as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = (self as u8) as u64
        if v <= 1u64 { return 1i8 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 8u32 { panic("i8::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u8) as i8
    }
}

impl Bits for i16 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount((self as u16) as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz((self as u16) as u64) - 48u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz((self as u16) as u64)
        if z > 16u32 { 16u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 16u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u16) as u64
        (((v << k) | (v >> (16u64 - k))) as u16) as i16
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 16u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u16) as u64
        (((v >> k) | (v << (16u64 - k))) as u16) as i16
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse((self as u16) as u64) >> 48u64) as u16) as i16 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes((self as u16) as u64) >> 48u64) as u16) as i16 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = (self as u16) as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = (self as u16) as u64
        if v <= 1u64 { return 1i16 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 16u32 { panic("i16::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u16) as i16
    }
}

impl Bits for i32 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount((self as u32) as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz((self as u32) as u64) - 32u32 }
    fn trailing_zeros(self: Self) -> u32 {
        val z: u32 = __extern_bits_ctz((self as u32) as u64)
        if z > 32u32 { 32u32 } else { z }
    }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 32u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u32) as u64
        (((v << k) | (v >> (32u64 - k))) as u32) as i32
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 32u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u32) as u64
        (((v >> k) | (v << (32u64 - k))) as u32) as i32
    }
    fn reverse_bits(self: Self) -> Self { ((__extern_bits_reverse((self as u32) as u64) >> 32u64) as u32) as i32 }
    fn swap_bytes(self: Self) -> Self { ((__extern_bits_swap_bytes((self as u32) as u64) >> 32u64) as u32) as i32 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = (self as u32) as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = (self as u32) as u64
        if v <= 1u64 { return 1i32 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 32u32 { panic("i32::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u32) as i32
    }
}

impl Bits for i64 {
    fn popcount(self: Self) -> u32 { __extern_bits_popcount((self as u64) as u64) }
    fn leading_zeros(self: Self) -> u32 { __extern_bits_clz((self as u64) as u64) }
    fn trailing_zeros(self: Self) -> u32 { __extern_bits_ctz((self as u64) as u64) }
    fn rotate_left(self: Self, n: u32) -> Self {
        val k: u64 = (n % 64u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u64) as u64
        (((v << k) | (v >> (64u64 - k))) as u64) as i64
    }
    fn rotate_right(self: Self, n: u32) -> Self {
        val k: u64 = (n % 64u32) as u64
        if k == 0u64 { return self }
        val v: u64 = (self as u64) as u64
        (((v >> k) | (v << (64u64 - k))) as u64) as i64
    }
    fn reverse_bits(self: Self) -> Self { (__extern_bits_reverse((self as u64) as u64) as u64) as i64 }
    fn swap_bytes(self: Self) -> Self { (__extern_bits_swap_bytes((self as u64) as u64) as u64) as i64 }
    fn is_power_of_two(self: Self) -> bool {
        val v: u64 = (self as u64) as u64
        if v == 0u64 { false } else { __extern_bits_popcount(v) == 1u32 }
    }
    fn next_power_of_two(self: Self) -> Self {
        val v: u64 = (self as u64) as u64
        if v <= 1u64 { return 1i64 }
        val bits: u32 = 64u32 - __extern_bits_clz(v - 1u64)
        if bits >= 64u32 { panic("i64::next_power_of_two: the answer does not fit") }
        ((1u64 << (bits as u64)) as u64) as i64
    }
}
