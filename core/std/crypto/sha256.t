# SHA-256 and SHA-224 (FIPS 180-4), in toylang. Design and the
# reasoning behind the shape: `design-docs/STDLIB_CRYPTO.md`.
#
# Auto-loaded from `<core>/std/crypto/sha256.t -> ["std", "crypto",
# "sha256"]`, so the one-shot entry points are `sha256::sum(&bytes)`
# and `sha256::sum_224(&bytes)`. The streaming form is the struct
# below; `sum` is written on top of it rather than beside it, so
# there is one compression function to be right about.
#
# API:
#   - `Sha256::new() -> Self` / `Sha256::new_224() -> Self`
#   - `h.update(&data)` (`&mut self`) — feed more input
#   - `h.finalize() -> Sum` (`&mut self`) — pad and produce the digest
#   - `h.output_size()` / `h.block_size()`
#   - `sha256::sum(&data) -> Sum` / `sha256::sum_224(&data) -> Sum`
#
# Two things about the arithmetic are worth knowing before reading
# the rounds:
#
#   - **`+` on `u32` wraps**, which is the addition SHA-2 specifies
#     (`+` and `*` wrap on every backend and build profile, unlike
#     `u64` subtraction which traps). So the round additions are
#     written plainly.
#   - **shifts go through `shr32` / `shl32`**, not `>>` and `<<`. A
#     shift wants both operands to be `u64` today, so a `u32` shift
#     has to widen and come back. `rotate_right` needs no such
#     detour: `Bits` implements it at every width.
#
# The state holds three `Vec`s that are allocated once by `fresh` and
# then only written through: the 64-byte block buffer, the 64-word
# message schedule, and the round constants. Fixed-size arrays would
# suit all three better, but `[0u32; 64]` cannot be spelled and a
# `const` array is refused by the compiled lanes (STDLIB_CRYPTO.md
# "実測 4"), so they are heap buffers and this module does not claim
# `never_allocates`.

pub struct Sha256 {
    # The eight working chain values, named rather than held in a
    # `Vec` so the compression function's adds stay in registers.
    h0: u32,
    h1: u32,
    h2: u32,
    h3: u32,
    h4: u32,
    h5: u32,
    h6: u32,
    h7: u32,
    # Exactly 64 slots, of which the first `nbuf` are pending input.
    buf: Vec<u8>,
    nbuf: u64,
    # The 64-word message schedule, reused by every block.
    w: Vec<u32>,
    # Message bytes fed so far. The padding encodes this times 8.
    total: u64,
    # 32 for SHA-256, 28 for SHA-224. The only difference between the
    # two is this and the initial chain values.
    out: u64,
}

# `x >> n` for a `u32`.
#
# The widen-and-return is a workaround (see the header), but it
# changes one behaviour that the contract has to cover: shifting a
# `u64` by 32 or more is defined and yields 0, where the `u32` shift
# this stands in for is not. Without the clause an out-of-range shift
# would quietly produce a wrong digest instead of stopping.
fn shr32(x: u32, n: u64) -> u32
    requires n < 32u64
{
    ((x as u64) >> n) as u32
}

# `x << n` for a `u32`, truncating back to 32 bits. Same reason for
# the contract as `shr32`.
fn shl32(x: u32, n: u64) -> u32
    requires n < 32u64
{
    ((x as u64) << n) as u32
}

# FIPS 180-4 §4.1.2. `ch` picks bits of `y` where `x` is set and bits
# of `z` where it is not; `maj` takes the majority of the three.
fn ch(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (~x & z) }
fn maj(x: u32, y: u32, z: u32) -> u32 { (x & y) ^ (x & z) ^ (y & z) }

# The four mixing functions. `bsig*` feed the rounds, `ssig*` extend
# the message schedule.
fn bsig0(x: u32) -> u32 {
    x.rotate_right(2u32) ^ x.rotate_right(13u32) ^ x.rotate_right(22u32)
}
fn bsig1(x: u32) -> u32 {
    x.rotate_right(6u32) ^ x.rotate_right(11u32) ^ x.rotate_right(25u32)
}
fn ssig0(x: u32) -> u32 {
    x.rotate_right(7u32) ^ x.rotate_right(18u32) ^ shr32(x, 3u64)
}
fn ssig1(x: u32) -> u32 {
    x.rotate_right(17u32) ^ x.rotate_right(19u32) ^ shr32(x, 10u64)
}

impl Sha256 {
    # SHA-256: FIPS 180-4 §5.3.3.
    fn new() -> Self {
        # Bound rather than returned directly: the compiled lanes
        # refuse a compound-returning associated call in expression
        # position.
        val h: Sha256 = Sha256::fresh(
            0x6a09e667u32, 0xbb67ae85u32, 0x3c6ef372u32, 0xa54ff53au32,
            0x510e527fu32, 0x9b05688cu32, 0x1f83d9abu32, 0x5be0cd19u32,
            32u64,
        )
        h
    }

