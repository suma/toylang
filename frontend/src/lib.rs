pub mod ast;
pub mod token;
use crate::ast::*;
use crate::token::Token;

mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    lexer: lexer::Lexer<'a>,
    ahead: Vec<Token>,
    ast:   ExprPool,
    inst:  Vec<Inst>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = lexer::Lexer::new(&input, 1u64);
        let pool = ExprPool(Vec::with_capacity(1024 * 1024));
        let inst = Vec::<Inst>::with_capacity(1024 * 1024);
        Parser {
            lexer,
            ahead: Vec::new(),
            ast: pool,
            inst: inst,
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
    #[allow(dead_code)]
    fn peek_n(&mut self, pos: usize) -> Option<&Token> {
        while self.ahead.len() < pos + 1 {
            match self.lexer.yylex() {
                Ok(t) => self.ahead.push(t),
                _ => return None,
            }
        }
        return self.ahead.get(pos);
    }

    #[allow(dead_code)]
    fn consume(&mut self, count: usize) -> usize {
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
            false
        }
    }

    fn new_binary(op: Operator, lhs: ExprRef, rhs: ExprRef) -> Expr {
        Expr::Binary(op, lhs, rhs)
    }

    pub fn expect_err(&mut self, accept: &Token) -> Result<(), String> {
        if !self.expect(accept) {
            return Err(format!("{:?} expected but {:?}", accept, self.ahead.get(0)));
        }
        Ok(())
    }

    fn add_inst(&mut self, i: Inst) {
        self.inst.push(i);
    }

    pub fn get_inst(&self, i: usize) -> Option<&Inst> {
        return self.inst.get(i);
    }

    pub fn inst_len(&self) -> usize {
        return self.inst.len();
    }

    pub fn inst_iter(&self) -> std::slice::Iter<'_, Inst> {
        return self.inst.iter();
    }

    fn add(&mut self, e: Expr) -> ExprRef {
        let len = self.ast.0.len();
        self.ast.0.push(e);
        ExprRef(len as u32)
    }

    pub fn get(&self, i: u32) -> Option<&Expr> {
        return self.ast.0.get(i as usize);
    }

    pub fn len(&self) -> usize {
        return self.ast.0.len();
    }

    pub fn next_expr(&self) -> u32 {
        return self.len() as u32;
    }

    // prog := expr NewLine expr | expr | e
    // expr := assign | if_expr
    // block := "{" prog* "}"
    // if_expr := "if" expr block else_expr?
    // else_expr := "else" block
    // assign := val_def | identifier "=" logical_expr | logical_expr
    // val_def := "val" identifier (":" def_ty)? ("=" logical_expr)
    // def_ty := Int64 | UInt64 | identifier | Unknown
    // logical_expr := equality ("&&" relational | "||" relational)*
    // equality := relational ("==" relational | "!=" relational)*
    // relational := add ("<" add | "<=" add | ">" add | ">=" add")*
    // add := mul ("+" mul | "-" mul)*
    // mul := primary ("*" mul | "/" mul)*
    // primary := "(" expr ")" | identifier "(" expr_list ")" |
    //            identifier |
    //            UInt64 | Int64 | Integer | Null
    // expr_list = "" | expr | expr "," expr_list
    // TODO: add define function

    // this function is for test
    pub fn parse_stmt_line(&mut self) -> Result<ExprRef, String> {
        let expr = self.parse_some_exprs()?;
        if expr.len() != 1 {
            return Err(format!("parse_stmt_line: expected 1 expression but {:?} length", expr.len()));
        }
        self.inst.push(Inst::Expression(ExprRef(self.next_expr() - 1)));
        Ok(*expr.first().unwrap())
    }

    pub fn parse_some_exprs(&mut self) -> Result<Vec<ExprRef>, String> {
        // remove unused NewLine
        loop {
            match self.peek() {
                Some(Token::NewLine) =>
                    self.next(),
                Some(_) | None =>
                    break,
            }
        }
        let mut exprs = vec![];

        let lhs = self.parse_expr();
        if lhs.is_err() {
            return Err(format!("parse_some_exprs: expected expression: {:?}", lhs.err()));
        }
        exprs.push(lhs.unwrap());
        match self.peek() {
            Some(Token::NewLine) => {
                loop {
                    match self.peek() {
                        Some(Token::NewLine) =>
                            self.next(),
                        Some(_) => {
                            let rhs = self.parse_expr();
                            if rhs.is_err() {
                                return Err(format!("parse_some_exprs: expected expression: {:?}", rhs.err()));
                            }
                            self.add_inst(Inst::Expression(ExprRef(self.next_expr())));
                            exprs.push(rhs.unwrap());
                            break;
                        }
                        _ => break,
                    }
                }
            }
            _ => (),
        }
        return Ok(exprs);
    }

    pub fn parse_expr(&mut self) -> Result<ExprRef, String> {
        let assign = self.parse_assign();
        if assign.is_ok() {
            return assign;
        }

        match self.peek() {
            Some(Token::If) => {
                self.next();
                return self.parse_if();
            }
            Some(Token::Val) => {
                self.next();
                return self.parse_val_def();
            }
            x => {
                return Err(format!("parse_expr: expected expression but {:?}", x));
            }
        }
    }

    pub fn parse_assign(&mut self) -> Result<ExprRef, String> {
        match self.peek() {
            Some(Token::Val) => {
                self.next();
                return self.parse_val_def();
            }
            _ => {
                let lhs = self.parse_logical_expr()?;
                match self.peek() {
                    Some(Token::Equal) => {
                        self.next();
                        let rhs = self.parse_logical_expr()?;
                        return Ok(self.add(Self::new_binary(
                            Operator::Assign,
                            lhs,
                            rhs),
                        ));
                    }
                    _ => return Ok(lhs),
                }
            }
        }
    }

    pub fn parse_if(&mut self) -> Result<ExprRef, String> {
        let cond = self.parse_logical_expr()?;
        let if_block = self.parse_block()?;

        let else_block: ExprRef = match self.peek() {
            Some(Token::Else) => {
                self.next();
                self.parse_block()?
            }
            _ => self.add(Expr::Block(vec![])), // through
        };
        return Ok(self.add(Expr::IfElse(cond, if_block, else_block)));
    }

    pub fn parse_block(&mut self) -> Result<ExprRef, String> {
        self.expect_err(&Token::BraceOpen)?;
        let block = self.parse_some_exprs()?;
        self.expect_err(&Token::BraceClose)?;
        return Ok(self.add(Expr::Block(block)));
    }

    pub fn parse_val_def(&mut self) -> Result<ExprRef, String> {
        let ident: String = match self.peek() {
            Some(Token::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                s
            }
            x => return Err(format!("parse_val_def: expected identifier but {:?}", x)),
        };

        let ty: Type = match self.peek() {
            Some(Token::Colon) => {
                self.next();
                self.parse_def_ty()?
            }
            _ => Type::Unknown,
        };

        // "=" logical_expr
        let rhs = match self.peek() {
            Some(Token::Equal) => {
                self.next();
                Some(self.parse_logical_expr()?)
            }
            _ => None,
        };
        return Ok(self.add(Expr::Val(ident, Some(ty), rhs)));
    }

    fn parse_def_ty(&mut self) -> Result<Type, String> {
        let ty: Type = match self.peek() {
            Some(Token::U64) => Type::UInt64,
            Some(Token::I64) => Type::Int64,
            Some(Token::Identifier(s)) => {
                let ident = s.to_string();
                Type::Identifier(ident)
            }
            _ => Type::Unknown,
        };
        self.next();
        return Ok(ty);
    }

    fn parse_logical_expr(&mut self) -> Result<ExprRef, String> {
        let mut lhs = self.parse_equality()?;

        loop {
            match self.peek() {
                Some(Token::DoubleAnd) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::LogicalAnd, lhs, rhs));
                }
                Some(Token::DoubleOr) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::LogicalOr, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_equality(&mut self) -> Result<ExprRef, String> {
        let mut lhs = self.parse_relational()?;

        loop {
            match self.peek() {
                Some(Token::DoubleEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::EQ, lhs, rhs));
                }
                Some(Token::NotEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::NE, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_relational(&mut self) -> Result<ExprRef, String> {
        let mut lhs = self.parse_add()?;

        loop {
            match self.peek() {
                Some(Token::LT) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::LT, lhs, rhs));
                }
                Some(Token::LE) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::LE, lhs, rhs));
                }
                Some(Token::GT) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::GT, lhs, rhs));
                }
                Some(Token::GE) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::GE, lhs, rhs))
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_add(&mut self) -> Result<ExprRef, String> {
        let mut lhs = self.parse_mul()?;

        loop {
            match self.peek() {
                Some(Token::IAdd) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IAdd, lhs, rhs));
                }
                Some(Token::ISub) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::ISub, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_mul(&mut self) -> Result<ExprRef, String> {
        let mut lhs = self.parse_primary()?;

        loop {
            match self.peek() {
                Some(Token::IMul) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IMul, lhs, rhs));
                }
                Some(Token::IDiv) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IDiv, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_primary(&mut self) -> Result<ExprRef, String> {
        match self.peek() {
            Some(Token::ParenOpen) => {
                self.next();
                let node = self.parse_expr()?;
                self.expect_err(&Token::ParenClose)?;
                return Ok(node);
            }
            Some(Token::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                return match self.peek() {
                    Some(Token::ParenOpen) => {
                        // function call
                        self.next();
                        let args = self.parse_expr_list(vec![])?;
                        self.expect_err(&Token::ParenClose)?;
                        let args = self.add(Expr::Block(args));
                        Ok(self.add(Expr::Call(s, args)))
                    }
                    _ => {
                        // identifier
                        Ok(self.add(Expr::Identifier(s)))
                    }
                };
            }
            x => {
                let e = match x {
                    Some(&Token::UInt64(num)) => Ok(self.add(Expr::UInt64(num))),
                    Some(&Token::Int64(num)) => Ok(self.add(Expr::Int64(num))),
                    Some(Token::Integer(num)) => {
                        let integer = Expr::Int(num.clone());
                        Ok(self.add(integer))
                    }
                    Some(&Token::Null) => Ok(self.add(Expr::Null)),
                    x => return Err(format!("parse_primary: unexpected token {:?}", x)),
                };
                self.next();
                return e;
            }
        }
    }

    fn parse_expr_list(&mut self, mut args: Vec<ExprRef>) -> Result<Vec<ExprRef>, String> {
        match self.peek() {
            Some(Token::ParenClose) => return Ok(args),
            _ => (),
        }

        let expr = self.parse_expr();
        if expr.is_err() {
            // there is no expr in this context
            return Ok(args);
        }
        args.push(expr.unwrap());

        return match self.peek() {
            Some(Token::Comma) => {
                self.next();
                self.parse_expr_list(args)
            }
            Some(Token::ParenClose) => Ok(args),
            x => Err(format!("parse_expr_list: unexpected token {:?}", x)),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Token;
    use crate::Expr;

    #[test]
    fn lexer_simple_keyword() {
        let s = " if else while break continue for class fn val var";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap(), Token::If);
        assert_eq!(l.yylex().unwrap(), Token::Else);
        assert_eq!(l.yylex().unwrap(), Token::While);
        assert_eq!(l.yylex().unwrap(), Token::Break);
        assert_eq!(l.yylex().unwrap(), Token::Continue);
        assert_eq!(l.yylex().unwrap(), Token::For);
        assert_eq!(l.yylex().unwrap(), Token::Class);
        assert_eq!(l.yylex().unwrap(), Token::Function);
        assert_eq!(l.yylex().unwrap(), Token::Val);
        assert_eq!(l.yylex().unwrap(), Token::Var);
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
        let s = " ( ) { } [ ] , . :: : = !";
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
        assert_eq!(l.yylex().unwrap(), Token::Exclamation);
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
        assert_eq!(
            l.yylex().unwrap(),
            Token::Identifier("Identifier".to_string())
        );
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
        let mut p = Parser::new("1u64 + 2u64");
        let t1 = p.peek_n(1).unwrap();
        assert_eq!(Token::IAdd, *t1);
        assert_eq!(2, p.consume(2));

        let t2 = p.peek().unwrap();
        assert_eq!(Token::UInt64(2), *t2);
    }

    #[test]
    fn parser_simple_expr_test1() {
        let mut p = Parser::new("1u64 + 2u64 ");
        let _ = p.parse_stmt_line().unwrap();
        assert_eq!(3, p.len(), "ExprPool.len must be 3");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get(2).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);

        assert_eq!(1, p.inst_len(), "Inst.len must be 1");
        let d = p.get_inst(0).unwrap();
        assert_eq!(Inst::Expression(ExprRef(2)), *d);
    }

    #[test]
    fn parser_simple_expr_mul() {
        let mut p = Parser::new("(1u64) + 2u64 * 3u64");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(5, p.len(), "ExprPool.len must be 3");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get(2).unwrap();
        assert_eq!(Expr::UInt64(3), *c);

        let d = p.get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::IMul, ExprRef(1), ExprRef(2)), *d);
        let e = p.get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(3)), *e);
    }

    #[test]
    fn parser_simple_relational_expr() {
        let mut p = Parser::new("0u64 < 2u64 + 4u64");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(5, p.len(), "ExprPool.len must be 3");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(0), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get(2).unwrap();
        assert_eq!(Expr::UInt64(4), *c);

        let d = p.get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(1), ExprRef(2)), *d);
        let e = p.get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::LT, ExprRef(0), ExprRef(3)), *e);
    }

    #[test]
    fn parser_simple_logical_expr() {
        let mut p = Parser::new("1u64 && 2u64 < 3u64");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(5, p.len(), "ExprPool.len must be 3");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get(2).unwrap();
        assert_eq!(Expr::UInt64(3), *c);

        let d = p.get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::LT, ExprRef(1), ExprRef(2)), *d);
        let e = p.get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::LogicalAnd, ExprRef(0), ExprRef(3)), *e);
    }

    #[test]
    fn parser_expr_accept() {
        assert!(Parser::new("1u64").parse_stmt_line().is_ok());
        assert!(Parser::new("(1u64 + 2u64)").parse_stmt_line().is_ok());
        assert!(Parser::new("1u64 && 2u64 < 3u64").parse_stmt_line().is_ok());
        assert!(Parser::new("1u64 || 2u64 < 3u64").parse_stmt_line().is_ok());
        assert!(Parser::new("1u64 || (2u64) < 3u64 + 4u64")
            .parse_stmt_line()
            .is_ok());

        assert!(Parser::new("variable").parse_stmt_line().is_ok());
        assert!(Parser::new("a + b").parse_stmt_line().is_ok());
        assert!(Parser::new("a + 1u64").parse_stmt_line().is_ok());

        assert!(Parser::new("a() + 1u64").parse_stmt_line().is_ok());
        assert!(Parser::new("a(b,c) + 1u64").parse_stmt_line().is_ok());
    }

    #[test]
    fn parser_simple_ident_expr() {
        let mut p = Parser::new("abc + 1u64");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(3, p.len(), "ExprPool.len must be 3");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::Identifier("abc".to_string()), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(1), *b);

        let c = p.get(2).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);
    }

    #[test]
    fn parser_simple_apply_empty() {
        let mut p = Parser::new("abc()");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(2, p.len(), "ExprPool.len must be 2");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::Block(vec![]), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::Call("abc".to_string(), ExprRef(0)), *b);
    }

    #[test]
    fn parser_simple_apply_expr() {
        let mut p = Parser::new("abc(1u64, 2u64)");
        let _ = p.parse_stmt_line().unwrap();

        assert_eq!(4, p.len(), "ExprPool.len must be 4");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);

        let c = p.get(2).unwrap();
        assert_eq!(Expr::Block(vec![ExprRef(0), ExprRef(1)]), *c);
        let d = p.get(3).unwrap();
        assert_eq!(Expr::Call("abc".to_string(), ExprRef(2)), *d);
    }

    /*
    #[test]
    fn parser_simple_expr_null_value() {
        let res = Parser::new("null").parse_stmt_line().unwrap();
        assert_eq!(Expr::Null, res);
    }

    #[test]
    fn parser_simple_assign() {
        let res = Parser::new("a = 1u64").parse_stmt_line().unwrap();
        assert_eq!(
            Expr::Binary(Box::new(BinaryExpr {
                op: Operator::Assign,
                lhs: Expr::Identifier("a".to_string()),
                rhs: Expr::UInt64(1)
            })),
            res
        );
    }

    #[test]
    fn parser_err_primary() {
        let res = Parser::new(".").parse_stmt_line();
        assert!(res.is_err());
    }

    #[test]
    fn parser_err_call_expr_list() {
        let res = Parser::new("foo(a,,)").parse_stmt_line();
        assert!(res.is_err());
    }

    #[test]
    fn parser_val_simple_expr() {
        let res = Parser::new("val foo = 10u64").parse_stmt_line().unwrap();
        assert_eq!(
            Expr::Val(
                "foo".to_string(),
                Some(Type::Unknown),
                Some(Box::new(Expr::UInt64(10)))
            ),
            res
        );
    }

    #[test]
    fn parser_val_simple_expr_with_type() {
        let res = Parser::new("val foo: u64 = 30u64")
            .parse_stmt_line()
            .unwrap();
        assert_eq!(
            Expr::Val(
                "foo".to_string(),
                Some(Type::UInt64),
                Some(Box::new(Expr::UInt64(30)))
            ),
            res
        );
    }
    #[test]
    fn parser_val_simple_expr_without_type1() {
        let res = Parser::new("val foo = 20u64").parse_stmt_line().unwrap();
        assert_eq!(
            Expr::Val(
                "foo".to_string(),
                Some(Type::Unknown),
                Some(Box::new(Expr::UInt64(20)))
            ),
            res
        );
    }

    #[test]
    fn parser_val_simple_expr_without_type2() {
        let res = Parser::new("val foo: ty = 20u64")
            .parse_stmt_line()
            .unwrap();
        assert_eq!(
            Expr::Val(
                "foo".to_string(),
                Some(Type::Identifier("ty".to_string())),
                Some(Box::new(Expr::UInt64(20)))
            ),
            res
        );
    }

    #[test]
    fn parser_if_expr() {
        let res = Parser::new("if condition { }").parse_stmt_line().unwrap();
        assert_eq!(
            Expr::IfElse(
                Box::new(Expr::Identifier("condition".to_string())),
                vec![],
                vec![],
            ),
            res
        );
    }

    #[test]
    fn parser_if_else_expr() {
        let res = Parser::new("if condition { a } else { b }")
            .parse_stmt_line()
            .unwrap();
        assert_eq!(
            Expr::IfElse(
                Box::new(Expr::Identifier("condition".to_string())),
                vec![Expr::Identifier("a".to_string())],
                vec![Expr::Identifier("b".to_string())],
            ),
            res
        );
    }
     */
}
