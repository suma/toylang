# Stdlib traits for byte-buffer / string-like operations.
#
# Auto-loaded from `<core>/std/str_ops.t -> ["std", "str_ops"]`.
#
# `Substring` / `Trim` / `CaseConvert` / `Concat` / `Contains` /
# `Split` are implemented on `String` in `core/std/string.t` so
# user code can call `.substring(start, end)` / `.trim()` /
# `.to_upper()` / `.to_lower()` / `.concat(other)` /
# `.contains(needle)` / `.split(sep)` against either a `str`
# literal or a heap-allocated `String`. `str` already carries
# the equivalent operations as builtin methods
# (`BuiltinMethod::StrSubstring` / `StrTrim` / `StrToUpper` /
# `StrToLower` / `StrConcat` / `StrContains`) so each receiver
# shape uses the native fast path: `str` goes through the
# builtin, `String` goes through the trait impl.
#
# `ToString` is intentionally NOT a trait — its non-`Self` return
# type (`String`) tripped the frontend's trait-conformance
# canonicalisation in mixed `Identifier(String)` /
# `Struct(String, [])` shapes. Each type provides `to_string` as
# an inherent method instead (`String::to_string` does an
# identity-clone; `str` users construct owned `String`s via
# `String::from_str(s)`).

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
# either a `String` (auto-borrowed at the call site) or a borrow
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

# `Split` — split `self` at every occurrence of `sep`, returning
# a vector of slices (each slice itself is a `String`). Empty
# `sep` panics — the libc / Rust convention of "every codepoint
# boundary" doesn't apply at the byte level. `Out` is generic so
# future receivers can pick their own container shape (e.g. an
# iterator type) without changing the trait surface.
pub trait Split<Sep, Out> {
    fn split(&self, sep: &Sep) -> Out
}
