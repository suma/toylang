# Stdlib cryptographic digests — the shared interface and the value
# a hash produces. Design: `design-docs/STDLIB_CRYPTO.md`.
#
# Auto-loaded from `<core>/std/crypto/digest.t -> ["std", "crypto",
# "digest"]`, so the free functions here are reached as
# `digest::ct_eq(...)`. No `package` line, matching every other
# stdlib file (the auto-load integration derives the path from the
# file system).
#
# The concrete algorithms live beside this file: `crypto/sha256.t`
# today, `sha512.t` / `hmac.t` / `sha1.t` / `md5.t` later. They all
# hand back the same `Sum`.

# A finished digest.
#
# A nominal struct rather than a bare `Vec<u8>`, because both the
# input to a hash and its output are byte buffers: with `Vec<u8>` on
# both sides, `sha256::sum(&digest)` (hashing a hash) and
# `sha256::sum(&message)` are indistinguishable to the checker. `Sum`
# costs one wrapper and buys that mistake back.
#
# What it does *not* distinguish is which algorithm produced it. A
# trait cannot vary its return type per implementation (no associated
# types), so `Digest::finalize` returns one type for every algorithm
# and the length lives at run time. `Sum` separates "a digest" from
# "some bytes", not SHA-256's 32 from SHA-512's 64.
pub struct Sum {
    bytes: Vec<u8>,
}

impl Sum {
    # Adopt bytes that a hash just produced.
    #
    # A zero-length digest is refused: every algorithm here has a
    # fixed, non-zero output size, so an empty `Sum` can only come
    # from a caller that lost the bytes on the way, and comparing
    # against one would succeed against anything of the same length.
    fn from_bytes(b: Vec<u8>) -> Self
        requires b.size() > 0u64
    {
        if b.size() == 0u64 { panic("Sum::from_bytes: empty digest") }
        Sum { bytes: b }
    }

    fn size(&self) -> u64 { self.bytes.size() }

    # One byte of the digest, big-endian as the standards print it:
    # index 0 is the byte that `to_hex` writes first.
    fn get(&self, i: u64) -> u8
        requires i < self.size()
    {
        if i >= self.bytes.size() { panic("Sum::get: index {i} of {self.bytes.size()}") }
        self.bytes.get(i)
    }

    # The lowercase hex spelling every one of these standards uses to
    # print a digest. Delegated to `hex::encode` on purpose: the hex
    # alphabet and case are already decided in `core/std/hex.t`, and a
    # second spelling of the same thing is a second thing to keep
    # right.
    fn to_hex(&self) -> String {
        # Both bindings are worked around, not stylistic: the
        # compiled lanes refuse a compound-returning module call in
        # expression position, and they refuse `&self.field` as the
        # argument of one (a plain name is fine -- a compound `val`
        # aliases, so `b` is the same buffer).
        val b = self.bytes
        val h: String = hex::encode(&b)
        h
    }

    # `println(sum)` prints the hex, which is what a digest is for.
    fn to_str(&self) -> str {
        val h = self.to_hex()
        "{h}"
    }

    # Ordinary equality: stops at the first byte that differs.
    #
    # Fine for "is this the file I already have". **Not** for checking
    # a MAC -- use `digest::ct_eq` there, and read its comment about
    # what it can and cannot promise.
    fn eq(&self, other: &Self) -> bool {
        if self.bytes.size() != other.bytes.size() { return false }
        var i = 0u64
        while i < self.bytes.size() {
            if self.bytes.get(i) != other.bytes.get(i) { return false }
            i = i + 1u64
        }
        true
    }
}

# A hash that takes its input in pieces.
#
# Streaming is the primitive and the one-shot `sum` functions are
# built on it, for two reasons: a caller hashing a file need not hold
# it all in memory, and HMAC runs the inner hash twice over different
# prefixes.
#
# **This trait carries no `requires` clauses**, and not by choice: a
# contract on a body-less trait method mis-binds its expression and
# reports a type error against an unrelated method (STDLIB_CRYPTO.md
# "実測 2"). DBC-LISKOV then forbids an implementation from adding one
# of its own (`[E0023]`). So the contracts live on the inherent
# methods of each hasher, which is where they have something to say
# anyway.
pub trait Digest {
    # Feed more input. Any number of calls; the split between them
    # does not change the result.
    fn update(&mut self, data: &Vec<u8>)
    # Finish, and produce the digest. The state is spent afterwards --
    # `update` past this point is not defined.
    fn finalize(&mut self) -> Sum
    # Bytes the finished digest occupies.
    fn output_size(&self) -> u64
        ensures result > 0u64
    # Bytes the compression function consumes at a time. HMAC needs
    # this to size its key padding, which is the reason it is on the
    # trait rather than on each hasher.
    fn block_size(&self) -> u64
        ensures result > 0u64
}

# Compare two digests without letting the time taken say how far the
# match got.
#
# Ordinary `==` returns as soon as two bytes differ, so an attacker
# who can time the check learns the digest one byte at a time. This
# folds every byte into one accumulator and only looks at the result
# at the end. The length is mixed in the same way rather than
# short-circuiting on it.
#
# **This is an intent, not a guarantee.** The language has no
# optimisation barrier, so nothing stops a backend from turning the
# fold back into a branch. Treat it as "the version that was written
# to be constant time", and do not build anything on the assumption
# that it is one.
pub fn ct_eq(a: &Sum, b: &Sum) -> bool {
    # Difference in length is itself a difference; fold it in rather
    # than returning early, and keep reading `min` bytes so the loop
    # count depends only on lengths, never on content.
    var diff: u64 = a.size() ^ b.size()
    var n = a.size()
    if b.size() < n { n = b.size() }
    var i = 0u64
    while i < n {
        diff = diff | ((a.get(i) ^ b.get(i)) as u64)
        i = i + 1u64
    }
    diff == 0u64
}
