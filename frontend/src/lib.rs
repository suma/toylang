mod token;
mod ast;
use crate::token::Token;
use crate::ast::*;

mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    lexer: lexer::Lexer<'a>,
    token: Option<Token>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = lexer::Lexer::new(&input, 1u64);
        Parser {
            lexer,
            token: None,
        }
    }

    pub fn consume(&mut self, accept: Token) -> bool {
        let tk = if self.token.is_none() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.token = Some(t);
                    self.token.as_ref()
                }
                _ => return false,
            }
        } else {
            self.token.as_ref()
        };
        if *tk.unwrap() == accept {
            self.token = None;
            true
        } else {
            false
        }
    }

    pub fn expect(&mut self, accept: &Token) -> bool {
        let tk = if self.token.is_none() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.token = Some(t);
                    self.token.as_ref()
                }
                _ => return false,
            }
        } else {
            self.token.as_ref()
        };
        if *tk.unwrap() == *accept {
            self.token = None;
            true
        } else {
            false
        }
    }

    fn token(&mut self) -> Result<Token, ()> {
        if self.token.is_none() {
            let res = self.lexer.yylex();
            if !res.is_err() {
                return Ok(res.unwrap());
            }
        }
        return Ok(self.token.take().unwrap());
    }

    pub fn expect_err(&mut self, accept: &Token) {
        if !self.expect(accept) {
            println!("{:?} expected but {:?}", accept, self.token)
        }
    }

    pub fn parse_expr(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_mul()?;

        loop {
            if self.consume(Token::IAdd) {
                let rhs = self.parse_mul()?;
                let bexpr = BinaryExpr {
                    op: Operator::Add,
                    lhs,
                    rhs,
                };
                lhs = Expr::Binary(Box::new(bexpr));
            } else if self.consume(Token::IAdd) {
                let rhs = self.parse_mul()?;
                let bexpr = BinaryExpr {
                    op: Operator::Sub,
                    lhs,
                    rhs,
                };
                lhs = Expr::Binary(Box::new(bexpr));
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_mul(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_primary()?;

        loop {
            if self.consume(Token::IMul) {
                let rhs = self.parse_mul()?;
                let bexpr = BinaryExpr {
                    op: Operator::Mul,
                    lhs,
                    rhs,
                };
                lhs = Expr::Binary(Box::new(bexpr));
            } else if self.consume(Token::IDiv) {
                let rhs = self.parse_mul()?;
                let bexpr = BinaryExpr {
                    op: Operator::Div,
                    lhs,
                    rhs,
                };
                lhs = Expr::Binary(Box::new(bexpr));
            } else {
                return Ok(lhs);
            }
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        if self.consume(Token::ParenOpen) {
            let node = self.parse_expr()?;
            self.expect_err(&Token::ParenClose);
            println!("primary {:?}", node);
            return Ok(node);
        } else {
            let t = self.token()?;
            return match t {
                Token::UInt64(num) => {
                    Ok(Expr::UInt64(num))
                }
                Token::Int64(num) => {
                    Ok(Expr::Int64(num))
                }
                Token::Integer(num) => {
                    Ok(Expr::Int64(0))  // FIXME
                }
                x => {
                    Err(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Token;

    #[test]
    fn lexer_simple_keyword() {
        let s = " if else while for class fn val var";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::If);
        assert_eq!(l.yylex().unwrap(), Token::Else);
        assert_eq!(l.yylex().unwrap(), Token::While);
        assert_eq!(l.yylex().unwrap(), Token::For);
        assert_eq!(l.yylex().unwrap(), Token::Class);
        assert_eq!(l.yylex().unwrap(), Token::Function);
        assert_eq!(l.yylex().unwrap(), Token::Val);
        assert_eq!(l.yylex().unwrap(), Token::Var);
        assert_eq!(l.yylex().unwrap(), Token::EOF);
    }

    #[test]
    fn lexer_simple_integer() {
        let s = " -1i64 1i64 2u64 123 -456";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::Int64(-1));
        assert_eq!(l.yylex().unwrap(), Token::Int64(1));
        assert_eq!(l.yylex().unwrap(), Token::UInt64(2u64));
        assert_eq!(l.yylex().unwrap(), Token::Integer("123".to_string()));
        assert_eq!(l.yylex().unwrap(), Token::Integer("-456".to_string()));
    }

    #[test]
    fn lexer_simple_symbol() {
        let s = " ( ) { } [ ] , . :: : =";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::ParenOpen);
        assert_eq!(l.yylex().unwrap(), Token::ParenClose);
        assert_eq!(l.yylex().unwrap(), Token::BraceOpen);
        assert_eq!(l.yylex().unwrap(), Token::BraceClose);
        assert_eq!(l.yylex().unwrap(), Token::BracketOpen);
        assert_eq!(l.yylex().unwrap(), Token::BracketClose);
        assert_eq!(l.yylex().unwrap(), Token::Comma);
        assert_eq!(l.yylex().unwrap(), Token::Dot);
        assert_eq!(l.yylex().unwrap(), Token::DoubleColon);
        assert_eq!(l.yylex().unwrap(), Token::Colon);
        assert_eq!(l.yylex().unwrap(), Token::Equal);
    }

    #[test]
    fn lexer_arithmetic_operator_symbol() {
        let s = " + - * / +. -. *. /.";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::IAdd);
        assert_eq!(l.yylex().unwrap(), Token::ISub);
        assert_eq!(l.yylex().unwrap(), Token::IMul);
        assert_eq!(l.yylex().unwrap(), Token::IDiv);
        assert_eq!(l.yylex().unwrap(), Token::FAdd);
        assert_eq!(l.yylex().unwrap(), Token::FSub);
        assert_eq!(l.yylex().unwrap(), Token::FMul);
        assert_eq!(l.yylex().unwrap(), Token::FDiv);
    }

    #[test]
    fn lexer_simple_identifier() {
        let s = " A _name Identifier ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap(), Token::Identifier("_name".to_string()));
        assert_eq!(l.yylex().unwrap(), Token::Identifier("Identifier".to_string()));
    }

    #[test]
    fn lexer_multiple_lines() {
        let s = " A \n B ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap(), Token::NewLine);
        assert_eq!(l.yylex().unwrap(), Token::Identifier("B".to_string()));
        assert_eq!(*l.get_line_count(), 2);
    }

    #[test]
    fn parser_simple_expr() {
        let mut p = Parser::new("1u64 + 2u64 ");
        let res = p.parse_expr().unwrap();
        assert_eq!(Expr::Binary(Box::new(
            BinaryExpr {
                op: Operator::Add,
                lhs: Expr::UInt64(1),
                rhs: Expr::UInt64(2),
            }
        )), res);
    }

    #[test]
    fn parser_simple_expr_mul() {
        let mut p = Parser::new("(1u64) + 2u64 * 3u64");
        let res = p.parse_expr().unwrap();
        assert_eq!(Expr::Binary(Box::new(
            BinaryExpr {
                op: Operator::Add,
                lhs: Expr::UInt64(1),
                rhs: Expr::Binary(Box::new(
                    BinaryExpr {
                        op: Operator::Mul,
                        lhs: Expr::UInt64(2),
                        rhs: Expr::UInt64(3),
                    }
                )),
            }
        )), res);
    }
}
