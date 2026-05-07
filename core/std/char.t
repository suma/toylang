# `char` — alias for `u32`, holding a Unicode codepoint.
# `'a'` / `'\n'` / `'\u{1F600}'` literals all lex straight to
# `Kind::UInt32(<codepoint>)` (see `frontend/src/lexer.l:585, 597,
# 603`), so a `char`-typed parameter can receive any codepoint
# without truncation.
#
# Use `char` at signature sites where the value is logically a
# single Unicode scalar (e.g. `Vec<u8>::push_char(c: char)`,
# which UTF-8 encodes the codepoint into 1-4 bytes). Use raw `u8`
# when the value really is a byte.
#
# Note: `char` and `u8` are no longer interchangeable. Callers
# passing literal byte values should write `0x41u32` (or `'A'`),
# not `0x41u8`.
#
# Auto-loaded from `<core>/std/char.t -> ["std", "char"]` —
# segments-sort puts this file second, just after
# `["allocator"]`, so every other stdlib module can use `char`
# in its annotations.
type char = u32
