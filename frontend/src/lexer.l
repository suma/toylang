use crate::token::{Token, Kind, StringPart};

macro_rules! token {
    ($self: ident, $kind: expr) => {
        Token { kind: $kind, position: $self.yybytepos() }
    }
}

/// STR-INTERP-FMT: split one `{...}` interpolation segment into its
/// expression text and optional format spec at the first depth-0 `:`.
///
/// Brackets of every kind raise the depth, so field colons inside a
/// struct or dict literal stay part of the expression. A `::` pair is
/// consumed whole — a path (`Color::Red`, `String::from_str(s)`) has
/// no spec. Returns `(expr, None)` when no separator is found.
fn split_format_spec(segment: &str) -> (&str, Option<&str>) {
    let bytes = segment.as_bytes();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => {
                if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    i += 2;
                    continue;
                }
                return (&segment[..i], Some(&segment[i + 1..]));
            }
            _ => {}
        }
        i += 1;
    }
    (segment, None)
}

/// One ASCII hex digit `0-9 a-f A-F` -> nibble value `0..=15`.
/// Used by both the `'\xHH'` char literal rule and the `\xHH`
/// escape inside string literals.
#[allow(dead_code)]
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Why `yylex` failed. rflex's generated `Error::Unmatch` carries no
/// reason, so every failing rule action records the reason in
/// `last_lex_error` first; the parser's token source reads it back to
/// build a diagnostic that points at the offending literal. The
/// fallback [`LexErrorKind::UnmatchedChar`] (a character no rule
/// matches) is recorded by the token source when the field is `None` —
/// no action ran, so nothing could set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorKind {
    /// `\x` escape whose byte is not valid UTF-8 on its own (`\x80`).
    NonAsciiHexByte { byte: u8 },
    /// `\x` / `\u` escape with missing or non-hex digits.
    MalformedHexEscape,
    /// An escape sequence the string / char rules do not know (`\q`).
    UnknownEscape { byte: u8 },
    /// A string literal whose `{...}` interpolation never closes.
    UnterminatedInterpolation,
    /// A string literal with no closing `"` before end of input.
    UnterminatedString,
    /// `\u{HEX}` naming a value outside the Unicode scalar range.
    InvalidCodePoint,
    /// A string literal ending with a bare `\` (which would escape the
    /// closing quote — `\"` is not decodable yet).
    DanglingEscape,
    /// A float literal whose text cannot be parsed as `f64`.
    MalformedNumber,
    /// `[0-9][A-Za-z_0-9]*` — digits followed by identifier chars.
    BadNumber,
    /// No rule matched the character (rflex's fallback). Constructed
    /// by the parser's token source, which is a different include of
    /// this file — hence the `allow`.
    #[allow(dead_code)]
    UnmatchedChar,
}

impl LexErrorKind {
    /// Render the failure as a diagnostic message, quoting the
    /// offending source text (`text` is the whole failing token —
    /// usually a literal including its quotes).
    #[allow(dead_code)]
    pub fn describe(self, text: &str) -> String {
        match self {
            LexErrorKind::NonAsciiHexByte { byte } => format!(
                "`\\x{byte:02X}` cannot be decoded inside a string literal: `str` is UTF-8, \
                 and a lone byte >= 0x80 has no representation — use `\\u{{HEX}}` for non-ASCII \
                 code points"
            ),
            LexErrorKind::MalformedHexEscape => format!(
                "malformed escape in `{text}`: `\\x` needs exactly two hex digits and \
                 `\\u` needs `{{HEX}}`"
            ),
            LexErrorKind::UnknownEscape { byte } => format!(
                "unknown escape sequence `\\{}` in `{text}`",
                byte as char
            ),
            LexErrorKind::UnterminatedInterpolation => format!(
                "unterminated interpolation in `{text}`: the `{{...}}` inside a string \
                 literal never closes"
            ),
            LexErrorKind::UnterminatedString => format!(
                "unterminated string literal: `{text}` has no closing `\"`"
            ),
            LexErrorKind::InvalidCodePoint => format!(
                "invalid Unicode code point in `{text}`: `\\u{{HEX}}` must name a scalar \
                 value (max U+10FFFF, no surrogates)"
            ),
            LexErrorKind::DanglingEscape => format!(
                "`{text}` ends with a bare backslash: `\\\"` (an escaped quote) is not \
                 decodable inside a string literal yet — use `\\u{{22}}` for the quote \
                 character"
            ),
            LexErrorKind::MalformedNumber => format!("invalid number literal `{text}`"),
            LexErrorKind::BadNumber => {
                format!("invalid number `{text}`: digits cannot be followed by letters")
            }
            LexErrorKind::UnmatchedChar => format!("unrecognized character `{text}`"),
        }
    }
}

