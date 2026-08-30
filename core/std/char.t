# `char` — alias for `u32`, holding a Unicode codepoint.
# `'a'` / `'\n'` / `'\u{1F600}'` literals carry their code point in
# 32 bits (`Kind::CharLiteral`), so a `char`-typed parameter can
# receive any codepoint without truncation. A char literal also
# takes a *narrower* integer type when the position asks for one and
# the value fits (CHAR-LITERAL-NUM) — that is what lets a string's
# `u8` bytes be compared against `'0'` without a cast.
#
# Use `char` at signature sites where the value is logically a
# single Unicode scalar (e.g. `Vec<u8>::push_char(c: char)`,
# which UTF-8 encodes the codepoint into 1-4 bytes). Use raw `u8`
# when the value really is a byte.
#
# Note: `char` and `u8` are not interchangeable as *types* — a `u8`
# variable does not pass for a `char` parameter, and vice versa.
# Only literals cross the line (`'A'` fits both; `0x41u8` does not
# become a `char`).
#
# Auto-loaded from `<core>/std/char.t -> ["std", "char"]` —
# segments-sort puts this file second, just after
# `["allocator"]`, so every other stdlib module can use `char`
# in its annotations.
type char = u32
