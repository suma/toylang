# Stdlib traits for byte-buffer / string-like operations.
#
# Auto-loaded from `<core>/std/str_ops.t -> ["std", "str_ops"]`.
#
# `Substring` / `Trim` / `CaseConvert` are implemented on
# `Vec<u8>` (= `String`) in `core/std/string.t` so user code can
# call `s.substring(start, end)` / `s.trim()` / `s.to_upper()` /
# `s.to_lower()` against either a `str` literal or a heap-allocated
# `String`. `str` already carries the equivalent operations as
# builtin methods (`BuiltinMethod::StrSubstring` / `StrTrim` /
# `StrToUpper` / `StrToLower`) so each receiver shape uses the
# native fast path: `str` goes through the builtin, `Vec<u8>`
# goes through the trait impl.
#
# Deferred (await follow-up): `Concat<Other>` / `Contains<Needle>`
# both need AOT-lower support for trait methods that take a struct
# (or `&struct`) argument — today the lower bails with "method
# argument produced no value" for that shape (fine on interpreter
# / JIT).

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
