pub mod ast;
pub mod type_decl;
pub mod token;
pub mod type_checker;
mod visitor;

use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::{Token, Kind};

use anyhow::{anyhow, Result};
use string_interner::{DefaultStringInterner, DefaultSymbol};

mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    pub lexer: lexer::Lexer<'a>,
    pub ahead: Vec<Token>,
    pub stmt:  StmtPool,
    pub expr:  ExprPool,
    pub string_interner: DefaultStringInterner,
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
            stmt: StmtPool::with_capacity(1024),
            expr: ExprPool::with_capacity(1024),
            string_interner: DefaultStringInterner::new(),
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
        self.expr.len() as u32
    }

    // code := (import | fn)*
    // fn := "fn" identifier "(" param_def_list* ") "->" def_ty block
    // param_def_list := e | param_def | param_def "," param_def_list
    // param_def := identifier ":" def_ty |
    // prog := expr NewLine expr | expr | e
    // stmt := var_def_stmt |
    //         break | continue |
    //         for_stmt |
    //         while_stmt |
    //         "return" expr? |
    //         expr
    // expr := logical_expr |
    //         assign_expr |
    //         if_expr |
    // assign_expr := logical_expr ("=" assign_expr)*
    // block := "{" prog* "}"
    // var_def_stmt := ("val" | "var") identifier (":" def_ty)? ("=" logical_expr)
    // for_stmt := "for" identifier in logical_expr to logical_expr block
    // if_expr := "if" expr block else_expr?
    // else_expr := "else" block
    // def_ty := Int64 | UInt64 | identifier | Unknown
    // logical_expr := equality ("&&" relational | "||" relational)*
    // equality := relational ("==" relational | "!=" relational)*
    // relational := add ("<" add | "<=" add | ">" add | ">=" add")*
    // add := mul ("+" mul | "-" mul)*
    // mul := primary ("*" mul | "/" mul)*
    // primary := identifier "(" expr_list ")" |
    //            identifier |
    //            UInt64 | Int64 | String | Null | "(" expr ")" | "{" block "}"
    // expr_list := "" | expr | expr "," expr_list

    // this function is for test
    pub fn parse_stmt_line(&mut self) -> Result<(StmtRef, StmtPool)> {
        let e = self.parse_stmt();
        if e.is_err() {
            return Err(anyhow!(e.err().unwrap()));
        }
        let mut stmt: StmtPool = StmtPool(vec![]);
        std::mem::swap(&mut stmt, &mut self.stmt);
        Ok((e?, stmt))
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
                            let s = s.to_string();
                            let fn_name = self.string_interner.get_or_intern(s);
                            self.next();

                            self.expect_err(&Kind::ParenOpen)?;
                            let params = self.parse_param_def_list(vec![])?;
                            self.expect_err(&Kind::ParenClose)?;
                            let mut ret_ty: Option<type_decl::TypeDecl> = None;
                            match self.peek() {
                                Some(Kind::Arrow) => {
                                    self.expect_err(&Kind::Arrow)?;
                                    ret_ty =  Some(self.parse_type_declaration()?);
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
                                code: self.stmt.add(Stmt::Expression(block)),
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
        let mut stmt = StmtPool::new();
        std::mem::swap(&mut stmt, &mut self.stmt);
        let mut expr = ExprPool::new();
        std::mem::swap(&mut expr, &mut self.expr);
        let mut string_interner = DefaultStringInterner::new();
        std::mem::swap(&mut string_interner, &mut self.string_interner);
        Ok(Program{
            node: Node::new(start_pos.unwrap_or(0usize), end_pos.unwrap_or(0usize)),
            import: vec![],
            function: def_func,
            statement: stmt,
            expression: expr,
            string_interner,
        })
    }

    pub fn parse_param_def(&mut self) -> Result<Parameter> {
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let name = self.string_interner.get_or_intern(s);
                self.next();
                self.expect_err(&Kind::Colon)?;
                let typ = self.parse_type_declaration()?;
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

    pub fn parse_stmt(&mut self) -> Result<StmtRef> {
        match self.peek() {
            Some(Kind::Val) | Some(Kind::Var) => {
                self.parse_var_def()
            }
            Some(Kind::Break) => {
                self.next();
                Ok(self.stmt.add(Stmt::Break))
            }
            Some(Kind::Continue) => {
                self.next();
                Ok(self.stmt.add(Stmt::Continue))
            }
            Some(Kind::Return) => {
                self.next();
                match self.peek() {
                    Some(&Kind::NewLine) | Some(&Kind::BracketClose) | Some(Kind::EOF) => {
                        self.next();
                        Ok(self.stmt.add(Stmt::Return(None)))
                    }
                    // Usually None is error but we treat this case for unit test.
                    None => Ok(self.stmt.add(Stmt::Return(None))),
                    Some(_expr) => {
                        let expr = self.parse_expr_impl()?;
                        Ok(self.stmt.add(Stmt::Return(Some(expr))))
                    }
                }
            }
            Some(Kind::For) => {
                // e.g. `for x in 0 to 100 { println("hello") }`
                self.next();
                match self.peek() {
                    Some(Kind::Identifier(s)) => {
                        let s = s.to_string();
                        let ident = self.string_interner.get_or_intern(s);
                        self.next();
                        self.expect_err(&Kind::In)?;
                        let start = self.parse_relational()?;
                        self.expect_err(&Kind::To)?;
                        let end = self.parse_relational()?;
                        let block = self.parse_block()?;
                        Ok(self.stmt.add(Stmt::For(ident, start, end, block)))
                    }
                    x => Err(anyhow!("parse_stmt for: expected identifier but {:?}", x)),
                }
            }
            Some(Kind::While) => {
                self.next();
                let cond = self.parse_logical_expr()?;
                let block = self.parse_block()?;
                Ok(self.stmt.add(Stmt::While(cond, block)))
            }
            _ => self.parse_expr(),
        }
    }

    fn expr_to_stmt(&mut self, e: ExprRef) -> StmtRef {
        self.stmt.add(Stmt::Expression(e))
    }

    pub fn parse_expr(&mut self) -> Result<StmtRef> {
        let e = self.parse_expr_impl();
        if e.is_err() {
            return Err(e.err().unwrap());
        }
        Ok(self.expr_to_stmt(e.unwrap()))
    }

    pub fn parse_expr_impl(&mut self) -> Result<ExprRef> {
        let lhs = self.parse_logical_expr();
        if lhs.is_ok() {
            return match self.peek() {
                Some(Kind::Equal) => {
                    // don't consume current Kind::Equal token. Consume in next parse_assign function.
                    self.parse_assign(lhs?)
                }
                _ => lhs,
            };
        }

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
    }

    pub fn parse_assign(&mut self, mut lhs: ExprRef) -> Result<ExprRef> {
        loop {
            match self.peek() {
                Some(Kind::Equal) => {
                    self.next();
                    let new_rhs = self.parse_logical_expr()?;
                    lhs = self.expr.add(Expr::Assign(lhs, new_rhs));
                }
                _ => return Ok(lhs),
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
            _ => self.expr.add(Expr::Block(vec![])), // through
        };
        Ok(self.expr.add(Expr::IfElse(cond, if_block, else_block)))
    }

    pub fn parse_block(&mut self) -> Result<ExprRef> {
        self.expect_err(&Kind::BraceOpen)?;
        match self.peek() {
            Some(Kind::BraceClose) | None => {
                // empty block
                self.next();
                Ok(self.expr.add(Expr::Block(vec![])))
            }
            _ => {
                let block = self.parse_block_impl(vec![])?;
                self.expect_err(&Kind::BraceClose)?;
                Ok(self.expr.add(Expr::Block(block)))
            }
        }
    }

    // input multi expressions by lines
    pub fn parse_block_impl(&mut self, mut statements: Vec<StmtRef>) -> Result<Vec<StmtRef>> {
        // check end of expressions
        match self.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None =>
                return Ok(statements),
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
                return Ok(statements);
            }
            _ => (),
        }

        let lhs = self.parse_stmt();
        if lhs.is_err() {
            return Err(anyhow!("parse_expression_block: expected stmt: {:?}", lhs.err()));
        }
        statements.push(lhs?);

        self.parse_block_impl(statements)
    }

    pub fn parse_var_def(&mut self) -> Result<StmtRef> {
        // ("val" | "var") identifier (":" type)? "=" logical_expr?
        let is_val = match self.peek() {
            Some(Kind::Val) => true,
            Some(Kind::Var) => false,
            _ => return Err(anyhow!("parse_var_def: expected val or var")),
        };
        self.next();

        let ident: DefaultSymbol = match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let s = self.string_interner.get_or_intern(s);
                self.next();
                s
            }
            x => return Err(anyhow!("parse_var_def: expected identifier but {:?}", x)),
        };

        let ty: TypeDecl = match self.peek() {
            Some(Kind::Colon) => {
                self.next();
                self.parse_type_declaration()?
            }
            _ => TypeDecl::Unknown,
        };

        let rhs = match self.peek() {
            Some(Kind::Equal) => {
                self.next();
                let expr = self.parse_logical_expr();
                if expr.is_err() {
                    return Err(expr.err().unwrap());
                }
                Some(expr.unwrap())
            }
            Some(Kind::NewLine) => None,
            _ => return Err(anyhow!("parse_var_def: expected expression but {:?}", self.peek())),
        };
        if is_val {
            Ok(self.stmt.add(Stmt::Val(ident, Some(ty), rhs.unwrap())))
        } else {
            Ok(self.stmt.add(Stmt::Var(ident, Some(ty), rhs)))
        }
    }

    fn parse_type_declaration(&mut self) -> Result<TypeDecl> {
        let ty: TypeDecl = match self.peek() {
            Some(Kind::Bool) => TypeDecl::Bool,
            Some(Kind::U64) => TypeDecl::UInt64,
            Some(Kind::I64) => TypeDecl::Int64,
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let ident = self.string_interner.get_or_intern(s);
                TypeDecl::Identifier(ident)
            }
            Some(Kind::Str) => {
                TypeDecl::String
            }
            Some(_) | None => {
                panic!("parse_type_declaration: unexpected token {:?}", self.peek());
            }
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
                    lhs = self.expr.add(Self::new_binary(op.clone(), lhs, rhs));
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
            Some(Kind::ParenOpen) => {
                self.next();
                let node = self.parse_expr_impl()?;
                self.expect_err(&Kind::ParenClose)?;
                Ok(node)
            }
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let s = self.string_interner.get_or_intern(s);
                self.next();
                match self.peek() {
                    Some(Kind::ParenOpen) => { // function call
                        self.next();
                        let args = self.parse_expr_list(vec![])?;
                        self.expect_err(&Kind::ParenClose)?;
                        let args = self.expr.add(Expr::ExprList(args));
                        let expr = self.expr.add(Expr::Call(s, args));
                        Ok(expr)
                    }
                    _ => {
                        // identifier
                        Ok(self.expr.add(Expr::Identifier(s)))
                    }
                }
            }
            x => {
                let e = Ok(match x {
                    Some(&Kind::UInt64(num)) => self.expr.add(Expr::UInt64(num)),
                    Some(&Kind::Int64(num)) => self.expr.add(Expr::Int64(num)),
                    Some(&Kind::Null) => self.expr.add(Expr::Null),
                    Some(&Kind::True) => self.expr.add(Expr::True),
                    Some(&Kind::False) => self.expr.add(Expr::False),
                    Some(Kind::String(s)) => {
                        let s = s.to_string();
                        let s = self.string_interner.get_or_intern(s);
                        self.expr.add(Expr::String(s))
                    }
                    Some(Kind::Integer(s)) => {
                        let s = s.to_string();
                        let s = self.string_interner.get_or_intern(s);
                        self.expr.add(Expr::Number(s))
                    }
                    x => {
                        return match x {
                            Some(Kind::ParenOpen) => {
                                self.next();
                                let e = self.parse_expr_impl()?;
                                self.expect_err(&Kind::ParenClose)?;
                                Ok(e)
                            }
                            Some(Kind::BraceOpen) => {
                                self.parse_block()
                            }
                            // TODO: write parse_expr right recursion (TODO: more smart way 🤔)
                            Some(Kind::If) => {
                                self.next();
                                self.parse_if()
                            }
                            _ => {
                                Err(anyhow!("parse_primary: unexpected token {:?}", x))
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

        let expr = self.parse_expr_impl();
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

    mod lexer_tests{
        use super::*;
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
            let s = " -1i64 1i64 2u64  true false null 1234";
            let mut l = lexer::Lexer::new(&s, 1u64);
            assert_eq!(l.yylex().unwrap().kind, Kind::Int64(-1));
            assert_eq!(l.yylex().unwrap().kind, Kind::Int64(1));
            assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(2u64));
            assert_eq!(l.yylex().unwrap().kind, Kind::True);
            assert_eq!(l.yylex().unwrap().kind, Kind::False);
            assert_eq!(l.yylex().unwrap().kind, Kind::Null);
            assert_eq!(l.yylex().unwrap().kind, Kind::Integer("1234".to_string()));
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
            let s = " + - * /";
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
    }

    mod parser_tests {
        use super::*;
        use crate::type_checker::TypeCheckerVisitor;

        #[test]
        fn parser_util_lookahead() {
            let mut p = Parser::new("1u64 + 2u64");

            let t0 = p.peek_n(0).unwrap().clone();
            let t1 = p.peek_n(1).unwrap().clone();
            assert_eq!(Kind::UInt64(1), t0);
            assert_eq!(Kind::IAdd, t1);
            let mut consume = |count: usize| -> usize {
                p.ahead.drain(0..count).count()
            };
            assert_eq!(2, consume(2));

            let t2 = p.peek().unwrap();
            assert_eq!(Kind::UInt64(2), *t2);
        }

        #[rstest]
        #[case("for i in 0u64 to 9u64 { }")]
        fn lexer_simple_example(#[case] input: &str) {
            let mut l = lexer::Lexer::new(input, 1u64);
            loop {
                let t = l.yylex();
                match t {
                    Ok(x) => eprintln!("{:?}", x.kind),
                    Err(_) => break,
                }
            }
        }
        #[test]
        fn parser_simple_expr_test1() {
            let mut p = Parser::new("1u64 + 2u64 ");
            let _ = p.parse_stmt().unwrap();
            assert_eq!(3, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);

            println!("p.stmt: {:?}", p.stmt);
            println!("INSTRUCTION {:?}", p.stmt.get(0));
            println!("INSTRUCTION {:?}", p.stmt.get(1));
            assert_eq!(1, p.stmt.len(), "stmt.len must be 1");

            let d = p.stmt.get(0).unwrap();
            assert_eq!(Stmt::Expression(ExprRef(2)), *d);
        }

        #[test]
        fn parser_simple_expr_mul() {
            let mut p = Parser::new("(1u64) + 2u64 * 3u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::UInt64(3), *c);

            let d = p.expr.get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::IMul, ExprRef(1), ExprRef(2)), *d);
            let e = p.expr.get(4).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(3)), *e);
        }

        #[test]
        fn parser_simple_relational_expr() {
            let mut p = Parser::new("0u64 < 2u64 + 4u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::UInt64(0), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::UInt64(4), *c);

            let d = p.expr.get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(1), ExprRef(2)), *d);
            let e = p.expr.get(4).unwrap();
            assert_eq!(Expr::Binary(Operator::LT, ExprRef(0), ExprRef(3)), *e);
        }

        #[test]
        fn parser_simple_logical_expr() {
            let mut p = Parser::new("1u64 && 2u64 < 3u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::UInt64(3), *c);

            let d = p.expr.get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::LT, ExprRef(1), ExprRef(2)), *d);
            let e = p.expr.get(4).unwrap();
            assert_eq!(Expr::Binary(Operator::LogicalAnd, ExprRef(0), ExprRef(3)), *e);
        }

        #[rstest]
        #[case("1u64")]
        #[case("(1u64 + 2u64)")]
        #[case("1u64 && 2u64 < 3u64")]
        #[case("1u64 || 2u64 < 3u64")]
        #[case("1u64 || (2u64) < 3u64 + 4u64")]
        #[case("variable")]
        #[case("a + b")]
        #[case("a + 1u64")]
        #[case("a() + 1u64")]
        #[case("a(b,c) + 1u64")]
        fn parser_expr_accept(#[case] input: &str) {
            let mut p = Parser::new(input);
            let e = p.parse_stmt();
            assert!(e.is_ok(), "failed: {}", input);
        }

        #[test]
        fn parser_simple_ident_expr() {
            let mut p = Parser::new("abc + 1u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(3, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::Identifier(p.string_interner.get_or_intern("abc".to_string())), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(1), *b);

            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);
        }

        #[test]
        fn parser_simple_apply_empty() {
            let mut p = Parser::new("abc()");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(2, p.expr.len(), "ExprPool.len must be 2");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::ExprList(vec![]), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::Call(p.string_interner.get_or_intern("abc".to_string()), ExprRef(0)), *b);
        }

        #[test]
        fn parser_simple_assign_expr() {
            let mut p = Parser::new("a = 1u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(3, p.expr.len(), "ExprPool.len must be 3");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::Identifier(p.string_interner.get_or_intern("a".to_string())), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(1u64), *b);
            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::Assign(ExprRef(0), ExprRef(1)), *c);
        }

        // Valid statement or expression
        #[rstest]
        #[case("1u64")]
        #[case("1i64")]
        #[case("true")]
        #[case("false")]
        #[case("null")]
        #[case("\"string\"")]
        #[case("val x = 1u64")]
        #[case("val x: u64 = 1u64")]
        #[case("val x: u64 = if true { 1u64 } else { 2u64 }")]
        #[case("var x = 1u64")]
        #[case("x = y = z = 1u64")]
        #[case("x = 1u64")]
        #[case("if true { 1u64 }")]
        #[case("if true { 1u64 } else { 2u64 }")]
        #[case("{ if true { 1u64 } else { 2u64 } }")]
        #[case("fn_call()")]
        #[case("fn_call(a, b, c)")]
        #[case("a + b * c / d")]
        #[case("a || b && c")]
        #[case("a <= b && c >= d && e < f && g > h")]
        #[case("a == b && c != d")]
        #[case("for i in 0u64 to 9u64 { continue }")]
        #[case("while true { break }")]
        #[case("return true")]
        #[case("return")]
        fn parser_test_parse_stmt(#[case] input: &str) {
            let mut parser = Parser::new(input);
            let err = parser.parse_stmt();
            assert!(err.is_ok(), "input: {} err: {:?}", input, err);
        }

        #[rstest]
        #[case("1u64+")]
        #[case("*2u64")]
        #[case("(1u64+2u64")]
        fn parser_errors_parse_expr(#[case] input: &str) {
            let mut parser = Parser::new(input);
            assert!(parser.parse_expr_impl().is_err(), "input: {}", input);
        }

        #[test]
        fn parser_simple_apply_expr() {
            let mut p = Parser::new("abc(1u64, 2u64)");
            let e = p.parse_stmt();
            assert!(e.is_ok(), "{:?}", p.expr);

            assert_eq!(4, p.expr.len(), "ExprPool.len must be 4");
            let a = p.expr.get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.expr.get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);

            let c = p.expr.get(2).unwrap();
            assert_eq!(Expr::ExprList(vec![ExprRef(0), ExprRef(1)]), *c);
            let d = p.expr.get(3).unwrap();
            assert_eq!(Expr::Call(p.string_interner.get_or_intern("abc".to_string()), ExprRef(2)), *d);
        }

        #[test]
        fn parser_param_def() {
            let mut p = Parser::new("test: u64");
            let param = p.parse_param_def();
            assert!(param.is_ok());
            let param = param.unwrap();
            let test_id = p.string_interner.get_or_intern("test".to_string());
            assert_eq!((test_id, TypeDecl::UInt64), param);
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
            let mut p = Parser::new("test: u64, test2: i64, test3: some_type");
            let param = p.parse_param_def_list(vec![]);
            assert!(param.is_ok());
            let some_type = p.string_interner.get_or_intern("some_type".to_string());
            assert_eq!(
                vec![
                    (p.string_interner.get_or_intern("test".to_string()), TypeDecl::UInt64),
                    (p.string_interner.get_or_intern("test2".to_string()), TypeDecl::Int64),
                    (p.string_interner.get_or_intern("test3".to_string()), TypeDecl::Identifier(some_type)),
                ],
                param.unwrap()
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

            let stmt_pool = &program.statement;
            let mut expr_pool = program.expression;
            let string_interner = &program.string_interner;

            let mut tc = TypeCheckerVisitor::new(stmt_pool, &mut expr_pool, string_interner);
            // Register all defined functions
            program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

            program.function.iter().for_each(|f| {
                let res = tc.type_check(f.clone());
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

            let stmt_pool = program.statement;
            let mut expr_pool = program.expression;
            let interner = program.string_interner;

            let mut tc = TypeCheckerVisitor::new(&stmt_pool, &mut expr_pool, &interner);
            let mut res = true;
            program.function.iter().for_each(|f| {
                let r = tc.type_check(f.clone());
                if r.is_err() {
                    res = false;
                }
            });

            assert!(!res, "{:?}: type check should fail", path.to_str().unwrap());
        }
    }
}