    # SHA-224: FIPS 180-4 §5.3.2. Same compression function, a
    # different starting chain, and the last 32 bits of the result
    # dropped.
    fn new_224() -> Self {
        val h: Sha256 = Sha256::fresh(
            0xc1059ed8u32, 0x367cd507u32, 0x3070dd17u32, 0xf70e5939u32,
            0xffc00b31u32, 0x68581511u32, 0x64f98fa7u32, 0xbefa4fa4u32,
            28u64,
        )
        h
    }

    # The shared constructor: initial chain values in, sized buffers
    # out.
    #
    # `out` is contracted rather than trusted because it reaches
    # `finalize` as a byte count and nothing downstream would notice a
    # third value -- the digest would simply be the wrong length, and
    # a truncated SHA-256 is not a hash anyone specified.
    fn fresh(
        a: u32, b: u32, c: u32, d: u32,
        e: u32, f: u32, g: u32, h: u32,
        out: u64,
    ) -> Self
        requires out == 32u64 || out == 28u64
    {
        if out != 32u64 && out != 28u64 { panic("Sha256::fresh: bad output size {out}") }
        var buf: Vec<u8> = Vec::new()
        var i = 0u64
        while i < 64u64 { buf.push(0u8)  i = i + 1u64 }
        var w: Vec<u32> = Vec::new()
        i = 0u64
        while i < 64u64 { w.push(0u32)  i = i + 1u64 }
        Sha256 {
            h0: a, h1: b, h2: c, h3: d, h4: e, h5: f, h6: g, h7: h,
            buf: buf, nbuf: 0u64, w: w,
            total: 0u64, out: out,
        }
    }

    # Append one byte of *padding* -- not of message, so `total` is
    # left alone -- and run a block whenever 64 have accumulated.
    #
    # `finalize` is the only caller. Message bytes go through
    # `update`, which also has to advance `total`.
    #
    # The clause is the buffer invariant, stated where it is about to
    # be relied on: a block is run and `nbuf` reset the moment it
    # reaches 64, so on entry there is always room for one more byte.
    fn pad_byte(&mut self, b: u8)
        requires self.nbuf < 64u64
    {
        self.buf.set(self.nbuf, b)
        self.nbuf = self.nbuf + 1u64
        if self.nbuf == 64u64 {
            self.compress()
            self.nbuf = 0u64
        }
    }

    # One application of the compression function to the 64 bytes in
    # `buf`. FIPS 180-4 §6.2.2.
    #
    # The contract states what the body assumes about the buffer it is
    # about to read whole. It is a class invariant more than a
    # precondition -- `fresh` sizes `buf` to 64 and nothing shrinks it
    # -- but the read happens 64 times per block, and this is the one
    # place where a future change to how `buf` is managed would go
    # wrong silently rather than loudly.
    never_allocates fn compress(&mut self)
        requires self.buf.size() == 64u64
    {
        # Words 0..15 are the block, read big-endian.
        var t = 0u64
        while t < 16u64 {
            val b0 = self.buf.get(t * 4u64) as u32
            val b1 = self.buf.get(t * 4u64 + 1u64) as u32
            val b2 = self.buf.get(t * 4u64 + 2u64) as u32
            val b3 = self.buf.get(t * 4u64 + 3u64) as u32
            self.w.set(t, shl32(b0, 24u64) | shl32(b1, 16u64) | shl32(b2, 8u64) | b3)
            t = t + 1u64
        }
        # Words 16..63 are derived from four earlier ones.
        while t < 64u64 {
            val v = ssig1(self.w.get(t - 2u64)) + self.w.get(t - 7u64)
                  + ssig0(self.w.get(t - 15u64)) + self.w.get(t - 16u64)
            self.w.set(t, v)
            t = t + 1u64
        }

        var a = self.h0
        var b = self.h1
        var c = self.h2
        var d = self.h3
        var e = self.h4
        var f = self.h5
        var g = self.h6
        var h = self.h7
        t = 0u64
        while t < 64u64 {
            val t1 = h + bsig1(e) + ch(e, f, g) + SHA256_K[t] + self.w.get(t)
            val t2 = bsig0(a) + maj(a, b, c)
            h = g
            g = f
            f = e
            e = d + t1
            d = c
            c = b
            b = a
            a = t1 + t2
            t = t + 1u64
        }
        self.h0 = self.h0 + a
        self.h1 = self.h1 + b
        self.h2 = self.h2 + c
        self.h3 = self.h3 + d
        self.h4 = self.h4 + e
        self.h5 = self.h5 + f
        self.h6 = self.h6 + g
        self.h7 = self.h7 + h
    }

    # Append one chain value to the digest, most significant byte
    # first.
    fn emit(&self, out: &mut Vec<u8>, v: u32) {
        out.push(shr32(v, 24u64) as u8)
        out.push((shr32(v, 16u64) & 0xFFu32) as u8)
        out.push((shr32(v, 8u64) & 0xFFu32) as u8)
        out.push((v & 0xFFu32) as u8)
    }
}

