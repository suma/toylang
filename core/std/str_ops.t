# Stdlib traits for byte-buffer / string-like operations.
#
# Auto-loaded from `<core>/std/str_ops.t -> ["std", "str_ops"]`.
#
# `Substring` / `Trim` / `CaseConvert` / `Concat` / `Contains`
# are implemented on `Vec<u8>` (= `String`) in `core/std/string.t`
# so user code can call `.substring(start, end)` / `.trim()` /
# `.to_upper()` / `.to_lower()` / `.concat(other)` /
# `.contains(needle)` against either a `str` literal or a
# heap-allocated `String`. `str` already carries the equivalent
# operations as builtin methods (`BuiltinMethod::StrSubstring` /
# `StrTrim` / `StrToUpper` / `StrToLower` / `StrConcat` /
# `StrContains`) so each receiver shape uses the native fast path:
# `str` goes through the builtin, `Vec<u8>` goes through the
# trait impl.

# `Substring` — half-open byte slice `[start, end)`. Both indices
# are byte offsets, not codepoint counts. Out-of-range / inverted
# ranges panic via `assert(...)`.
pub trait Substring {
    fn substring(&self, start: u64, end: u64) -> Self
}

# `Trim` — strip ASCII whitespace (` `, `\t`, `\n`, `\r`) from
# both ends. Non-ASCII whitespace (e.g. U+00A0) is intentionally
# not recognised — multi-byte UTF-8 awareness lives in a future
# `chars()` iterator phase.
pub trait Trim {
    fn trim(&self) -> Self
}

# `CaseConvert` — ASCII-only case folding. High-bit-set bytes are
# left untouched so multi-byte UTF-8 sequences pass through
# unchanged.
pub trait CaseConvert {
    fn to_upper(&self) -> Self
    fn to_lower(&self) -> Self
}

# `Concat` — append two values of the same shape, returning a new
# value. `other` is taken by `&` reference so user code can pass
# either a `Vec<u8>` (auto-borrowed at the call site) or a borrow
# explicitly.
pub trait Concat<Other> {
    fn concat(&self, other: &Other) -> Self
}

# `Contains` — substring / sub-buffer search. Returns true iff
# `needle` appears in `self` as a contiguous run. Empty `needle`
# matches at position 0 (Rust / libc convention).
pub trait Contains<Needle> {
    fn contains(&self, needle: &Needle) -> bool
}

# `ToString` — convert `self` into an owned `Vec<u8>` (= `String`).
# Idempotent on `Vec<u8>` (returns a fresh copy of the same bytes).
# `str` impl lives alongside the str builtins in `core/std/str.t`.
# Receiver kind matches `Length` / `AsPtr` (`self: Self`) since
# both implementors are cheap to take by value (`str` is a tiny
# pointer + length pair, `Vec<u8>` shares the heap buffer).
pub trait ToString {
    fn to_string(self: Self) -> Vec<u8>
}
