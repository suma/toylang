# NOTE: no `package` line. Following the same pattern as
# `core/std/str.t`, this file's package path
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

# The avalanching step an open-addressing table applies to whatever
# `hash` returned before it takes the low bits as a slot index
# (`design-docs/COLLECTIONS.md` 1.3). It lives here, next to the
# trait, rather than inside each impl, so that a hash written by user
# code gets the same treatment as the ones below: `hash` is only
# asked to promise that equal values hash equally.
#
# Named `hash_mix` rather than `mix`: stdlib functions are auto-loaded
# into the same namespace user code writes in, and a name this short
# collides with an ordinary program's own (`fn mix(a, b)` in the FFI
# tests did). The prefix is the same reason `dict.t` spells its
# reserved slot value `dict_slot_empty`.
#
# splitmix64's finalizer — three rounds of xor-shift-multiply, which
# spreads every input bit over the whole word. The multiplications
# wrap (`*` wraps on every backend and build profile), which is the
# arithmetic this mixer wants.
pub fn hash_mix(h: u64) -> u64 {
    var x: u64 = h
    x = (x ^ (x >> 30u64)) * 0xBF58476D1CE4E5B9u64
    x = (x ^ (x >> 27u64)) * 0x94D049BB133111EBu64
    x = x ^ (x >> 31u64)
    x
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

# str: FNV-1a over the UTF-8 bytes, in the runtime rather than in
# toylang. The byte walk is the same three lines in either language,
# but `str::as_ptr()` heap-allocates a copy of the bytes on every
# call in the interpreter (see `core/std/str.t`) and a hash runs once
# per lookup, so a toylang walk would trade a table scan for an
# allocation per probe. RUNTIME-PORT R3/R4 measured the same shape
# for the other str helpers.
#
# The three backends must agree on the value, so the algorithm is
# pinned rather than delegated: `toylang_rt::toy_str_hash` and the
# interpreter's `__extern_str_hash` implement the same FNV-1a, and
# `impl Hash for String` (`core/std/string.t`) walks its bytes with
# the same constants so that a `String` and the `str` holding the
# same text hash alike. No seed: a per-process seed would make the
# iteration order of a future hash table differ run to run, which
# the determinism rules in `docs/language.md` rule out.
extern fn __extern_str_hash(s: str) -> u64 from "toylang_rt" as "toy_str_hash"

impl Hash for str {
    fn hash(self: Self) -> u64 {
        __extern_str_hash(self)
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