/// Parse the body of a `\u{HEX}` Unicode escape. `bytes[start]`
/// must point at the `{`. Returns `(code_point, end_index)`
/// where `end_index` is the index *after* the closing `}`.
/// Errors on missing `{` / `}`, no digits, non-hex, overflow,
/// or out-of-range / surrogate code points.
#[allow(dead_code)]
fn parse_unicode_escape_body(bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    if start >= bytes.len() || bytes[start] != b'{' {
        return None;
    }
    let mut i = start + 1;
    let mut value: u32 = 0;
    let mut digits = 0;
    while i < bytes.len() && bytes[i] != b'}' {
        let nib = hex_digit(bytes[i])?;
        // Cap at 6 hex digits — anything beyond can't fit in a
        // valid Unicode scalar (max is 0x10FFFF).
        if digits >= 6 {
            return None;
        }
        value = (value << 4) | (nib as u32);
        digits += 1;
        i += 1;
    }
    if digits == 0 || i >= bytes.len() {
        return None;
    }
    // i points at `}`; consume it.
    Some((value, i + 1))
}

%%
%class Lexer
%result_type Token
%field u64 line_count
%field Option<LexErrorKind> last_lex_error

"if"        return Ok(token!(self, Kind::If));
"elif"      return Ok(token!(self, Kind::Elif));
"else"      return Ok(token!(self, Kind::Else));
"for"       return Ok(token!(self, Kind::For));
"in"        return Ok(token!(self, Kind::In));
"to"        return Ok(token!(self, Kind::To));
"while"     return Ok(token!(self, Kind::While));
"loop"      return Ok(token!(self, Kind::Loop));
"break"     return Ok(token!(self, Kind::Break));
"continue"  return Ok(token!(self, Kind::Continue));
"return"    return Ok(token!(self, Kind::Return));

"class"     return Ok(token!(self, Kind::Class));
"struct"    return Ok(token!(self, Kind::Struct));
"trait"     return Ok(token!(self, Kind::Trait));
"impl"      return Ok(token!(self, Kind::Impl));
"dyn"       return Ok(token!(self, Kind::Dyn));
"fn"        return Ok(token!(self, Kind::Function));
"extern"    return Ok(token!(self, Kind::Extern));
"pub"       return Ok(token!(self, Kind::Public));
"package"   return Ok(token!(self, Kind::Package));
"import"    return Ok(token!(self, Kind::Import));
"as"        return Ok(token!(self, Kind::As));
"val"       return Ok(token!(self, Kind::Val));
"var"       return Ok(token!(self, Kind::Var));
"mut"       return Ok(token!(self, Kind::Mut));
"const"     return Ok(token!(self, Kind::Const));
"with"      return Ok(token!(self, Kind::With));
"ambient"   return Ok(token!(self, Kind::Ambient));
"enum"      return Ok(token!(self, Kind::Enum));
"match"     return Ok(token!(self, Kind::Match));
"requires"  return Ok(token!(self, Kind::Requires));
"ensures"   return Ok(token!(self, Kind::Ensures));
"type"      return Ok(token!(self, Kind::Type));

