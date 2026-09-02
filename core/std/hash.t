# NOTE: no `package` line. Following the same pattern as
# `core/std/i64.t` / `core/std/f64.t`, this file's package path
# would be `std.hash`, but we leave the declaration off so the
# auto-load integration derives the path from the file system
# (`core/std/hash.t -> ["std", "hash"]`).
#
# Stdlib `Hash` extension trait, auto-loaded from
# `<core>/std/hash.t`. Provides a primitive-keyed `hash(self) -> u64`
# operation that user code (and the `Dict<K, V, A>` in
# `core/std/dict.t`) can dispatch through the regular
# extension-trait method-registry path.
#
# The implementations below are deliberately simple — identity for
# the unsigned widths, a same-width cast for the signed ones. They
# do not avalanche, and they are not meant to: an open-addressing
# table reading the low bits of an identity hash would cluster
# badly, so the mixing belongs on the *table* side, where it also
# covers hashes that user code wrote (`design-docs/COLLECTIONS.md`
# 1.3). The trait signature is the stable contract: `hash` promises
# only that equal values hash equally; the distribution is the
# table's problem.

trait Hash {
    fn hash(self: Self) -> u64
}

# i64: reinterpret the 64 bits as u64. Negative values become
# large unsigned values (two's complement). Fine for equality-
# based linear search; would be poor for power-of-two table
# sizing without a mixer.
impl Hash for i64 {
    fn hash(self: Self) -> u64 {
        self as u64
    }
}

# u64: identity. Same caveat about avalanching as i64::hash.
impl Hash for u64 {
    fn hash(self: Self) -> u64 {
        self
    }
}

# bool: just the discriminant. Two-bucket distribution is fine
# for the linear-scan dict (we only need `eq` to break ties).
impl Hash for bool {
    fn hash(self: Self) -> u64 {
        if self { 1u64 } else { 0u64 }
    }
}

# str: placeholder constant. Returning `0u64` is the worst possible
# distribution but stays correct: the linear-scan `Dict<str, V>`
# falls back to `key == probed_key` for actual equality, so every
# str key lands in the same bucket and gets compared one by one.
#
# The reason this file used to give — that the AOT backend cannot
# lower `str.len()` — no longer holds; `s.len()` compiles on all
# three backends. What blocks a toylang byte walk now is cost:
# `str::as_ptr()` heap-allocates a copy of the bytes on every call
# in the interpreter (see `core/std/str.t`), and a hash runs once
# per lookup, so the walk would trade a linear scan for an
# allocation per probe. The replacement is an extern
# (`__extern_str_hash`) — phase C0 of
# `design-docs/COLLECTIONS.md`.
impl Hash for str {
    fn hash(self: Self) -> u64 {
        0u64
    }
}

# NUM-W narrow integer Hash impls.
#
# Unsigned widths cast straight to u64 — same pattern as
# `Hash for u64` (identity).
#
# Signed widths first cast through the matching unsigned
# width to avoid sign extension. `(-5_i8) as u64` would
# sign-extend to 0xFF…FB (a huge unsigned value); routing
# through u8 first gives `0xFB = 251`, which preserves the
# 8 bits of information the value actually carries and
# behaves the way a hash table's "the same byte hashes
# the same way" intuition expects. Same trick for i16 → u16
# → u64 and i32 → u32 → u64. Equality fallback in
# `Dict<K,V>` uses `==` on the original signed value, so
# correctness is preserved.
#
# Caveat: still not avalanching — a real open-addressing
# table will want a Wyhash / FxHash mixer on top.
impl Hash for u8 {
    fn hash(self: Self) -> u64 {
        self as u64
    }
}
impl Hash for u16 {
    fn hash(self: Self) -> u64 {
        self as u64
    }
}
impl Hash for u32 {
    fn hash(self: Self) -> u64 {
        self as u64
    }
}
impl Hash for i8 {
    fn hash(self: Self) -> u64 {
        (self as u8) as u64
    }
}
impl Hash for i16 {
    fn hash(self: Self) -> u64 {
        (self as u16) as u64
    }
}
impl Hash for i32 {
    fn hash(self: Self) -> u64 {
        (self as u32) as u64
    }
}
