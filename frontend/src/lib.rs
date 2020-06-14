pub mod token;
pub mod ast;
use crate::token::Token;
use crate::ast::*;

mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    lexer: lexer::Lexer<'a>,
    ahead: Vec<Token>,
    current_id: u64,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = lexer::Lexer::new(&input, 1u64);
        Parser {
            lexer,
            ahead: Vec::new(),
            current_id: 0,
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        if self.ahead.is_empty() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.ahead.push(t);
                    self.ahead.get(0)
                }
                _ => return None,
            }
        } else {
            self.ahead.get(0)
        }
    }

    // pos: 0-origin
    fn peek_n(&mut self, pos: usize) -> Option<&Token> {
        while self.ahead.len() < pos + 1 {
            match self.lexer.yylex() {
                Ok(t) => self.ahead.push(t),
                _ => return None,
            }
        }
        return self.ahead.get(pos);
    }

    fn consume(&mut self, count: usize) -> usize{
        return self.ahead.drain(0..count).count();
    }

    fn next(&mut self) {
        self.ahead.remove(0);
    }

    pub fn expect(&mut self, accept: &Token) -> bool {
        let tk = self.peek();
        if *tk.unwrap() == *accept {
            self.next();
            true
        } else {
            self.next();
            false
        }
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
            println!("{:?} expected but {:?}", accept, self.ahead.get(0))
        }
    }

    pub fn parse_expr(&mut self) -> Result<Expr, ()> {
        return self.parse_logical_expr();
    }

    fn parse_logical_expr(&mut self) -> Result<Expr, ()> {
        let mut lhs = self.parse_equality()?;

        loop {
            match self.peek() {
                Some(Token::DoubleAnd) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = Self::new_binary(Operator::LogicalAnd, lhs, rhs);
                }
                Some(Token::DoubleOr) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = Self::new_binary(Operator::LogicalOr, lhs, rhs);
                }
                _ => return Ok(lhs),
            }
        }
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

    fn fresh_ty(&mut self) -> VarType {
        self.current_id += 1;
        return VarType {
            id: self.current_id,
            ty: Type::Unknown,
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
            Some(Token::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                return match self.peek() {
                    Some(Token::ParenOpen) => {
                        // function call
                        self.next();
                        let ty = Type::Variable(Box::new(self.fresh_ty()));
                        let args = self.parse_expr_list(vec![])?;
                        self.expect_err(&Token::ParenClose);
                        Ok(Expr::Call(TVar{ s, ty }, args))
                    }
                    _ => {
                        // identifier
                        let ty = Type::Variable(Box::new(self.fresh_ty()));
                        Ok(Expr::Identifier(TVar{ s, ty }))
                    }
                }
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

    fn parse_expr_list(&mut self, mut args: Vec<Expr>) -> Result<Vec<Expr>, ()> {
        let expr = self.parse_expr();
        if expr.is_ok() {
            args.push(expr.unwrap());
        } else {
            return Ok(args);
        }

        match self.peek() {
            Some(Token::Comma) => {
                self.next();
                return Ok(self.parse_expr_list(args)?);
            }
            _ => return Ok(args),
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
    fn parser_util_lookahead() {
        let mut p = Parser::new("1u64 + 2u64");;
        let t1 = p.peek_n(1).unwrap();
        assert_eq!(Token::IAdd, *t1);
        assert_eq!(2, p.consume(2));

        let t2 = p.peek().unwrap();
        assert_eq!(Token::UInt64(2), *t2);

        let t3 = p.peek_n(2);
        assert!(t3.is_some());
        assert_eq!(Token::EOF, *(t3.unwrap()));
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

    #[test]
    fn parser_simple_logical_expr() {
        let mut p = Parser::new("1u64 && 2u64 < 3u64");
        let res = p.parse_expr().unwrap();
        assert_eq!(Expr::Binary(Box::new(
            BinaryExpr {
                op: Operator::LogicalAnd,
                lhs: Expr::UInt64(1),
                rhs: Expr::Binary(Box::new(
                    BinaryExpr {
                        op: Operator::LT,
                        lhs: Expr::UInt64(2),
                        rhs: Expr::UInt64(3),
                    }
                )),
            }
        )), res);
    }

    #[test]
    fn parser_expr_accept() {
        assert!(Parser::new("1u64").parse_expr().is_ok());
        assert!(Parser::new("(1u64 + 2u64)").parse_expr().is_ok());
        assert!(Parser::new("1u64 && 2u64 < 3u64").parse_expr().is_ok());
        assert!(Parser::new("1u64 || 2u64 < 3u64").parse_expr().is_ok());
        assert!(Parser::new("1u64 || (2u64) < 3u64 + 4u64").parse_expr().is_ok());

        assert!(Parser::new("variable").parse_expr().is_ok());
        assert!(Parser::new("a + b").parse_expr().is_ok());
        assert!(Parser::new("a + 1u64").parse_expr().is_ok());

        assert!(Parser::new("a() + 1u64").parse_expr().is_ok());
        assert!(Parser::new("a(b,c) + 1u64").parse_expr().is_ok());
    }

    #[test]
    fn parser_simple_ident_expr() {
        let res = Parser::new("abc + 1u64").parse_expr().unwrap();;
        assert_eq!(Expr::Binary(Box::new(
                BinaryExpr {
                    op: Operator::IAdd,
                    lhs: Expr::Identifier(TVar {
                        s: "abc".to_string(),
                        ty: Type::Variable(Box::new(VarType{ id: 1, ty: Type::Unknown })),
                    }),
                    rhs: Expr::UInt64(1),
                }
            ),
        ), res);
    }

    #[test]
    fn parser_simple_apply_empty() {
        let res = Parser::new("abc()").parse_expr().unwrap();;
        assert_eq!(Expr::Call {
            0: TVar { s: "abc".to_string(), ty: Type::Variable(Box::new(VarType{ id: 1, ty: Type::Unknown }))},
            1: vec![],
        }, res);
    }

    #[test]
    fn parser_simple_apply_expr() {
        let res = Parser::new("abc(1u64,2u64)").parse_expr().unwrap();;
        assert_eq!(Expr::Call {
            0: TVar { s: "abc".to_string(), ty: Type::Variable(Box::new(VarType{ id: 1, ty: Type::Unknown }))},
            1: vec![
                Expr::UInt64(1),
                Expr::UInt64(2),
            ],
        }, res);
    }
}
