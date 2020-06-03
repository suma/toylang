use crate::token::Token;

%%
%class Lexer
%result_type Token

"if"     return Ok(Token::If);
"else"   return Ok(Token::Else);
"for"    return Ok(Token::For);
"while"  return Ok(Token::While);
"class"  return Ok(Token::Class);
"fn"     return Ok(Token::Function);

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


[A-Za-z_][A-Za-z_0-9]*  return Ok(Token::Identifier(self.yytext()));

" "      /* skip ws (TODO: count and return ws)  */

%%