"true"      return Ok(token!(self, Kind::True));
"false"     return Ok(token!(self, Kind::False));

"u64"       return Ok(token!(self, Kind::U64));
"i64"       return Ok(token!(self, Kind::I64));
"f64"       return Ok(token!(self, Kind::F64));
"u8"        return Ok(token!(self, Kind::U8));
"u16"       return Ok(token!(self, Kind::U16));
"u32"       return Ok(token!(self, Kind::U32));
"i8"        return Ok(token!(self, Kind::I8));
"i16"       return Ok(token!(self, Kind::I16));
"i32"       return Ok(token!(self, Kind::I32));
"str"       return Ok(token!(self, Kind::Str));
"ptr"       return Ok(token!(self, Kind::Ptr));
"usize"     return Ok(token!(self, Kind::USize));
"null"      return Ok(token!(self, Kind::Null));
"bool"      return Ok(token!(self, Kind::Bool));
"dict"      return Ok(token!(self, Kind::Dict));
"Self"      return Ok(token!(self, Kind::Self_));

"::"        return Ok(token!(self, Kind::DoubleColon));
"->"        return Ok(token!(self, Kind::Arrow));
"=>"        return Ok(token!(self, Kind::FatArrow));
"=="        return Ok(token!(self, Kind::DoubleEqual));
"!="        return Ok(token!(self, Kind::NotEqual));
"<="        return Ok(token!(self, Kind::LE));
">="        return Ok(token!(self, Kind::GE));
"<"         return Ok(token!(self, Kind::LT));
">"         return Ok(token!(self, Kind::GT));
"<<"        return Ok(token!(self, Kind::LeftShift));
">>"        return Ok(token!(self, Kind::RightShift));
"&&"        return Ok(token!(self, Kind::DoubleAnd));
"||"        return Ok(token!(self, Kind::DoubleOr));

"("         return Ok(token!(self, Kind::ParenOpen));
")"         return Ok(token!(self, Kind::ParenClose));
"{"         return Ok(token!(self, Kind::BraceOpen));
"}"         return Ok(token!(self, Kind::BraceClose));
"["         return Ok(token!(self, Kind::BracketOpen));
"]"         return Ok(token!(self, Kind::BracketClose));
","         return Ok(token!(self, Kind::Comma));
".."        return Ok(token!(self, Kind::DotDot));
"."         return Ok(token!(self, Kind::Dot));
":"         return Ok(token!(self, Kind::Colon));
";"         return Ok(token!(self, Kind::Semicolon));
"!"         return Ok(token!(self, Kind::Exclamation));
"="         return Ok(token!(self, Kind::Equal));
"&"         return Ok(token!(self, Kind::And));
"|"         return Ok(token!(self, Kind::Or));
"^"         return Ok(token!(self, Kind::Xor));
"~"         return Ok(token!(self, Kind::Tilde));
"@"         return Ok(token!(self, Kind::At));
"?"         return Ok(token!(self, Kind::Question));
"+="        return Ok(token!(self, Kind::PlusEqual));
"-="        return Ok(token!(self, Kind::MinusEqual));
"*="        return Ok(token!(self, Kind::StarEqual));
"/="        return Ok(token!(self, Kind::SlashEqual));
"%="        return Ok(token!(self, Kind::PercentEqual));
"+"         return Ok(token!(self, Kind::IAdd));
"-"         return Ok(token!(self, Kind::ISub));
"*"         return Ok(token!(self, Kind::IMul));
"/"         return Ok(token!(self, Kind::IDiv));
"%"         return Ok(token!(self, Kind::IMod));


-?[0-9][0-9_]*"."[0-9][0-9_]*"f64"  {
                        // Numeric literal separators (`1_000_000.5_f64`):
                        // `_` is allowed between digits as a visual
                        // grouping aid. The first character must still
                        // be a digit so the regex never starts with
                        // `_` (which would conflict with identifier
                        // parsing). Strip every `_` before passing to
                        // `str::parse` — the underlying parser doesn't
                        // know about separators.
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<f64>() {
                            Ok(value) => return Ok(token!(self, Kind::Float64(value))),
                            Err(_) => {
                                self.get_last_lex_error().replace(LexErrorKind::MalformedNumber);
                                return Err(Error::Unmatch);
                            }
                        }
                    }

