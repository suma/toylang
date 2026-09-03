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

# ---------------------------------------------------------------------
# ASCII classification (STDLIB-TEXT §6).
#
# Impl'd for **both `u8` and `u32`**, the same way `Hash` and `Ord` are
# impl'd for every integer width. That is not redundancy: `String::get`
# hands back a `u8` and `push_char` takes a `u32`, and without both
# impls every call would have to say which one it meant with a cast
# that carries no information.
#
# **Everything outside ASCII answers `false`, and the conversions
# return the value unchanged.** The names say `ascii`, so this is not a
# surprise -- and it is the whole of the language's case handling. A
# Unicode fold needs tens of kilobytes of tables and, for `ß` -> `SS`
# or Turkish `i`, a locale, which the determinism rule rules out
# (STDLIB_TEXT §4).

pub trait AsciiClass {
    fn is_ascii(self: Self) -> bool
    fn is_ascii_digit(self: Self) -> bool
    fn is_ascii_alpha(self: Self) -> bool
    fn is_ascii_alnum(self: Self) -> bool
    # Space, tab, newline, carriage return, form feed, vertical tab --
    # what `isspace` covers in the C locale.
    fn is_ascii_space(self: Self) -> bool
    fn is_ascii_upper(self: Self) -> bool
    fn is_ascii_lower(self: Self) -> bool
    fn to_ascii_upper(self: Self) -> Self
    fn to_ascii_lower(self: Self) -> Self
    # The value of this character as a digit in `radix`, or `None` if
    # it is not one. `radix` is 2..=36; `'f'` and `'F'` are both 15.
    #
    # In the trait because three separate parsers want it -- decimal
    # numbers today, and hex escapes and timestamps later -- and each
    # would otherwise write the same comparison chain.
    fn digit_value(self: Self, radix: u32) -> Option<u32>
}

impl AsciiClass for u8 {
    fn is_ascii(self: Self) -> bool { self < 128u8 }

    fn is_ascii_digit(self: Self) -> bool { self >= '0' && self <= '9' }

    fn is_ascii_alpha(self: Self) -> bool {
        (self >= 'a' && self <= 'z') || (self >= 'A' && self <= 'Z')
    }

    fn is_ascii_alnum(self: Self) -> bool {
        self.is_ascii_digit() || self.is_ascii_alpha()
    }

    fn is_ascii_space(self: Self) -> bool {
        self == ' ' || self == '\t' || self == '\n' || self == '\r'
            || self == '\x0b' || self == '\x0c'
    }

    fn is_ascii_upper(self: Self) -> bool { self >= 'A' && self <= 'Z' }

    fn is_ascii_lower(self: Self) -> bool { self >= 'a' && self <= 'z' }

    fn to_ascii_upper(self: Self) -> Self {
        if self.is_ascii_lower() { self - 0x20u8 } else { self }
    }

    fn to_ascii_lower(self: Self) -> Self {
        if self.is_ascii_upper() { self + 0x20u8 } else { self }
    }

    fn digit_value(self: Self, radix: u32) -> Option<u32> {
        val v: u32 = ascii_digit_value(self as u32, radix)
        if v == 4294967295u32 { Option::None } else { Option::Some(v) }
    }
}

impl AsciiClass for u32 {
    fn is_ascii(self: Self) -> bool { self < 128u32 }

    fn is_ascii_digit(self: Self) -> bool { self >= '0' && self <= '9' }

    fn is_ascii_alpha(self: Self) -> bool {
        (self >= 'a' && self <= 'z') || (self >= 'A' && self <= 'Z')
    }

    fn is_ascii_alnum(self: Self) -> bool {
        self.is_ascii_digit() || self.is_ascii_alpha()
    }

    fn is_ascii_space(self: Self) -> bool {
        self == ' ' || self == '\t' || self == '\n' || self == '\r'
            || self == '\x0b' || self == '\x0c'
    }

    fn is_ascii_upper(self: Self) -> bool { self >= 'A' && self <= 'Z' }

    fn is_ascii_lower(self: Self) -> bool { self >= 'a' && self <= 'z' }

    fn to_ascii_upper(self: Self) -> Self {
        if self.is_ascii_lower() { self - 0x20u32 } else { self }
    }

    fn to_ascii_lower(self: Self) -> Self {
        if self.is_ascii_upper() { self + 0x20u32 } else { self }
    }

    fn digit_value(self: Self, radix: u32) -> Option<u32> {
        val v: u32 = ascii_digit_value(self, radix)
        if v == 4294967295u32 { Option::None } else { Option::Some(v) }
    }
}

# Shared body of the two `digit_value` impls. `u32::MAX` stands in for
# "not a digit" so the arithmetic stays in one place and each impl
# only wraps it in an `Option` -- a trait method cannot yet be shared
# between two impls any other way.
fn ascii_digit_value(c: u32, radix: u32) -> u32 {
    if radix < 2u32 || radix > 36u32 { return 4294967295u32 }
    var v: u32 = 4294967295u32
    if c >= '0' && c <= '9' {
        v = c - '0'
    } elif c >= 'a' && c <= 'z' {
        v = c - 'a' + 10u32
    } elif c >= 'A' && c <= 'Z' {
        v = c - 'A' + 10u32
    }
    if v >= radix { return 4294967295u32 }
    v
}
