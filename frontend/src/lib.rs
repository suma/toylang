pub mod ast;
pub mod token;
use crate::ast::*;
use crate::token::{Token, Kind};

use anyhow::{anyhow, Result};

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

    fn peek(&mut self) -> Option<&Kind> {
        if self.ahead.is_empty() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.ahead.push(t);
                    Some(&self.ahead.get(0).unwrap().kind)
                }
                _ => return None,
            }
        } else {
            match self.ahead.get(0) {
                Some(t) => Some(&t.kind),
                None => None,
            }
        }
    }

    // pos: 0-origin
    #[allow(dead_code)]
    fn peek_n(&mut self, pos: usize) -> Option<&Kind> {
        while self.ahead.len() < pos + 1 {
            match self.lexer.yylex() {
                Ok(t) => self.ahead.push(t),
                _ => return None,
            }
        }
        match self.ahead.get(pos) {
            Some(t) => Some(&t.kind),
            None => None,
        }
    }

    #[allow(dead_code)]
    fn consume(&mut self, count: usize) -> usize {
        return self.ahead.drain(0..count).count();
    }

    fn next(&mut self) {
        self.ahead.remove(0);
    }

    pub fn expect(&mut self, accept: &Kind) -> bool {
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

    pub fn expect_err(&mut self, accept: &Kind) -> Result<()> {
        if !self.expect(accept) {
            return Err(anyhow!("{:?} expected but {:?}", accept, self.ahead.get(0)));
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

    pub fn get_block(&self, i: u32) -> Option<Vec<&Expr>> {
        let mut exprs_blk: Vec<&Expr> = vec![];
        match self.get(i) {
            Some(e) => match e {
                Expr::Block(exprs) => {
                    exprs.iter().for_each(|x| exprs_blk.push(&self.get(x.0).unwrap()));
                }
                _ => return Option::None,
            }
            _ => return Option::None,
        }
        return Some(exprs_blk);
    }

    pub fn len(&self) -> usize {
        return self.ast.0.len();
    }

    pub fn next_expr(&self) -> u32 {
        return self.len() as u32;
    }

    // code := (import | fn)*
    // fn := "fn" identifier "(" param_def_list* ") "->" def_ty block
    // param_def_list := e | param_def | param_def "," param_def_list
    // param_def := identifier ":" def_ty |
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

    // this function is for test
    pub fn parse_stmt_line(&mut self) -> Result<ExprRef> {
        let expr = self.parse_some_exprs(vec![])?;
        if expr.len() != 1 {
            return Err(anyhow!("parse_stmt_line: expected 1 expression but {:?} length", expr.len()));
        }
        self.inst.push(Inst::Expression(ExprRef(self.next_expr() - 1)));
        Ok(*expr.first().unwrap())
    }

    pub fn parse_code(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(Kind::Function) => {
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let fn_name = s.to_string();
                            self.next();

                            self.expect_err(&Kind::ParenOpen)?;
                            let params = self.parse_param_def_list(vec![])?;
                            self.expect_err(&Kind::ParenClose)?;
                            self.expect_err(&Kind::Arrow)?;
                            let ret_ty = self.parse_def_ty()?;
                            let block = self.parse_block();
                            let block = block.unwrap();
                            self.add_inst(Inst::Function(fn_name, params, Some(ret_ty), block));
                        }
                        _ => return Err(anyhow!("expected function")),
                    }
                }
                Some(Kind::NewLine) => self.next(), // skip
                None | Some(Kind::EOF) => break,
                // import, etc...
                x => return Err(anyhow!("not implemented!!: {:?}", x)),
            }
        }
       return Ok(());// FIXME: return Err
    }

    pub fn parse_param_def(&mut self) -> Result<Parameter> {
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let name = s.to_string();
                self.next();
                self.expect_err(&Kind::Colon)?;
                let typ = self.parse_def_ty()?;
                Ok((name, typ))
            }
            x => Err(anyhow!("expect type parameter of function but: {:?}", x)),
        }
    }

    fn parse_param_def_list(&mut self, mut args: Vec<Parameter>) -> Result<Vec<Parameter>> {
        match self.peek() {
            Some(Kind::ParenClose) => return Ok(args),
            _ => (),
        }

        let def = self.parse_param_def();
        if def.is_err() {
            // there is no expr in this context
            return Ok(args);
        }
        args.push(def.unwrap());

        return match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_param_def_list(args)
            }
            // We expect Kind::ParenClose will appearr
            // but other tokens can be accepted for testability
            _ => Ok(args),
        };
    }

    // input multi expressions by lines
    pub fn parse_some_exprs(&mut self, mut exprs: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
        // check end of expressions
        match self.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None =>
                return Ok(exprs),
            _ => (),
        }

        // remove unused NewLine
        loop {
            match self.peek() {
                Some(Kind::NewLine) =>
                    self.next(),
                Some(_) | None =>
                    break,
            }
        }

        // check end of expressions (twice)
        match self.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None =>
                return Ok(exprs),
            _ => (),
        }

        let lhs = self.parse_expr();
        if lhs.is_err() {
            return Err(anyhow!("parse_some_exprs: expected expression: {:?}", lhs.err()));
        }
        exprs.push(lhs.unwrap());

        return self.parse_some_exprs(exprs);
    }

    pub fn parse_expr(&mut self) -> Result<ExprRef> {
        let assign = self.parse_assign();
        if assign.is_ok() {
            return assign;
        }

        match self.peek() {
            Some(Kind::If) => {
                self.next();
                return self.parse_if();
            }
            Some(Kind::Val) => {
                self.next();
                return self.parse_val_def();
            }
            Some(x) => {
                return Err(anyhow!("parse_expr: expected expression but Kind ({:?})", x));
            }
            None => {
                return Err(anyhow!("parse_expr: expected expression but None"));
            }
        }
    }

    pub fn parse_assign(&mut self) -> Result<ExprRef> {
        match self.peek() {
            Some(Kind::Val) => {
                self.next();
                return self.parse_val_def();
            }
            _ => {
                let lhs = self.parse_logical_expr()?;
                match self.peek() {
                    Some(Kind::Equal) => {
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

    pub fn parse_if(&mut self) -> Result<ExprRef> {
        let cond = self.parse_logical_expr()?;
        let if_block = self.parse_block()?;

        let else_block: ExprRef = match self.peek() {
            Some(Kind::Else) => {
                self.next();
                self.parse_block()?
            }
            _ => self.add(Expr::Block(vec![])), // through
        };
        return Ok(self.add(Expr::IfElse(cond, if_block, else_block)));
    }

    pub fn parse_block(&mut self) -> Result<ExprRef> {
        self.expect_err(&Kind::BraceOpen)?;
        match self.peek() {
            Some(Kind::BraceClose) => {
                // empty block
                self.next();
                Ok(self.add(Expr::Block(vec![])))
            }
            _ => {
                let block = self.parse_some_exprs(vec![])?;
                self.expect_err(&Kind::BraceClose)?;
                Ok(self.add(Expr::Block(block)))
            }
        }
    }

    pub fn parse_val_def(&mut self) -> Result<ExprRef> {
        let ident: String = match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                s
            }
            x => return Err(anyhow!("parse_val_def: expected identifier but {:?}", x)),
        };

        let ty: Type = match self.peek() {
            Some(Kind::Colon) => {
                self.next();
                self.parse_def_ty()?
            }
            _ => Type::Unknown,
        };

        // "=" logical_expr
        let rhs = match self.peek() {
            Some(Kind::Equal) => {
                self.next();
                Some(self.parse_logical_expr()?)
            }
            _ => None,
        };
        return Ok(self.add(Expr::Val(ident, Some(ty), rhs)));
    }

    fn parse_def_ty(&mut self) -> Result<Type> {
        let ty: Type = match self.peek() {
            Some(Kind::U64) => Type::UInt64,
            Some(Kind::I64) => Type::Int64,
            Some(Kind::Identifier(s)) => {
                let ident = s.to_string();
                Type::Identifier(ident)
            }
            _ => Type::Unknown,
        };
        self.next();
        return Ok(ty);
    }

    fn parse_logical_expr(&mut self) -> Result<ExprRef> {
        let mut lhs = self.parse_equality()?;

        loop {
            match self.peek() {
                Some(Kind::DoubleAnd) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::LogicalAnd, lhs, rhs));
                }
                Some(Kind::DoubleOr) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::LogicalOr, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_equality(&mut self) -> Result<ExprRef> {
        let mut lhs = self.parse_relational()?;

        loop {
            match self.peek() {
                Some(Kind::DoubleEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::EQ, lhs, rhs));
                }
                Some(Kind::NotEqual) => {
                    self.next();
                    let rhs = self.parse_relational()?;
                    lhs = self.add(Self::new_binary(Operator::NE, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_relational(&mut self) -> Result<ExprRef> {
        let mut lhs = self.parse_add()?;

        loop {
            match self.peek() {
                Some(Kind::LT) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::LT, lhs, rhs));
                }
                Some(Kind::LE) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::LE, lhs, rhs));
                }
                Some(Kind::GT) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::GT, lhs, rhs));
                }
                Some(Kind::GE) => {
                    self.next();
                    let rhs = self.parse_add()?;
                    lhs = self.add(Self::new_binary(Operator::GE, lhs, rhs))
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_add(&mut self) -> Result<ExprRef> {
        let mut lhs = self.parse_mul()?;

        loop {
            match self.peek() {
                Some(Kind::IAdd) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IAdd, lhs, rhs));
                }
                Some(Kind::ISub) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::ISub, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_mul(&mut self) -> Result<ExprRef> {
        let mut lhs = self.parse_primary()?;

        loop {
            match self.peek() {
                Some(Kind::IMul) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IMul, lhs, rhs));
                }
                Some(Kind::IDiv) => {
                    self.next();
                    let rhs = self.parse_mul()?;
                    lhs = self.add(Self::new_binary(Operator::IDiv, lhs, rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_primary(&mut self) -> Result<ExprRef> {
        match self.peek() {
            Some(Kind::ParenOpen) => {
                self.next();
                let node = self.parse_expr()?;
                self.expect_err(&Kind::ParenClose)?;
                return Ok(node);
            }
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                return match self.peek() {
                    Some(Kind::ParenOpen) => {
                        // function call
                        self.next();
                        let args = self.parse_expr_list(vec![])?;
                        self.expect_err(&Kind::ParenClose)?;
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
                    Some(&Kind::UInt64(num)) => Ok(self.add(Expr::UInt64(num))),
                    Some(&Kind::Int64(num)) => Ok(self.add(Expr::Int64(num))),
                    Some(Kind::Integer(num)) => {
                        let integer = Expr::Int(num.clone());
                        Ok(self.add(integer))
                    }
                    Some(&Kind::Null) => Ok(self.add(Expr::Null)),
                    x => return Err(anyhow!("parse_primary: unexpected token {:?}", x)),
                };
                self.next();
                return e;
            }
        }
    }

    fn parse_expr_list(&mut self, mut args: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
        match self.peek() {
            Some(Kind::ParenClose) => return Ok(args),
            _ => (),
        }

        let expr = self.parse_expr();
        if expr.is_err() {
            // there is no expr in this context
            return Ok(args);
        }
        args.push(expr.unwrap());

        return match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_expr_list(args)
            }
            Some(Kind::ParenClose) => Ok(args),
            x => Err(anyhow!("parse_expr_list: unexpected token {:?}", x)),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_simple_keyword() {
        let s = " if else while break continue for class fn val var";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::If);
        assert_eq!(l.yylex().unwrap().kind, Kind::Else);
        assert_eq!(l.yylex().unwrap().kind, Kind::While);
        assert_eq!(l.yylex().unwrap().kind, Kind::Break);
        assert_eq!(l.yylex().unwrap().kind, Kind::Continue);
        assert_eq!(l.yylex().unwrap().kind, Kind::For);
        assert_eq!(l.yylex().unwrap().kind, Kind::Class);
        assert_eq!(l.yylex().unwrap().kind, Kind::Function);
        assert_eq!(l.yylex().unwrap().kind, Kind::Val);
        assert_eq!(l.yylex().unwrap().kind, Kind::Var);
    }

    #[test]
    fn lexer_simple_integer() {
        let s = " -1i64 1i64 2u64 123 -456";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(-1));
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(1));
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(2u64));
        assert_eq!(l.yylex().unwrap().kind, Kind::Integer("123".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::Integer("-456".to_string()));
    }

    #[test]
    fn lexer_simple_symbol1() {
        let s = " ( ) { } [ ] , . :: : = !";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::ParenOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::ParenClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::BraceOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::BraceClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::BracketOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::BracketClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::Comma);
        assert_eq!(l.yylex().unwrap().kind, Kind::Dot);
        assert_eq!(l.yylex().unwrap().kind, Kind::DoubleColon);
        assert_eq!(l.yylex().unwrap().kind, Kind::Colon);
        assert_eq!(l.yylex().unwrap().kind, Kind::Equal);
        assert_eq!(l.yylex().unwrap().kind, Kind::Exclamation);
    }

    #[test]
    fn lexer_simple_symbol2() {
        let s = "== != <= < >= >";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::DoubleEqual);
        assert_eq!(l.yylex().unwrap().kind, Kind::NotEqual);
        assert_eq!(l.yylex().unwrap().kind, Kind::LE);
        assert_eq!(l.yylex().unwrap().kind, Kind::LT);
        assert_eq!(l.yylex().unwrap().kind, Kind::GE);
        assert_eq!(l.yylex().unwrap().kind, Kind::GT);
    }

    #[test]
    fn lexer_arithmetic_operator_symbol() {
        let s = " + - * / +. -. *. /.";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::IAdd);
        assert_eq!(l.yylex().unwrap().kind, Kind::ISub);
        assert_eq!(l.yylex().unwrap().kind, Kind::IMul);
        assert_eq!(l.yylex().unwrap().kind, Kind::IDiv);
        assert_eq!(l.yylex().unwrap().kind, Kind::FAdd);
        assert_eq!(l.yylex().unwrap().kind, Kind::FSub);
        assert_eq!(l.yylex().unwrap().kind, Kind::FMul);
        assert_eq!(l.yylex().unwrap().kind, Kind::FDiv);
    }

    #[test]
    fn lexer_simple_identifier() {
        let s = " A _name Identifier ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("_name".to_string()));
        assert_eq!(
            l.yylex().unwrap().kind,
            Kind::Identifier("Identifier".to_string())
        );
    }

    #[test]
    fn lexer_multiple_lines() {
        let s = " A \n B ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::NewLine);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("B".to_string()));
        assert_eq!(*l.get_line_count(), 2);
    }

    #[test]
    fn parser_util_lookahead() {
        let mut p = Parser::new("1u64 + 2u64");
        let t0 = p.peek_n(0).unwrap().clone();
        let t1 = p.peek_n(1).unwrap().clone();
        assert_eq!(Kind::UInt64(1), t0);
        assert_eq!(Kind::IAdd, t1);
        assert_eq!(2, p.consume(2));

        let t2 = p.peek().unwrap();
        assert_eq!(Kind::UInt64(2), *t2);
    }

    /*
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

        println!("p.inst: {:?}", p.inst);
        println!("INSTRUCTION {:?}", p.get_inst(0));
        println!("INSTRUCTION {:?}", p.get_inst(1));
        assert_eq!(1, p.inst_len(), "Inst.len must be 1");

        let d = p.get_inst(0).unwrap();
        assert_eq!(Inst::Expression(ExprRef(2)), *d);
    }
   */

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
        let res = p.parse_stmt_line();
        println!("{:?}", res.err());

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

    #[test]
    fn parser_param_def() {
        let param = Parser::new("test: u64").parse_param_def();
        let p = param.unwrap();
        assert_eq!(("test".to_string(), Type::UInt64), p);
    }

    #[test]
    fn parser_param_def_list_empty() {
        let param = Parser::new("").parse_param_def_list(vec![]);
        let p = param.unwrap();
        assert_eq!(0, p.len());
    }

    #[test]
    fn parser_param_def_list() {
        let param = Parser::new("test: u64, test2: i64, test3: some_type").parse_param_def_list(vec![]);
        let p = param.unwrap();
        assert_eq!(
            vec![
                ("test".to_string(), Type::UInt64),
                ("test2".to_string(), Type::Int64),
                ("test3".to_string(), Type::Identifier("some_type".to_string())),
            ],
            p
        );
    }

    #[test]
    fn parser_simple_error() {
        let result = Parser::new("++").parse_stmt_line();
        assert!(result.is_err());

        if let Err(e) = result {
            println!("{}", e);
        }
    }

    #[test]
    fn parser_input_code() {
        let code = r#"
fn hello() -> u64 {
a
b
}

fn hello2(a: u64) -> u64 {
b
}

fn hello3(a: u64, b: u64) -> u64 {
c
}
        "#;
        let mut p = Parser::new(code);
        let result = p.parse_code();
        assert!(result.is_ok());
        assert_eq!(3, p.inst_len());
        assert_eq!(&Inst::Function("hello".to_string(), vec![], Some(Type::UInt64), ExprRef(2)),
                   p.get_inst(0).unwrap());

        // hello, hello2, hello3 blocks
        let mut blocks: Vec<Option<Vec<&Expr>>> = vec![];
        for inst in p.inst_iter() {
            match inst {
                Inst::Import(_) => (),
                Inst::Expression(_) => (),
                Inst::Function(str, param, result_type, block) => {
                    let block = p.get_block(block.0);
                    blocks.push(block);
                    println!("Func {} {:?}", str, blocks.last());
                }
            }
        }

        let block0 = blocks.get(0).unwrap();
        assert_eq!(
            vec![&Expr::Identifier("a".to_string()), &Expr::Identifier("b".to_string())],
            block0.clone().unwrap()
        );

        let block1 = blocks.get(1).unwrap();
        assert_eq!(
            vec![&Expr::Identifier("b".to_string())],
            block1.clone().unwrap()
        );

        let block2 = blocks.get(2).unwrap();
        assert_eq!(
            vec![&Expr::Identifier("c".to_string())],
            block2.clone().unwrap()
        );
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
