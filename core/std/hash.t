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
# The implementations below are deliberately simple — they're
# correct as identity / parity hashes for an MVP linear-scan
# `Dict`, but a future open-addressing hash table will want a
# real avalanching mixer (Wyhash / FxHash / SipHash). The trait
# signature is the stable contract; the implementations can be
# upgraded later without touching the call sites.

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

# str: placeholder constant. A real implementation needs to walk
# the bytes and mix them, but `BuiltinMethodCall::Len` is not yet
# lowered by the AOT compiler — calling `self.len()` here would
# work in the interpreter but fail at compile time for the AOT
# backend. Returning `0u64` is the worst possible distribution
# but stays correct: the linear-scan `Dict<str, V, A>` falls back
# to `key == probed_key` for actual equality, so every str key
# lands in the same bucket and gets compared one by one. Replace
# with a byte-mixing hash once `__extern_str_hash` (or AOT
# support for `str.len()`) is in place.
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
