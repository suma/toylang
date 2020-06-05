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

    fn peek(&mut self) -> Option<&Token> {
        let tk = if self.token.is_none() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.token = Some(t);
                    self.token.as_ref()
                }
                _ => return None,
            }
        } else {
            self.token.as_ref()
        };
        return tk;
    }

    fn next(&mut self) {
        self.token = None;
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

    fn new_binary(op: Operator, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary(Box::new(
            BinaryExpr {
                op,
                lhs,
                rhs,
            }
        ))
    }

    pub fn expect_err(&mut self, accept: &Token) {
        if !self.expect(accept) {
            println!("{:?} expected but {:?}", accept, self.token)
        }
    }

    pub fn parse_expr(&mut self) -> Result<Expr, ()> {
        return self.parse_equality();
    }

    fn parse_equality(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_relational()?;

        loop {
            match self.peek() {
                Some(Token::DoubleEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = Self::new_binary(Operator::DoubleEqual, lhs, rhs);
                }
                Some(Token::NotEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = Self::new_binary(Operator::NotEqual, lhs, rhs);
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_relational(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_add()?;

        loop {
            match self.peek() {
                Some(Token::LT) => {
                    self.next();
                    lhs = Self::new_binary(Operator::LT, lhs, self.parse_add()?)
                }
                Some(Token::LE) => {
                    self.next();
                    lhs = Self::new_binary(Operator::LE, lhs, self.parse_add()?)
                }
                Some(Token::GT) => {
                    self.next();
                    lhs = Self::new_binary(Operator::GT, lhs, self.parse_add()?)
                }
                Some(Token::GE) => {
                    self.next();
                    lhs = Self::new_binary(Operator::GE, lhs, self.parse_add()?)
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_mul()?;

        loop {
            match self.peek() {
                Some(Token::IAdd) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = Self::new_binary(Operator::IAdd, lhs, rhs);
                }
                Some(Token::ISub) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = Self::new_binary(Operator::ISub, lhs, rhs);
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_mul(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_primary()?;

        loop {
            match self.peek() {
                Some(Token::IMul) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = Self::new_binary(Operator::IMul, lhs, rhs);
                }
                Some(Token::IDiv) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = Self::new_binary(Operator::IDiv, lhs, rhs);
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        match self.peek() {
            Some(Token::ParenOpen) => {
                self.next();
                let node = self.parse_expr()?;
                self.expect_err(&Token::ParenClose);
                return Ok(node);
            }
            _ => {
                let e = match self.peek() {
                    Some(&Token::UInt64(num)) => {
                        Ok(Expr::UInt64(num))
                    }
                    Some(&Token::Int64(num)) => {
                        Ok(Expr::Int64(num))
                    }
                    Some(Token::Integer(num)) => {
                        Ok(Expr::Int64(0))  // FIXME
                    }
                    _ => return Err(()),
                };
                self.next();
                return e;
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
    fn lexer_simple_symbol1() {
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
    fn lexer_simple_symbol2() {
        let s = "== != <= < >= >";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::DoubleEqual);
        assert_eq!(l.yylex().unwrap(), Token::NotEqual);
        assert_eq!(l.yylex().unwrap(), Token::LE);
        assert_eq!(l.yylex().unwrap(), Token::LT);
        assert_eq!(l.yylex().unwrap(), Token::GE);
        assert_eq!(l.yylex().unwrap(), Token::GT);
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
                op: Operator::IAdd,
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
                op: Operator::IAdd,
                lhs: Expr::UInt64(1),
                rhs: Expr::Binary(Box::new(
                    BinaryExpr {
                        op: Operator::IMul,
                        lhs: Expr::UInt64(2),
                        rhs: Expr::UInt64(3),
                    }
                )),
            }
        )), res);
    }

    #[test]
    fn parser_simple_relational_expr() {
        let mut p = Parser::new("0u64 < 2u64 + 4u64");
        let res = p.parse_expr().unwrap();
        assert_eq!(Expr::Binary(Box::new(
            BinaryExpr {
                op: Operator::LT,
                lhs: Expr::UInt64(0),
                rhs: Expr::Binary(Box::new(
                    BinaryExpr {
                        op: Operator::IAdd,
                        lhs: Expr::UInt64(2),
                        rhs: Expr::UInt64(4),
                    }
                )),
            }
        )), res);
    }
}
