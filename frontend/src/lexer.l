use crate::token::Token;

%%
%class Lexer
%result_type Token

"if"     return Ok(Token::If);
"else"   return Ok(Token::Else);
"for"    return Ok(Token::For);
"while"  return Ok(Token::While);
" "      /* skip ws (TODO: count and return ws)  */

%%

