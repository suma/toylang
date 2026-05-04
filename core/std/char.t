# `char` — semantic alias for `u8`. Use this when a value
# represents a single byte that's logically a character (e.g.
# `Vec<u8>::push_char(c: char)`). Numerically identical to `u8`,
# but the name documents intent at signature sites.
#
# Auto-loaded from `<core>/std/char.t -> ["std", "char"]` —
# segments-sort puts this file second, just after
# `["allocator"]`, so every other stdlib module can use `char`
# in its annotations.
type char = u8
