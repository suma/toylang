use crate::token::Token;

%%
%class Lexer
%result_type Token
%field u64 line_count

"if"     return Ok(Token::If);
"else"   return Ok(Token::Else);
"for"    return Ok(Token::For);
"while"  return Ok(Token::While);
"break"  return Ok(Token::Break);
"continue"  return Ok(Token::Continue);
"class"  return Ok(Token::Class);
"struct" return Ok(Token::Struct);
"fn"     return Ok(Token::Function);
"return" return Ok(Token::Return);
"extern" return Ok(Token::Extern);
"pub"    return Ok(Token::Public);
"val"    return Ok(Token::Val);
"var"    return Ok(Token::Var);

"("      return Ok(Token::ParenOpen);
")"      return Ok(Token::ParenClose);
"{"      return Ok(Token::BraceOpen);
"}"      return Ok(Token::BraceClose);
"["      return Ok(Token::BracketOpen);
"]"      return Ok(Token::BracketClose);
","      return Ok(Token::Comma);
"."      return Ok(Token::Dot);
"::"     return Ok(Token::DoubleColon);
":"      return Ok(Token::Colon);
"->"     return Ok(Token::Arrow);
"!"      return Ok(Token::Exclamation);

"="      return Ok(Token::Equal);

"=="     return Ok(Token::DoubleEqual);
"!="     return Ok(Token::NotEqual);
"<="     return Ok(Token::LE);
"<"      return Ok(Token::LT);
">="     return Ok(Token::GE);
">"      return Ok(Token::GT);

"&&"     return Ok(Token::DoubleAnd);
"||"     return Ok(Token::DoubleOr);

"+"      return Ok(Token::IAdd);
"-"      return Ok(Token::ISub);
"*"      return Ok(Token::IMul);
"/"      return Ok(Token::IDiv);
"+."     return Ok(Token::FAdd);
"-."     return Ok(Token::FSub);
"*."     return Ok(Token::FMul);
"/."     return Ok(Token::FDiv);


-?[0-9]+"i64"       let mut text = self.yytext();
                    let drain = text.drain(0..(text.len()-3)); let s = drain.collect::<String>();
                    return Ok(Token::Int64(s.parse::<i64>().unwrap()));
[0-9]+"u64"         let mut text = self.yytext();
                    let drain = text.drain(0..(text.len()-3)); let s = drain.collect::<String>();
                    return Ok(Token::UInt64(s.parse::<u64>().unwrap()));
-?[0-9]+            return Ok(Token::Integer(self.yytext()));
                    /* TODO: hold original text in lexer as used for lint */

"u64"      return Ok(Token::U64);
"i64"      return Ok(Token::I64);
"ptr"      return Ok(Token::Ptr);
"usize"    return Ok(Token::USize);
"null"     return Ok(Token::Null);

[A-Za-z_][A-Za-z_0-9]*  return Ok(Token::Identifier(self.yytext()));

" "      /* skip ws (TODO: count and return ws)  */
\t       /* skip tab */
\n       self.line_count += 1; return Ok(Token::NewLine);

%%

