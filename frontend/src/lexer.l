use crate::token::{Token, Kind};

macro_rules! token {
    ($self: ident, $kind: expr) => {
        Token { kind: $kind, position: $self.yybytepos() }
    }
}

%%
%class Lexer
%result_type Token
%field u64 line_count

"if"        return Ok(token!(self, Kind::If));
"elif"      return Ok(token!(self, Kind::Elif));
"else"      return Ok(token!(self, Kind::Else));
"for"       return Ok(token!(self, Kind::For));
"in"        return Ok(token!(self, Kind::In));
"to"        return Ok(token!(self, Kind::To));
"while"     return Ok(token!(self, Kind::While));
"break"     return Ok(token!(self, Kind::Break));
"continue"  return Ok(token!(self, Kind::Continue));
"return"    return Ok(token!(self, Kind::Return));

"class"     return Ok(token!(self, Kind::Class));
"struct"    return Ok(token!(self, Kind::Struct));
"trait"     return Ok(token!(self, Kind::Trait));
"impl"      return Ok(token!(self, Kind::Impl));
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


-?[0-9]+"."[0-9]+"f64"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<f64>() {
                            Ok(value) => return Ok(token!(self, Kind::Float64(value))),
                            Err(_) => return Err(Error::Unmatch),
                        }
                    }

-?[0-9]+"f64"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<f64>() {
                            Ok(value) => return Ok(token!(self, Kind::Float64(value))),
                            Err(_) => return Err(Error::Unmatch),
                        }
                    }

-?[0-9]+"i64"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<i64>() {
                            Ok(value) => return Ok(token!(self, Kind::Int64(value))),
                            Err(_) => {
                                // Return as regular integer token for large values
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

[0-9]+"u64"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<u64>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt64(value))),
                            Err(_) => {
                                // Return as regular integer token for large values
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

-?[0-9]+"i32"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<i32>() {
                            Ok(value) => return Ok(token!(self, Kind::Int32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9]+"u32"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<u32>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9]+"i16"       {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<i16>() {
                            Ok(value) => return Ok(token!(self, Kind::Int16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9]+"u16"         {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        match s.parse::<u16>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9]+"i8"        {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        match s.parse::<i8>() {
                            Ok(value) => return Ok(token!(self, Kind::Int8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

[0-9]+"u8"          {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        match s.parse::<u8>() {
                            Ok(value) => return Ok(token!(self, Kind::UInt8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F]+"i64"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_str) = if s.starts_with("-") {
                            (true, &s[3..]) // Skip "-0x" or "-0X"
                        } else {
                            (false, &s[2..]) // Skip "0x" or "0X"
                        };
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

0[xX][0-9a-fA-F]+"u64"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = &s[2..]; // Skip "0x" or "0X"
                        match u64::from_str_radix(hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt64(value))),
                            Err(_) => {
                                return Ok(token!(self, Kind::Integer(self.yytext())))
                            }
                        }
                    }

0[xX][0-9a-fA-F]+"u32"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = &s[2..];
                        match u32::from_str_radix(hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt32(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F]+"u16"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let hex_str = &s[2..];
                        match u16::from_str_radix(hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt16(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F]+"u8"   {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        let hex_str = &s[2..];
                        match u8::from_str_radix(hex_str, 16) {
                            Ok(value) => return Ok(token!(self, Kind::UInt8(value))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F]+"i32"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_str) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        match i32::from_str_radix(hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int32(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F]+"i16"  {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 3));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_str) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        match i16::from_str_radix(hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int16(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?0[xX][0-9a-fA-F]+"i8"   {
                        let mut text = self.yytext();
                        let drain = text.drain(0..(text.len() - 2));
                        let s = drain.collect::<String>();
                        let (is_negative, hex_str) = if s.starts_with("-") {
                            (true, &s[3..])
                        } else {
                            (false, &s[2..])
                        };
                        match i8::from_str_radix(hex_str, 16) {
                            Ok(value) => {
                                let final_value = if is_negative { -value } else { value };
                                return Ok(token!(self, Kind::Int8(final_value)))
                            },
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

0[xX][0-9a-fA-F]+   {
                        let text = self.yytext();
                        let hex_str = &text[2..]; // Skip "0x" or "0X"
                        // Validate that the hex string parses; either way we hand
                        // the raw text to the parser/type checker as `Integer` so
                        // it can apply an explicit suffix or context-driven type.
                        match u64::from_str_radix(hex_str, 16) {
                            Ok(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                            Err(_) => return Ok(token!(self, Kind::Integer(self.yytext()))),
                        }
                    }

-?[0-9]+            return Ok(token!(self, Kind::Integer(self.yytext())));

\"[^"]*\"              {
                        let mut text = self.yytext();
                        let drain = text.drain(1..(text.len()-1));
                        return Ok(token!(self, Kind::String(drain.collect::<String>())));
                    }

'\\.'               {
                        // char literal escape — `'\n'` / `'\t'` / `'\r'`
                        // / `'\0'` / `'\\'` / `'\''` / `'\"'`. Lexes
                        // straight to `Kind::UInt32(<code point>)` so
                        // every downstream layer (parser / type
                        // checker / IR / 3 backends) reuses the
                        // existing `u32` literal pipeline. Unknown
                        // escapes (e.g. `'\x'`) bail out with
                        // `Error::Unmatch`.
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
                            _ => return Err(Error::Unmatch),
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

[0-9][A-Za-z_0-9]*      return Err(Error::Unmatch);
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