-?[0-9][0-9_]*"f64"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<f64>() {
                            Ok(value) => return Ok(token!(self, Kind::Float64(value))),
                            Err(_) => {
                                self.get_last_lex_error().replace(LexErrorKind::MalformedNumber);
                                return Err(Error::Unmatch);
                            }
                        }
                    }

-?[0-9][0-9_]*"i64"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<i64>() {
                            Ok(value) => return Ok(token!(self, Kind::Int64(value))),
                            Err(_) => {
                                // Return as regular integer token for large values
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

[0-9][0-9_]*"u64"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<u64>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt64(value))),
                            Err(_) => {
                                // Return as regular integer token for large values
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

-?[0-9][0-9_]*"i32"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<i32>() {
                            Ok(value) => return Ok(token!(self, Kind::Int32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9][0-9_]*"u32"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<u32>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9][0-9_]*"i16"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<i16>() {
                            Ok(value) => return Ok(token!(self, Kind::Int16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9][0-9_]*"u16"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<u16>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9][0-9_]*"i8"        {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<i8>() {
                            Ok(value) => return Ok(token!(self, Kind::Int8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9][0-9_]*"u8"          {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s: String = drain.collect::<String>().replace("_", "");
                        match s.parse::<u8>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F][0-9a-fA-F_]*"i64"  {
                        // Hex literals also accept `_` separators
                        // (`0xFF_FF_FF_FFi64`). Prefix `0x` / `-0x`
                        // is mandatory (anchors the regex); the
                        // first hex digit must come before any `_`
                        // for the same first-char-not-_ reason as
                        // decimal literals above.
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_str) = if s.starts_with("-") {
                            (true, &s[3..]) // Skip "-0x" or "-0X"
                        } else {
                            (false, &s[2..]) // Skip "0x" or "0X"
                        };
                        let hex_str = hex_str.replace("_", "");
                        let hex_str = hex_str.as_str();
                        match i64::from_str_radix(hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int64(final_value)))
                            },
                            Err(_) => {
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

0[xX][0-9a-fA-F][0-9a-fA-F_]*"u64"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = s[2..].replace("_", ""); // Skip "0x" or "0X"
                        match u64::from_str_radix(&hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt64(value))),
                            Err(_) => {
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

0[xX][0-9a-fA-F][0-9a-fA-F_]*"u32"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = s[2..].replace("_", "");
                        match u32::from_str_radix(&hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F][0-9a-fA-F_]*"u16"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = s[2..].replace("_", "");
                        match u16::from_str_radix(&hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F][0-9a-fA-F_]*"u8"   {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        let hex_str = s[2..].replace("_", "");
                        match u8::from_str_radix(&hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F][0-9a-fA-F_]*"i32"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_part) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        let hex_str = hex_part.replace("_", "");
                        match i32::from_str_radix(&hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int32(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F][0-9a-fA-F_]*"i16"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_part) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        let hex_str = hex_part.replace("_", "");
                        match i16::from_str_radix(&hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int16(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F][0-9a-fA-F_]*"i8"   {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_part) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        let hex_str = hex_part.replace("_", "");
                        match i8::from_str_radix(&hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int8(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F][0-9a-fA-F_]*   {
                        let text = self.yytext();
                        let hex_str = text[2..].replace("_", ""); // Skip "0x" or "0X"
                        // Validate that the hex string parses; either way we hand
                        // the raw text (separators included) to the parser /
                        // type checker as `Integer` so downstream context can
                        // apply an explicit suffix. The parser strips `_` again
                        // before computing the value.
                        match u64::from_str_radix(&hex_str, 16) {
                            Ok(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9][0-9_]*            return Ok(token!(self, Kind::Integer(self.yytext())));

\"[^"]*\"              {
                        // String literal with escape support + string
                        // interpolation. The regex itself stays simple
                        // (closing `"` always wins; `\"` would terminate
                        // early — same constraint as before). The 6
                        // single-char escapes `\n` / `\t` / `\r` / `\0`
                        // / `\\` / `\'`, plus `\xHH` and `\u{HEX}`, are
                        // converted in the byte-by-byte pass below.
                        // Unknown escapes bail out via `Error::Unmatch`.
                        //
                        // Interpolation: `{...}` inside the literal
                        // marks an expression segment. `{{` / `}}` are
                        // literal `{` / `}` (Rust convention). Brace
                        // depth is tracked inside expression segments
                        // so struct literals (`Point { x: 1, y: 2 }`)
                        // participate correctly. If at least one
                        // expression segment is found, the token
                        // becomes `Kind::InterpolatedString(parts)`;
                        // otherwise the original `Kind::String(out)`.
                        let text = self.yytext();
                        // INTERP-DIAG-SPAN: absolute offset of the
                        // literal's first inner byte, so each `{...}`
                        // segment can record where it really sits and
                        // the parser can put the re-lexed tokens back
                        // on the map.
                        let inner_base = self.yybytepos().start + 1;
                        let inner = &text[1..text.len() - 1];
                        let mut out = String::with_capacity(inner.len());
                        let mut parts: Vec<StringPart> = Vec::new();
                        let mut has_interpolation = false;
                        let bytes = inner.as_bytes();
                        let mut i = 0;
                        while i < bytes.len() {
                            let b = bytes[i];
                            // Interpolation: `{` opens an expression
                            // segment unless doubled (`{{` -> literal
                            // `{`). Must be checked before escape
                            // processing because `\` is not the escape
                            // marker for braces here.
                            if b == b'{' {
                                if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                                    out.push('{');
                                    i += 2;
                                    continue;
                                }
                                // Flush the literal segment built so
                                // far (may be empty if interpolation
                                // appears at the very start).
                                parts.push(StringPart::Literal(std::mem::take(&mut out)));
                                // Find matching `}` honoring brace
                                // depth. Nested string literals inside
                                // the expression are not supported (the
                                // outer `"` would have terminated the
                                // regex match anyway).
                                let expr_start = i + 1;
                                let mut depth: usize = 1;
                                let mut j = expr_start;
                                while j < bytes.len() {
                                    match bytes[j] {
                                        b'{' => depth += 1,
                                        b'}' => {
                                            depth -= 1;
                                            if depth == 0 {
                                                break;
                                            }
                                        }
                                        _ => {}
                                    }
                                    j += 1;
                                }
                                if depth != 0 {
                                    self.get_last_lex_error()
                                        .replace(LexErrorKind::UnterminatedInterpolation);
                                    return Err(Error::Unmatch);
                                }
                                let segment = match std::str::from_utf8(&bytes[expr_start..j]) {
                                    Ok(s) => s,
                                    // Unreachable: `inner` is a `&str` and
                                    // every branch above consumes whole
                                    // chars, so this slice is valid UTF-8.
                                    // Keep the error path anyway; the token
                                    // source falls back to UnmatchedChar.
                                    Err(_) => return Err(Error::Unmatch),
                                };
                                // STR-INTERP-FMT: `{expr:spec}` splits at
                                // the first depth-0 `:`. Nesting keeps a
                                // struct literal's field colons
                                // (`{Point { x: 1i64 }}`) and a dict
                                // literal's out of reach, and `::` is
                                // skipped as a pair so a path expression
                                // (`{Color::Red}`) is never mistaken for a
                                // spec. The spec text itself is validated
                                // by the parser.
                                let (expr_text, spec) = split_format_spec(segment);
                                parts.push(StringPart::Expr {
                                    text: expr_text.to_string(),
                                    spec: spec.map(|s| s.to_string()),
                                    offset: inner_base + expr_start,
                                });
                                has_interpolation = true;
                                i = j + 1;
                                continue;
                            }
                            // `}}` -> literal `}`. A bare `}` (not
                            // doubled) is allowed as a literal since
                            // we are clearly outside any `{...}`
                            // segment at this point — the `{` arm
                            // consumes through to its matching `}`.
                            if b == b'}' {
                                if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                                    out.push('}');
                                    i += 2;
                                } else {
                                    out.push('}');
                                    i += 1;
                                }
                                continue;
                            }
                            if b == b'\\' {
                                if i + 1 >= bytes.len() {
                                    self.get_last_lex_error()
                                        .replace(LexErrorKind::DanglingEscape);
                                    return Err(Error::Unmatch);
                                }
                                // `\xHH` — 2-digit hex byte. Consumes
                                // 4 input bytes and produces 1 output
                                // byte. Either hex digit being missing
                                // or non-hex bails as `Unmatch`.
                                if bytes[i + 1] == b'x' {
                                    if i + 3 >= bytes.len() {
                                        self.get_last_lex_error()
                                            .replace(LexErrorKind::MalformedHexEscape);
                                        return Err(Error::Unmatch);
                                    }
                                    let hi = match hex_digit(bytes[i + 2]) {
                                        Some(v) => v,
                                        None => {
                                            self.get_last_lex_error()
                                                .replace(LexErrorKind::MalformedHexEscape);
                                            return Err(Error::Unmatch);
                                        }
                                    };
                                    let lo = match hex_digit(bytes[i + 3]) {
                                        Some(v) => v,
                                        None => {
                                            self.get_last_lex_error()
                                                .replace(LexErrorKind::MalformedHexEscape);
                                            return Err(Error::Unmatch);
                                        }
                                    };
                                    let byte = (hi << 4) | lo;
                                    // `str` is UTF-8, so a lone byte >= 0x80
                                    // cannot be stored as itself. Pushing it
                                    // `as char` would silently encode it as
                                    // *two* bytes (Latin-1 -> UTF-8), which is
                                    // never what the source asked for. Use
                                    // `\u{HEX}` for non-ASCII code points.
                                    if byte >= 0x80 {
                                        self.get_last_lex_error()
                                            .replace(LexErrorKind::NonAsciiHexByte { byte });
                                        return Err(Error::Unmatch);
                                    }
                                    out.push(byte as char);
                                    i += 4;
                                    continue;
                                }
                                // `\u{HEX}` — Unicode code point,
                                // encoded into the string as 1-4 UTF-8
                                // bytes. Out-of-range or surrogate
                                // code points (which `char::from_u32`
                                // rejects) bail as `Unmatch`.
                                if bytes[i + 1] == b'u' {
                                    let (cp, end) = match parse_unicode_escape_body(bytes, i + 2)
                                    {
                                        Some(v) => v,
                                        None => {
                                            self.get_last_lex_error()
                                                .replace(LexErrorKind::MalformedHexEscape);
                                            return Err(Error::Unmatch);
                                        }
                                    };
                                    let c = match char::from_u32(cp) {
                                        Some(c) => c,
                                        None => {
                                            self.get_last_lex_error()
                                                .replace(LexErrorKind::InvalidCodePoint);
                                            return Err(Error::Unmatch);
                                        }
                                    };
                                    out.push(c);
                                    i = end;
                                    continue;
                                }
                                let escaped = match bytes[i + 1] {
                                    b'n'  => b'\n',
                                    b't'  => b'\t',
                                    b'r'  => b'\r',
                                    b'0'  => 0u8,
                                    b'\\' => b'\\',
                                    b'\'' => b'\'',
                                    // `\"` would terminate the regex
                                    // early today — unreachable until
                                    // the regex grows multi-char
                                    // matching, but listed so the
                                    // handler stays symmetric with the
                                    // char literal escape table.
                                    b'"'  => b'"',
                                    other => {
                                        self.get_last_lex_error()
                                            .replace(LexErrorKind::UnknownEscape { byte: other });
                                        return Err(Error::Unmatch);
                                    }
                                };
                                out.push(escaped as char);
                                i += 2;
                            } else if b < 0x80 {
                                out.push(b as char);
                                i += 1;
                            } else {
                                // Multi-byte UTF-8 scalar in the source —
                                // copy it through verbatim. `b as char` on
                                // each byte would re-encode every byte as
                                // its own code point, so a 3-byte `♠` came
                                // back out as 6 mojibake bytes. `inner` is a
                                // `&str` and every branch above consumes
                                // whole ASCII bytes, so `i` is always on a
                                // char boundary here.
                                let c = match inner[i..].chars().next() {
                                    Some(c) => c,
                                    // Unreachable (`i < bytes.len()` and `i`
                                    // is on a char boundary); the token
                                    // source falls back to UnmatchedChar.
                                    None => return Err(Error::Unmatch),
                                };
                                out.push(c);
                                i += c.len_utf8();
                            }
                        }
                        if has_interpolation {
                            // Push final trailing literal (may be empty).
                            parts.push(StringPart::Literal(out));
                            return Ok(token!(self, Kind::InterpolatedString(parts)));
                        }
                        return Ok(token!(self, Kind::String(out)));
                    }

\"[^"]*  {
                        // Unterminated string literal — no closing `"`.
                        // The rule above needs the closing quote to
                        // match, so this one only fires when it does
                        // not exist; the longest match takes the full
                        // literal (possibly to EOF). Recorded so the
                        // parser can say "the string never closes"
                        // instead of "unrecognized character `\"`".
                        self.get_last_lex_error().replace(LexErrorKind::UnterminatedString);
                        return Err(Error::Unmatch);
                    }

'\\u"{"[0-9a-fA-F]+"}"'            {
                        // char literal Unicode escape `'\u{HEX}'` —
                        // 1 to 6 hex digits encoding a Unicode code
                        // point (max 0x10FFFF). Lexes to
                        // `Kind::UInt32(<code point>)` so the value
                        // travels through the existing u32 literal
                        // pipeline. Out-of-range / surrogate code
                        // points (which `char::from_u32` rejects)
                        // bail as `Unmatch` so the user gets a parse
                        // error rather than a corrupted value.
                        let text = self.yytext();
                        let bytes = text.as_bytes();
                        // Layout: ' \ u { HEX } ' — body starts at
                        // index 3 (after `\u{`).
                        let (cp, _end) = match parse_unicode_escape_body(bytes, 3) {
                            Some(v) => v,
                            None => {
                                self.get_last_lex_error()
                                    .replace(LexErrorKind::MalformedHexEscape);
                                return Err(Error::Unmatch);
                            }
                        };
                        match char::from_u32(cp) {
                            Some(_) => {}
                            None => {
                                self.get_last_lex_error()
                                    .replace(LexErrorKind::InvalidCodePoint);
                                return Err(Error::Unmatch);
                            }
                        }
                        return Ok(token!(self, Kind::UInt32(cp)));
                    }

'\\x[0-9a-fA-F][0-9a-fA-F]'      {
                        // char literal hex escape `'\xHH'` — exactly
                        // 2 hex digits encode a u8 byte. The match
                        // length is fixed at 6 so no fall-through to
                        // the single-char escape rule below.
                        let bytes = self.yytext().into_bytes();
                        let hi = hex_digit(bytes[3]).ok_or(Error::Unmatch)?;
                        let lo = hex_digit(bytes[4]).ok_or(Error::Unmatch)?;
                        let value: u32 = ((hi << 4) | lo) as u32;
                        return Ok(token!(self, Kind::UInt32(value)));
                    }

'\\.'               {
                        // char literal escape — `'\n'` / `'\t'` / `'\r'`
                        // / `'\0'` / `'\\'` / `'\''` / `'\"'`. Lexes
                        // straight to `Kind::UInt32(<code point>)` so
                        // every downstream layer (parser / type
                        // checker / IR / 3 backends) reuses the
                        // existing `u32` literal pipeline. Unknown
                        // escapes (e.g. `'\x'` without 2 digits) bail
                        // out with `Error::Unmatch`.
                        let text = self.yytext();
                        let escape_byte = text.as_bytes()[2];
                        let value: u32 = match escape_byte {
                            b'n'  => 10,
                            b't'  => 9,
                            b'r'  => 13,
                            b'0'  => 0,
                            b'\\' => 92,
                            b'\'' => 39,
                            b'"'  => 34,
                            other => {
                                self.get_last_lex_error()
                                    .replace(LexErrorKind::UnknownEscape { byte: other });
                                return Err(Error::Unmatch);
                            }
                        };
                        return Ok(token!(self, Kind::UInt32(value)));
                    }

'[ -&(-~]'          {
                        // char literal — single ASCII printable byte
                        // (space 0x20 .. ~ 0x7E excluding `'` 0x27,
                        // which is covered by the escape rule above).
                        // Multi-byte UTF-8 chars and arbitrary
                        // control bytes are deferred to a later
                        // phase. Value is the byte's code point as
                        // `u32` — same shape as `<n>u32` literals.
                        let text = self.yytext();
                        let value = text.as_bytes()[1] as u32;
                        return Ok(token!(self, Kind::UInt32(value)));
                    }

[0-9][A-Za-z_0-9]*      {
                        // `123abc` — digits followed by identifier
                        // characters are not a number. Recorded so the
                        // parser can report it at this token instead of
                        // wherever the stream breaks.
                        self.get_last_lex_error().replace(LexErrorKind::BadNumber);
                        return Err(Error::Unmatch);
                    }
[A-Za-z_][A-Za-z_0-9]*  return Ok(token!(self, Kind::Identifier(self.yytext())));

"/*"([^*]|"*"[^/])*"*/"  {
                        let text = self.yytext();
                        // Remove /* and */ from the comment text
                        let comment_text = &text[2..text.len()-2];
                        // Count newlines for line tracking
                        let newline_count = comment_text.matches('\n').count();
                        self.line_count += newline_count as u64;
                        return Ok(token!(self, Kind::Comment(comment_text.to_string())));
                    }

"#".*               {
                        let mut text = self.yytext();
                        let drain = text.drain(1..);
                        return Ok(token!(self, Kind::Comment(drain.collect::<String>())));
                    }

" "                 /* skip whitespace */
\t                  /* skip tab */
\n                  self.line_count += 1; return Ok(token!(self, Kind::NewLine));

%%

    pub fn get_current_line_count(&self) -> usize {
        self.line_count as usize
    }

    /// Advance the lexer one character past a zero-width failure.
    ///
    /// When no rule matches a character at all, rflex falls through to
    /// `Error::Unmatch` with `zz_marked_pos == zz_start_read` — the
    /// position did not move, so a second `yylex` would fail on the
    /// same byte forever. The token source calls this after recording
    /// the error to resynchronise: the character is skipped, the
    /// lexer resumes after it, and the next `yylex` attempts a new
    /// token. Returns the number of bytes skipped (0 only when there
    /// is nothing left to skip, which the token source treats as EOF).
    #[allow(dead_code)]
    pub fn skip_current_char(&mut self) -> usize {
        if self.zz_start_read >= self.input.len() {
            return 0;
        }
        let Some(c) = self.input[self.zz_start_read..].chars().next() else {
            return 0;
        };
        let len = c.len_utf8();
        self.zz_marked_pos = self.zz_start_read + len;
        self.zz_marked_char += 1;
        // The DFA loop only ever reads `next.1` (the char) from the
        // iterator, so rebuilding it from the slice is equivalent to
        // advancing it — and does not depend on which state it was in
        // after the failed attempt.
        self.current = self.input[self.zz_marked_pos..].char_indices();
        len
    }