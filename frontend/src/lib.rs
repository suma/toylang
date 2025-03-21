#![feature(box_patterns)]

pub mod ast;
pub mod type_decl;
pub mod token;
mod type_checker;

use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::{Token, Kind};

use anyhow::{anyhow, Result};

mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    pub lexer: lexer::Lexer<'a>,
    pub ahead: Vec<Token>,
    pub ast:   ExprPool,
}

#[derive(Debug)]
struct OperatorGroup<'a> {
    tokens: Vec<(Kind, Operator)>,
    next_precedence: fn(&mut Parser<'a>) -> Result<ExprRef>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let lexer = lexer::Lexer::new(&input, 1u64);
        Parser {
            lexer,
            ahead: Vec::new(),
            ast: ExprPool::with_capacity(1024),
        }
    }

    fn peek(&mut self) -> Option<&Kind> {
        if self.ahead.is_empty() {
            match self.lexer.yylex() {
                Ok(t) => {
                    self.ahead.push(t);
                    Some(&self.ahead.get(0).unwrap().kind)
                }
                _ => None,
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
    fn peek_position_n(&mut self, pos: usize) -> Option<&std::ops::Range<usize>> {
        while self.ahead.len() < pos + 1 {
            match self.lexer.yylex() {
                Ok(t) => self.ahead.push(t),
                _ => return None,
            }
        }
        match self.ahead.get(pos) {
            Some(t) => Some(&t.position),
            None => None,
        }
    }

    #[allow(dead_code)]
    fn consume(&mut self, count: usize) -> usize {
        self.ahead.drain(0..count).count()
    }

    fn next(&mut self) {
        self.ahead.remove(0);
    }

    pub fn expect(&mut self, accept: &Kind) -> bool {
        let tk = self.peek();
        if tk.is_some() && *tk.unwrap() == *accept {
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


    pub fn next_expr(&self) -> u32 {
        self.ast.len() as u32
    }

    // code := (import | fn)*
    // fn := "fn" identifier "(" param_def_list* ") "->" def_ty block
    // param_def_list := e | param_def | param_def "," param_def_list
    // param_def := identifier ":" def_ty |
    // prog := expr NewLine expr | expr | e
    // expr := logical_expr
    // block := "{" prog* "}"
    // if_expr := "if" expr block else_expr?
    // else_expr := "else" block
    // assign := val_def | identifier "=" logical_expr | logical_expr
    // val_def := ("val" | "var") identifier (":" def_ty)? ("=" logical_expr)
    // def_ty := Int64 | UInt64 | identifier | Unknown
    // logical_expr := equality ("&&" relational | "||" relational)*
    // equality := relational ("==" relational | "!=" relational)*
    // relational := add ("<" add | "<=" add | ">" add | ">=" add")*
    // add := mul ("+" mul | "-" mul)*
    // mul := primary ("*" mul | "/" mul)*
    // primary := identifier "(" expr_list ")" |
    //            identifier |
    //            UInt64 | Int64 | String | Null | "(" expr ")"
    // expr_list := "" | expr | expr "," expr_list

    // this function is for test
    pub fn parse_stmt_line(&mut self) -> Result<(ExprRef, ExprPool)> {
        let e = self.parse_expr();
        if e.is_err() {
            return Err(anyhow!(e.err().unwrap()));
        }
        let mut expr: ExprPool = ExprPool(vec![]);
        std::mem::swap(&mut expr, &mut self.ast);
        Ok((e?, expr))
    }

    pub fn parse_program(&mut self) -> Result<Program> {
        let mut start_pos: Option<usize> = None;
        let mut end_pos: Option<usize> = None;
        let mut update_start_pos = |start: usize| {
            if start_pos.is_none() || start_pos.unwrap() < start {
                start_pos = Some(start);
            }
        };
        let mut update_end_pos = |end: usize| {
            end_pos = Some(end);
        };
        let mut def_func = vec![];
        loop {
            match self.peek() {
                // Function definition
                Some(Kind::Function) => {
                    let fn_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(fn_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let fn_name = s.to_string();
                            self.next();

                            self.expect_err(&Kind::ParenOpen)?;
                            let params = self.parse_param_def_list(vec![])?;
                            self.expect_err(&Kind::ParenClose)?;
                            let mut ret_ty: Option<type_decl::TypeDecl> = None;
                            match self.peek() {
                                Some(Kind::Arrow) => {
                                    self.expect_err(&Kind::Arrow)?;
                                    ret_ty =  Some(self.parse_def_ty()?);
                                }
                                _ => (),
                            }
                            let block = self.parse_block()?;
                            let fn_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(fn_end_pos);
                            
                            def_func.push(Rc::new(Function{
                                node: Node::new(fn_start_pos, fn_start_pos),
                                name: fn_name,
                                parameter: params,
                                return_type: ret_ty,
                                code: block,
                            }));
                        }
                        _ => return Err(anyhow!("expected function")),
                    }
                }
                Some(Kind::NewLine) => {
                    // skip
                    self.next()
                }
                None | Some(Kind::EOF) => break,
                // import, etc...
                x => return Err(anyhow!("not implemented!!: {:?}", x)),
            }
        }
        // TODO: update end_position each element
        // TODO: handle Err
        let mut expr = ExprPool::new();
        std::mem::swap(&mut expr, &mut self.ast);
        Ok(Program{
            node: Node::new(start_pos.unwrap_or(0usize), end_pos.unwrap_or(0usize)),
            import: vec![],
            function: def_func,
            expression: expr,
        })
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
        args.push(def?);

        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_param_def_list(args)
            }
            // We expect Kind::ParenClose will appear
            // but other tokens can be accepted for testability
            _ => Ok(args),
        }
    }

    pub fn parse_expr(&mut self) -> Result<ExprRef> {
        return self.parse_logical_expr();
        /*
        match self.peek() {
            Some(Kind::If) => {
                self.next();
                self.parse_if()
            }
            Some(x) => {
                let x = x.clone();
                let line = *((&mut (self.lexer)).get_line_count());
                Err(anyhow!("parse_expr: expected expression but Kind ({:?}) at {}", x, line))
            }
            None => Err(anyhow!("parse_expr: unexpected EOF")),
        }
        */
    }
    pub fn parse_if(&mut self) -> Result<ExprRef> {
        let cond = self.parse_logical_expr()?;
        let if_block = self.parse_block()?;

        let else_block: ExprRef = match self.peek() {
            Some(Kind::Else) => {
                self.next();
                self.parse_block()?
            }
            _ => self.ast.add(Expr::Block(vec![])), // through
        };
        Ok(self.ast.add(Expr::IfElse(cond, if_block, else_block)))
    }

    pub fn parse_block(&mut self) -> Result<ExprRef> {
        self.expect_err(&Kind::BraceOpen)?;
        match self.peek() {
            Some(Kind::BraceClose) | None => {
                // empty block
                self.next();
                Ok(self.ast.add(Expr::Block(vec![])))
            }
            _ => {
                let block = self.parse_block_impl(vec![])?;
                self.expect_err(&Kind::BraceClose)?;
                Ok(self.ast.add(Expr::Block(block)))
            }
        }
    }

    // input multi expressions by lines
    pub fn parse_block_impl(&mut self, mut expressions: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
        // check end of expressions
        match self.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None =>
                return Ok(expressions),
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

        // check end of expressions again
        match self.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => {
                return Ok(expressions);
            }
            _ => (),
        }

        let lhs = self.parse_expr();
        if lhs.is_err() {
            return Err(anyhow!("parse_expression_block: expected expression: {:?}", lhs.err()));
        }
        expressions.push(lhs?);

        self.parse_block_impl(expressions)
    }

    pub fn parse_val_def(&mut self, val_or_var: &Kind) -> Result<ExprRef> {
        let ident: String = match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                s
            }
            x => return Err(anyhow!("parse_val_def: expected identifier but {:?}", x)),
        };

        let ty: TypeDecl = match self.peek() {
            Some(Kind::Colon) => {
                self.next();
                self.parse_def_ty()?
            }
            _ => TypeDecl::Unknown,
        };

        // "=" logical_expr
        let rhs = match self.peek() {
            Some(Kind::Equal) => {
                self.next();
                let expr = self.parse_logical_expr();
                eprintln!("rhs: {:?}", expr);
                if expr.is_err() {
                    return expr;
                }
                Some(expr.unwrap())
            }
            Some(Kind::NewLine) => None,
            _ => return Err(anyhow!("parse_val_def: expected expression but {:?}", self.peek())),
        };
        if let Kind::Val = val_or_var {
            Ok(self.ast.add(Expr::Val(ident, Some(ty), rhs)))
        } else {
            Ok(self.ast.add(Expr::Var(ident, Some(ty), rhs)))
        }
    }

    fn parse_def_ty(&mut self) -> Result<TypeDecl> {
        let ty: TypeDecl = match self.peek() {
            Some(Kind::Bool) => TypeDecl::Bool,
            Some(Kind::U64) => TypeDecl::UInt64,
            Some(Kind::I64) => TypeDecl::Int64,
            Some(Kind::Identifier(s)) => {
                let ident = s.to_string();
                TypeDecl::Identifier(ident)
            }
            Some(Kind::Str) => {
                TypeDecl::String
            }
            _ => TypeDecl::Unknown,
        };
        self.next();
        Ok(ty)
    }

    fn parse_logical_expr(&mut self) -> Result<ExprRef> {
        let group = OperatorGroup {
            tokens: vec![
                (Kind::DoubleAnd, Operator::LogicalAnd),
                (Kind::DoubleOr, Operator::LogicalOr),
            ],
            next_precedence: Self::parse_equality
        };
        self.parse_binary(&group)
    }

    fn parse_equality(&mut self) -> Result<ExprRef> {
        let group = OperatorGroup {
            tokens: vec![
                (Kind::DoubleEqual, Operator::EQ),
                (Kind::NotEqual, Operator::NE),
            ],
            next_precedence: Self::parse_relational
        };
        self.parse_binary(&group)
    }

    fn parse_relational(&mut self) -> Result<ExprRef> {
        let group = OperatorGroup {
            tokens: vec![
                (Kind::LT, Operator::LT),
                (Kind::LE, Operator::LE),
                (Kind::GT, Operator::GT),
                (Kind::GE, Operator::GE),
            ],
            next_precedence: Self::parse_add
        };
        self.parse_binary(&group)
    }

    fn parse_binary(&mut self, group: &OperatorGroup<'a>) -> Result<ExprRef> {
        let mut lhs = (group.next_precedence)(self)?;

        loop {
            let next_token = self.peek();
            let matched_op = group.tokens.iter()
                .find(|(kind, _)| next_token == Some(kind));

            match matched_op {
                Some((_, op)) => {
                    self.next();
                    let rhs = (group.next_precedence)(self)?;
                    lhs = self.ast.add(Self::new_binary(op.clone(), lhs, rhs));
                }
                None => return Ok(lhs),
            }
        }
    }

    pub fn parse_add(&mut self) -> Result<ExprRef> {
        let group = OperatorGroup {
            tokens: vec![
                (Kind::IAdd, Operator::IAdd),
                (Kind::ISub, Operator::ISub),
            ],
            next_precedence: Self::parse_mul
        };
        self.parse_binary(&group)
    }

    pub fn parse_mul(&mut self) -> Result<ExprRef> {
        let group = OperatorGroup {
            tokens: vec![
                (Kind::IMul, Operator::IMul),
                (Kind::IDiv, Operator::IDiv),
            ],
            next_precedence: Self::parse_primary,
        };
        self.parse_binary(&group)
    }

    fn parse_primary(&mut self) -> Result<ExprRef> {
        match self.peek() {
            Some(Kind::Return) => {
                self.next();
                match self.peek() {
                    Some(&Kind::NewLine) | Some(&Kind::BracketClose) => {
                        self.next();
                        Ok(self.ast.add(Expr::Return(None)))
                    }
                    Some(_expr) => {
                        let expr = self.parse_expr()?;
                        Ok(self.ast.add(Expr::Return(Some(expr))))
                    }
                    None => Err(anyhow!("parse_primary: expected expression")),
                }
            }
            Some(Kind::ParenOpen) => {
                self.next();
                let node = self.parse_expr()?;
                self.expect_err(&Kind::ParenClose)?;
                Ok(node)
            }
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                self.next();
                match self.peek() {
                    Some(Kind::ParenOpen) => { // function call
                        self.next();
                        let args = self.parse_expr_list(vec![])?;
                        self.expect_err(&Kind::ParenClose)?;
                        let args = self.ast.add(Expr::ExprList(args));
                        let expr = self.ast.add(Expr::Call(s, args));
                        Ok(expr)
                    }
                    _ => {
                        // identifier
                        Ok(self.ast.add(Expr::Identifier(s)))
                    }
                }
            }
            x => {
                let e = Ok(match x {
                    Some(&Kind::UInt64(num)) => self.ast.add(Expr::UInt64(num)),
                    Some(&Kind::Int64(num)) => self.ast.add(Expr::Int64(num)),
                    Some(&Kind::Null) => self.ast.add(Expr::Null),
                    Some(&Kind::True) => self.ast.add(Expr::True),
                    Some(&Kind::False) => self.ast.add(Expr::False),
                    Some(Kind::String(str)) => {
                        // TODO: optimizing with string interning
                        let s = str.clone();
                        self.ast.add(Expr::String(s))
                    }
                    x => {
                        match x {
                            Some(Kind::ParenOpen) => {
                                self.next();
                                let e = self.parse_expr()?;
                                self.expect_err(&Kind::ParenClose)?;
                                return Ok(e);
                            }
                            _ => {
                                return Err(anyhow!("parse_primary: unexpected token {:?}", x));
                            }
                        }
                    }
                });
                self.next();
                e
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
        args.push(expr?);

        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_expr_list(args)
            }
            Some(Kind::ParenClose) => Ok(args),
            x => Err(anyhow!("parse_expr_list: unexpected token {:?}", x)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;
    use std::path::PathBuf;
    use super::*;
    use rstest::rstest;
    use crate::type_checker::{type_check, TypeCheckContext};

    #[test]
    fn lexer_simple_keyword() {
        let s = " if else while break continue return for class fn val var bool";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::If);
        assert_eq!(l.yylex().unwrap().kind, Kind::Else);
        assert_eq!(l.yylex().unwrap().kind, Kind::While);
        assert_eq!(l.yylex().unwrap().kind, Kind::Break);
        assert_eq!(l.yylex().unwrap().kind, Kind::Continue);
        assert_eq!(l.yylex().unwrap().kind, Kind::Return);
        assert_eq!(l.yylex().unwrap().kind, Kind::For);
        assert_eq!(l.yylex().unwrap().kind, Kind::Class);
        assert_eq!(l.yylex().unwrap().kind, Kind::Function);
        assert_eq!(l.yylex().unwrap().kind, Kind::Val);
        assert_eq!(l.yylex().unwrap().kind, Kind::Var);
        assert_eq!(l.yylex().unwrap().kind, Kind::Bool);
    }

    #[test]
    fn lexer_simple_integer() {
        let s = " -1i64 1i64 2u64  true false null";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(-1));
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(1));
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(2u64));
        assert_eq!(l.yylex().unwrap().kind, Kind::True);
        assert_eq!(l.yylex().unwrap().kind, Kind::False);
        assert_eq!(l.yylex().unwrap().kind, Kind::Null);
    }

    #[test]
    fn lexer_simple_string() {
        let s = " \"string\" ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::String("string".to_string()));
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
    fn lexer_simple_number() {
        let s = " 100u64 123i64 ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(100));
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(123));
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
        let e = p.parse_stmt_line();
        assert!(e.is_ok());
        let (_, p) = e.unwrap();

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
        let e = p.parse_stmt_line();
        assert!(e.is_ok());
        let (_, p) = e.unwrap();

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
        let e = p.parse_stmt_line();
        assert!(e.is_ok());
        let (_, p) = e.unwrap();

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
        let expr_str = vec!["1u64", "(1u64 + 2u64)", "1u64 && 2u64 < 3u64", "1u64 || 2u64 < 3u64", "1u64 || (2u64) < 3u64 + 4u64",
            "variable", "a + b", "a + 1u64", "a() + 1u64", "a(b,c) + 1u64"];
        for input in expr_str {
            let mut p = Parser::new(input);
            let e = p.parse_stmt_line();
            assert!(e.is_ok(), "failed: {}", input);
        }
    }

    #[test]
    fn parser_simple_ident_expr() {
        let mut p = Parser::new("abc + 1u64");
        let e = p.parse_stmt_line();
        assert!(e.is_ok());
        let (_, p) = e.unwrap();

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
        let e = p.parse_stmt_line();
        assert!(e.is_ok());
        let (_, p) = e.unwrap();

        assert_eq!(2, p.len(), "ExprPool.len must be 2");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::ExprList(vec![]), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::Call("abc".to_string(), ExprRef(0)), *b);
    }

    /*
    #[test]
    fn parser_simple_apply_expr() {
        let mut p = Parser::new("abc(1u64, 2u64)");
        let e = p.parse_stmt_line();
        assert!(e.is_ok(), "{:?}", p.ast);
        let (_, p) = e.unwrap();

        assert_eq!(4, p.len(), "ExprPool.len must be 4");
        let a = p.get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);

        let c = p.get(2).unwrap();
        assert_eq!(Expr::ExprList(vec![ExprRef(0), ExprRef(1)]), *c);
        let d = p.get(3).unwrap();
        assert_eq!(Expr::Call("abc".to_string(), ExprRef(2)), *d);
    }
    */

    #[test]
    fn parser_param_def() {
        let param = Parser::new("test: u64").parse_param_def();
        assert!(param.is_ok());
        let p = param.unwrap();
        assert_eq!(("test".to_string(), TypeDecl::UInt64), p);
    }

    #[test]
    fn parser_param_def_list_empty() {
        let param = Parser::new("").parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let p = param.unwrap();
        assert_eq!(0, p.len());
    }

    #[test]
    fn parser_param_def_list() {
        let param = Parser::new("test: u64, test2: i64, test3: some_type").parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let p = param.unwrap();
        assert_eq!(
            vec![
                ("test".to_string(), TypeDecl::UInt64),
                ("test2".to_string(), TypeDecl::Int64),
                ("test3".to_string(), TypeDecl::Identifier("some_type".to_string())),
            ],
            p
        );
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
        let result = p.parse_program();
        assert!(result.is_ok());
        let prog = result.unwrap();
        assert_eq!(3, prog.function.len());

        assert_eq!(Function{node: Node::new(1, 1), name: "hello".to_string(),
            parameter: vec![], return_type: Some(TypeDecl::UInt64), code: ExprRef(2)}, *prog.function[0]);

        // hello, hello2, hello3 blocks

        let mut blocks = vec![];
        for func in &prog.function {
            blocks.push(prog.get_block(func.code.0).unwrap());
            println!("Func {}", func.name);
        }

        let block0 = blocks.get(0).unwrap();
        assert_eq!("hello".to_string(), prog.function[0].name);
        assert_eq!(0, prog.function[0].parameter.len());
        assert_eq!(
            vec![&Expr::Identifier("a".to_string()), &Expr::Identifier("b".to_string())],
            block0.clone()
        );

        assert_eq!("hello2".to_string(), prog.function[1].name);
        assert_eq!(vec![("a".to_string(), TypeDecl::UInt64)],
                   prog.function[1].parameter);
        let block1 = blocks.get(1).unwrap();
        assert_eq!(
            vec![&Expr::Identifier("b".to_string())],
            block1.clone()
        );

        assert_eq!("hello3".to_string(), prog.function[2].name);
        assert_eq!(vec![("a".to_string(), TypeDecl::UInt64), ("b".to_string(), TypeDecl::UInt64)],
                   prog.function[2].parameter);
        let block2 = blocks.get(2).unwrap();
        assert_eq!(
            vec![&Expr::Identifier("c".to_string())],
            block2.clone()
        );
    }

    #[rstest]
    fn syntax_test(#[files("tests/syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = Parser::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let program = result.unwrap();
        let mut ctx = TypeCheckContext::new();

        let println_fun = Rc::new(ast::Function {
            node: Node::new(0, 0),
            name: "println".to_string(),
            parameter: vec![],
            return_type: Some(TypeDecl::Unit),
            code: ExprRef(0),   // not found
        });
        ctx.set_fn("println", println_fun);
        let ast = program.expression;

        program.function.iter().for_each(|f| {
            let res = type_check(&ast, f.code, &mut ctx);
            assert!(res.is_ok(), "type check err {:?}", res.err().unwrap());
        });
    }

    #[rstest]
    fn syntax_error_test(#[files("tests/err_syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = Parser::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let program = result.unwrap();
        let mut ctx = TypeCheckContext::new();

        let ast = program.expression;
        let mut res = true;
        program.function.iter().for_each(|f| {
            let r = type_check(&ast, f.code, &mut ctx);
            if r.is_err() {
                res = false;
            }
        });

        assert!(!res, "{:?}: type check should fail", path.to_str().unwrap());
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