impl Digest for Sha256 {
    fn update(&mut self, data: &Vec<u8>) {
        var i = 0u64
        while i < data.size() {
            self.buf.set(self.nbuf, data.get(i))
            self.nbuf = self.nbuf + 1u64
            self.total = self.total + 1u64
            if self.nbuf == 64u64 {
                self.compress()
                self.nbuf = 0u64
            }
            i = i + 1u64
        }
    }

    fn finalize(&mut self) -> Sum {
        # FIPS 180-4 §5.1.1: a 1 bit, then zeros up to 56 bytes into
        # the last block, then the message length in bits as a 64-bit
        # big-endian integer. `pad_byte` runs a block whenever the
        # buffer fills, so the case where the length does not fit in
        # the current block needs no special handling here.
        val bits = self.total * 8u64
        self.pad_byte(0x80u8)
        while self.nbuf != 56u64 { self.pad_byte(0u8) }
        var j = 0u64
        while j < 8u64 {
            self.pad_byte(((bits >> ((7u64 - j) * 8u64)) & 0xFFu64) as u8)
            j = j + 1u64
        }

        var out: Vec<u8> = Vec::new()
        self.emit(&mut out, self.h0)
        self.emit(&mut out, self.h1)
        self.emit(&mut out, self.h2)
        self.emit(&mut out, self.h3)
        self.emit(&mut out, self.h4)
        self.emit(&mut out, self.h5)
        self.emit(&mut out, self.h6)
        # SHA-224 stops here: its digest is the first seven chain
        # values, so the eighth is emitted only for SHA-256.
        if self.out == 32u64 { self.emit(&mut out, self.h7) }
        val s = Sum::from_bytes(out)
        s
    }

    fn output_size(&self) -> u64 { self.out }
    fn block_size(&self) -> u64 { 64u64 }
}

# The 64 round constants, K[0..63] (FIPS 180-4 §4.2.2): the first 32
# bits of the fractional parts of the cube roots of the first 64
# primes.
#
# A `const` array, so it is 256 bytes of read-only data that every
# hasher reads and none of them builds. It used to be a `Vec` filled
# by 64 `push`es per hasher, because the compiled lanes refused a
# `const` whose initialiser was an array (CONST-ARRAY, fixed
# 2026-09-21) — which is what kept `compress` from being
# `never_allocates`.
const SHA256_K: [u32; 64] = [
    0x428a2f98u32, 0x71374491u32, 0xb5c0fbcfu32, 0xe9b5dba5u32,
    0x3956c25bu32, 0x59f111f1u32, 0x923f82a4u32, 0xab1c5ed5u32,
    0xd807aa98u32, 0x12835b01u32, 0x243185beu32, 0x550c7dc3u32,
    0x72be5d74u32, 0x80deb1feu32, 0x9bdc06a7u32, 0xc19bf174u32,
    0xe49b69c1u32, 0xefbe4786u32, 0x0fc19dc6u32, 0x240ca1ccu32,
    0x2de92c6fu32, 0x4a7484aau32, 0x5cb0a9dcu32, 0x76f988dau32,
    0x983e5152u32, 0xa831c66du32, 0xb00327c8u32, 0xbf597fc7u32,
    0xc6e00bf3u32, 0xd5a79147u32, 0x06ca6351u32, 0x14292967u32,
    0x27b70a85u32, 0x2e1b2138u32, 0x4d2c6dfcu32, 0x53380d13u32,
    0x650a7354u32, 0x766a0abbu32, 0x81c2c92eu32, 0x92722c85u32,
    0xa2bfe8a1u32, 0xa81a664bu32, 0xc24b8b70u32, 0xc76c51a3u32,
    0xd192e819u32, 0xd6990624u32, 0xf40e3585u32, 0x106aa070u32,
    0x19a4c116u32, 0x1e376c08u32, 0x2748774cu32, 0x34b0bcb5u32,
    0x391c0cb3u32, 0x4ed8aa4au32, 0x5b9cca4fu32, 0x682e6ff3u32,
    0x748f82eeu32, 0x78a5636fu32, 0x84c87814u32, 0x8cc70208u32,
    0x90befffau32, 0xa4506cebu32, 0xbef9a3f7u32, 0xc67178f2u32,
]

# SHA-256 of one buffer. The digest is 32 bytes.
#
# The length wants to be an `ensures` -- `Sum` carries it at run time,
# so prose is the only other place it can live -- but a contract that
# reaches into a compound `result` is refused by the compiled lanes
# (STDLIB_CRYPTO.md "実測 5"). `Sha256::fresh` contracts the same fact
# on the way in instead, where the value is still a scalar.
pub fn sum(data: &Vec<u8>) -> Sum {
    var h = Sha256::new()
    h.update(data)
    val d = h.finalize()
    d
}

# SHA-224 of one buffer.
pub fn sum_224(data: &Vec<u8>) -> Sum {
    var h = Sha256::new_224()
    h.update(data)
    val d = h.finalize()
    d
}
