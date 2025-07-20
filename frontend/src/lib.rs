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
    pub ast_builder: AstBuilder,
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
            ast_builder: AstBuilder::with_capacity(1024, 1024),
            string_interner: DefaultStringInterner::new(),
        }
    }

    fn peek(&mut self) -> Option<&Kind> {
        if self.ahead.is_empty() {
            loop {
                match self.lexer.yylex() {
                    Ok(t) => {
                        // Skip comment tokens
                        if matches!(t.kind, Kind::Comment(_)) {
                            continue;
                        }
                        self.ahead.push(t);
                        return Some(&self.ahead.get(0).unwrap().kind);
                    }
                    _ => return None,
                }
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
            loop {
                match self.lexer.yylex() {
                    Ok(t) => {
                        // Skip comment tokens
                        if matches!(t.kind, Kind::Comment(_)) {
                            continue;
                        }
                        self.ahead.push(t);
                        break;
                    }
                    _ => return None,
                }
            }
        }
        match self.ahead.get(pos) {
            Some(t) => Some(&t.kind),
            None => None,
        }
    }

    fn peek_position_n(&mut self, pos: usize) -> Option<&std::ops::Range<usize>> {
        while self.ahead.len() < pos + 1 {
            loop {
                match self.lexer.yylex() {
                    Ok(t) => {
                        // Skip comment tokens
                        if matches!(t.kind, Kind::Comment(_)) {
                            continue;
                        }
                        self.ahead.push(t);
                        break;
                    }
                    _ => return None,
                }
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


    pub fn expect_err(&mut self, accept: &Kind) -> Result<()> {
        if !self.expect(accept) {
            return Err(anyhow!("{:?} expected but {:?}", accept, self.ahead.get(0)));
        }
        Ok(())
    }


    pub fn next_expr(&self) -> u32 {
        self.ast_builder.get_expr_pool().len() as u32
    }

    // Helper methods for tests
    pub fn get_expr_pool(&self) -> &ExprPool {
        self.ast_builder.get_expr_pool()
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        self.ast_builder.get_stmt_pool()
    }

    pub fn get_string_interner(&mut self) -> &mut DefaultStringInterner {
        &mut self.string_interner
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
        std::mem::swap(&mut stmt, self.ast_builder.get_stmt_pool_mut());
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
                                node: Node::new(fn_start_pos, fn_end_pos),
                                name: fn_name,
                                parameter: params,
                                return_type: ret_ty,
                                code: self.ast_builder.expression_stmt(block),
                            }));
                        }
                        _ => return Err(anyhow!("expected function")),
                    }
                }
                // Struct definition
                Some(Kind::Struct) => {
                    let struct_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(struct_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let struct_name = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let fields = self.parse_struct_fields(vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let struct_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(struct_end_pos);
                            
                            // Add struct declaration as a statement
                            self.ast_builder.struct_decl_stmt(struct_name, fields);
                        }
                        _ => return Err(anyhow!("expected struct name")),
                    }
                }
                // Impl block definition
                Some(Kind::Impl) => {
                    let impl_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(impl_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let target_type = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let methods = self.parse_impl_methods(vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let impl_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(impl_end_pos);
                            
                            // Add impl block as a statement
                            self.ast_builder.impl_block_stmt(target_type, methods);
                        }
                        _ => return Err(anyhow!("expected type name for impl block")),
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
        let mut ast_builder = AstBuilder::new();
        std::mem::swap(&mut ast_builder, &mut self.ast_builder);
        let (expr, stmt) = ast_builder.extract_pools();
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
                Ok(self.ast_builder.break_stmt())
            }
            Some(Kind::Continue) => {
                self.next();
                Ok(self.ast_builder.continue_stmt())
            }
            Some(Kind::Return) => {
                self.next();
                match self.peek() {
                    Some(&Kind::NewLine) | Some(&Kind::BracketClose) | Some(Kind::EOF) => {
                        self.next();
                        Ok(self.ast_builder.return_stmt(None))
                    }
                    // Usually None is error but we treat this case for unit test.
                    None => Ok(self.ast_builder.return_stmt(None)),
                    Some(_expr) => {
                        let expr = self.parse_expr_impl()?;
                        Ok(self.ast_builder.return_stmt(Some(expr)))
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
                        Ok(self.ast_builder.for_stmt(ident, start, end, block))
                    }
                    x => Err(anyhow!("parse_stmt for: expected identifier but {:?}", x)),
                }
            }
            Some(Kind::While) => {
                self.next();
                let cond = self.parse_logical_expr()?;
                let block = self.parse_block()?;
                Ok(self.ast_builder.while_stmt(cond, block))
            }
            _ => self.parse_expr(),
        }
    }

    fn expr_to_stmt(&mut self, e: ExprRef) -> StmtRef {
        self.ast_builder.expression_stmt(e)
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
                    lhs = self.ast_builder.assign_expr(lhs, new_rhs);
                }
                _ => return Ok(lhs),
            }
        }
    }
    pub fn parse_if(&mut self) -> Result<ExprRef> {
        let cond = self.parse_logical_expr()?;
        let if_block = self.parse_block()?;

        // Parse elif chains
        let mut elif_pairs = Vec::new();
        while let Some(Kind::Elif) = self.peek() {
            self.next(); // consume 'elif'
            let elif_cond = self.parse_logical_expr()?;
            let elif_block = self.parse_block()?;
            elif_pairs.push((elif_cond, elif_block));
        }

        let else_block: ExprRef = match self.peek() {
            Some(Kind::Else) => {
                self.next();
                self.parse_block()?
            }
            _ => self.ast_builder.block_expr(vec![]), // through
        };

        // Always use IfElifElse (elif_pairs can be empty for regular if-else)
        Ok(self.ast_builder.if_elif_else_expr(cond, if_block, elif_pairs, else_block))
    }

    pub fn parse_block(&mut self) -> Result<ExprRef> {
        self.expect_err(&Kind::BraceOpen)?;
        match self.peek() {
            Some(Kind::BraceClose) | None => {
                // empty block
                self.next();
                Ok(self.ast_builder.block_expr(vec![]))
            }
            _ => {
                let block = self.parse_block_impl(vec![])?;
                self.expect_err(&Kind::BraceClose)?;
                Ok(self.ast_builder.block_expr(block))
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
            Ok(self.ast_builder.val_stmt(ident, Some(ty), rhs.unwrap()))
        } else {
            Ok(self.ast_builder.var_stmt(ident, Some(ty), rhs))
        }
    }

    fn parse_type_declaration(&mut self) -> Result<TypeDecl> {
        match self.peek() {
            Some(Kind::BracketOpen) => {
                // Array type: [element_type; size]
                self.next(); // consume '['
                let element_type = self.parse_type_declaration()?;
                self.expect_err(&Kind::Semicolon)?;
                
                let size = match self.peek().cloned() {
                    Some(Kind::UInt64(n)) => {
                        self.next();
                        n as usize
                    }
                    Some(Kind::Integer(s)) => {
                        self.next();
                        s.parse::<usize>().map_err(|_| anyhow!("Invalid array size: {}", s))?
                    }
                    Some(Kind::Underscore) => {
                        self.next();
                        0 // placeholder for inferred size
                    }
                    _ => return Err(anyhow!("Expected array size or underscore"))
                };
                
                self.expect_err(&Kind::BracketClose)?;
                Ok(TypeDecl::Array(vec![element_type; size], size))
            }
            Some(Kind::Bool) => {
                self.next();
                Ok(TypeDecl::Bool)
            }
            Some(Kind::U64) => {
                self.next();
                Ok(TypeDecl::UInt64)
            }
            Some(Kind::I64) => {
                self.next();
                Ok(TypeDecl::Int64)
            }
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let ident = self.string_interner.get_or_intern(s);
                self.next();
                Ok(TypeDecl::Identifier(ident))
            }
            Some(Kind::Str) => {
                self.next();
                Ok(TypeDecl::String)
            }
            Some(_) | None => {
                Err(anyhow!("parse_type_declaration: unexpected token {:?}", self.peek()))
            }
        }
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
                    lhs = self.ast_builder.binary_expr(op.clone(), lhs, rhs);
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
            next_precedence: Self::parse_postfix,
        };
        self.parse_binary(&group)
    }

    fn parse_postfix(&mut self) -> Result<ExprRef> {
        let mut expr = self.parse_primary()?;
        
        loop {
            match self.peek() {
                Some(Kind::Dot) => {
                    self.next(); // consume '.'
                    match self.peek() {
                        Some(Kind::Identifier(field_name)) => {
                            let field_name = field_name.to_string();
                            let field_symbol = self.string_interner.get_or_intern(field_name);
                            self.next(); // consume field name
                            
                            // Check if this is a method call
                            if self.peek() == Some(&Kind::ParenOpen) {
                                self.next(); // consume '('
                                let args = self.parse_expr_list(vec![])?;
                                self.expect_err(&Kind::ParenClose)?;
                                expr = self.ast_builder.method_call_expr(expr, field_symbol, args);
                            } else {
                                // Field access
                                expr = self.ast_builder.field_access_expr(expr, field_symbol);
                            }
                        }
                        _ => return Err(anyhow!("parse_postfix: expected field name after '.'")),
                    }
                }
                _ => break,
            }
        }
        
        Ok(expr)
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
                        let expr = self.ast_builder.call_expr(s, args);
                        Ok(expr)
                    }
                    Some(Kind::BracketOpen) => { // array access
                        self.next();
                        let index = self.parse_expr_impl()?;
                        self.expect_err(&Kind::BracketClose)?;
                        let array_ref = self.ast_builder.identifier_expr(s);
                        Ok(self.ast_builder.array_access_expr(array_ref, index))
                    }
                    Some(Kind::BraceOpen) => { // struct literal
                        self.next();
                        let fields = self.parse_struct_literal_fields(vec![])?;
                        self.expect_err(&Kind::BraceClose)?;
                        Ok(self.ast_builder.struct_literal_expr(s, fields))
                    }
                    _ => {
                        // identifier
                        Ok(self.ast_builder.identifier_expr(s))
                    }
                }
            }
            x => {
                let e = Ok(match x {
                    Some(&Kind::UInt64(num)) => self.ast_builder.uint64_expr(num),
                    Some(&Kind::Int64(num)) => self.ast_builder.int64_expr(num),
                    Some(&Kind::Null) => self.ast_builder.null_expr(),
                    Some(&Kind::True) => self.ast_builder.bool_true_expr(),
                    Some(&Kind::False) => self.ast_builder.bool_false_expr(),
                    Some(Kind::String(s)) => {
                        let s = s.to_string();
                        let s = self.string_interner.get_or_intern(s);
                        self.ast_builder.string_expr(s)
                    }
                    Some(Kind::Integer(s)) => {
                        let s = s.to_string();
                        let s = self.string_interner.get_or_intern(s);
                        self.ast_builder.number_expr(s)
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
                            Some(Kind::BracketOpen) => { // array literal
                                self.next();
                                let elements = self.parse_array_elements(vec![])?;
                                self.expect_err(&Kind::BracketClose)?;
                                Ok(self.ast_builder.array_literal_expr(elements))
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

    fn parse_array_elements(&mut self, mut elements: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
        // Skip newlines
        self.skip_newlines();
        
        match self.peek() {
            Some(Kind::BracketClose) => return Ok(elements),
            _ => (),
        }

        let expr = self.parse_expr_impl();
        if expr.is_err() {
            // there is no expr in this context
            return Ok(elements);
        }
        elements.push(expr?);

        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                // Skip newlines after comma
                self.skip_newlines();
                // Check if we're at the end after comma (trailing comma case)
                match self.peek() {
                    Some(Kind::BracketClose) => Ok(elements),
                    _ => self.parse_array_elements(elements)
                }
            }
            Some(Kind::BracketClose) => Ok(elements),
            x => Err(anyhow!("parse_array_elements: unexpected token {:?}", x)),
        }
    }

    fn skip_newlines(&mut self) {
        while let Some(Kind::NewLine) = self.peek() {
            self.next();
        }
    }

    fn parse_struct_fields(&mut self, mut fields: Vec<StructField>) -> Result<Vec<StructField>> {
        // Skip newlines
        self.skip_newlines();
        
        match self.peek() {
            Some(Kind::BraceClose) => return Ok(fields),
            _ => (),
        }

        // Parse field definition: [pub] name: type
        let visibility = match self.peek() {
            Some(Kind::Public) => {
                self.next();
                Visibility::Public
            }
            _ => Visibility::Private,
        };

        let field_name = match self.peek() {
            Some(Kind::Identifier(s)) => {
                let name = s.to_string();
                self.next();
                name
            }
            _ => return Err(anyhow!("expected field name")),
        };

        self.expect_err(&Kind::Colon)?;
        let field_type = self.parse_type_declaration()?;

        fields.push(StructField {
            name: field_name,
            type_decl: field_type,
            visibility,
        });

        // Skip newlines and check for comma or end
        self.skip_newlines();
        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.skip_newlines();
                // Check if we're at the end after comma (trailing comma case)
                match self.peek() {
                    Some(Kind::BraceClose) => Ok(fields),
                    _ => self.parse_struct_fields(fields)
                }
            }
            Some(Kind::BraceClose) => Ok(fields),
            _ => self.parse_struct_fields(fields), // Allow newline-separated fields without commas
        }
    }

    fn parse_impl_methods(&mut self, mut methods: Vec<Rc<MethodFunction>>) -> Result<Vec<Rc<MethodFunction>>> {
        // Skip newlines
        self.skip_newlines();
        
        match self.peek() {
            Some(Kind::BraceClose) => return Ok(methods),
            _ => (),
        }

        // Parse method definition: fn method_name([&self,] params...) -> return_type { body }
        match self.peek() {
            Some(Kind::Function) => {
                let fn_start_pos = self.peek_position_n(0).unwrap().start;
                self.next();
                match self.peek() {
                    Some(Kind::Identifier(s)) => {
                        let s = s.to_string();
                        self.next();
                        let method_name = self.string_interner.get_or_intern(s);
                        
                        self.expect_err(&Kind::ParenOpen)?;
                        let (params, has_self) = self.parse_method_param_list(vec![])?;
                        self.expect_err(&Kind::ParenClose)?;
                        
                        let mut ret_ty: Option<TypeDecl> = None;
                        match self.peek() {
                            Some(Kind::Arrow) => {
                                self.expect_err(&Kind::Arrow)?;
                                ret_ty = Some(self.parse_type_declaration()?);
                            }
                            _ => (),
                        }
                        
                        let block = self.parse_block()?;
                        let fn_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                        
                        methods.push(Rc::new(MethodFunction {
                            node: Node::new(fn_start_pos, fn_end_pos),
                            name: method_name,
                            parameter: params,
                            return_type: ret_ty,
                            code: self.ast_builder.expression_stmt(block),
                            has_self_param: has_self,
                        }));
                        
                        // Skip newlines and continue parsing more methods
                        self.skip_newlines();
                        self.parse_impl_methods(methods)
                    }
                    _ => Err(anyhow!("expected method name after fn")),
                }
            }
            _ => Ok(methods), // No more methods
        }
    }

    fn parse_method_param_list(&mut self, args: Vec<Parameter>) -> Result<(Vec<Parameter>, bool)> {
        let mut has_self = false;
        
        match self.peek() {
            Some(Kind::ParenClose) => return Ok((args, has_self)),
            _ => (),
        }

        // Check for &self parameter
        if let Some(Kind::And) = self.peek() {
            // Check if next token is 'self'
            if let Some(Kind::Identifier(name)) = self.peek_n(1) {
                if name == "self" {
                    self.next(); // consume '&'
                    self.next(); // consume 'self'
                    has_self = true;
                    
                    // Check for comma or end
                    match self.peek() {
                        Some(Kind::Comma) => {
                            self.next();
                            let (rest_params, _) = self.parse_param_def_list_impl(args)?;
                            return Ok((rest_params, has_self));
                        }
                        Some(Kind::ParenClose) => return Ok((args, has_self)),
                        _ => return Err(anyhow!("expected comma or closing paren after &self")),
                    }
                }
            }
        }

        // Parse regular parameters
        let (params, _) = self.parse_param_def_list_impl(args)?;
        Ok((params, has_self))
    }

    fn parse_param_def_list_impl(&mut self, mut args: Vec<Parameter>) -> Result<(Vec<Parameter>, bool)> {
        match self.peek() {
            Some(Kind::ParenClose) => return Ok((args, false)),
            _ => (),
        }

        let def = self.parse_param_def();
        if def.is_err() {
            return Ok((args, false));
        }
        args.push(def?);

        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_param_def_list_impl(args)
            }
            _ => Ok((args, false)),
        }
    }

    fn parse_struct_literal_fields(&mut self, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> Result<Vec<(DefaultSymbol, ExprRef)>> {
        if self.peek() == Some(&Kind::BraceClose) {
            return Ok(fields);
        }

        loop {
            // Parse field name
            let field_name = match self.peek() {
                Some(Kind::Identifier(name)) => {
                    let name = name.to_string();
                    let symbol = self.string_interner.get_or_intern(name);
                    self.next();
                    symbol
                }
                _ => return Err(anyhow!("parse_struct_literal_fields: expected field name")),
            };

            // Expect colon
            self.expect_err(&Kind::Colon)?;

            // Parse field value
            let field_value = self.parse_expr_impl()?;

            fields.push((field_name, field_value));

            // Check for comma or end
            match self.peek() {
                Some(&Kind::Comma) => {
                    self.next();
                    if self.peek() == Some(&Kind::BraceClose) {
                        break;
                    }
                }
                Some(&Kind::BraceClose) => break,
                _ => return Err(anyhow!("parse_struct_literal_fields: expected ',' or '}}'")),
            }
        }

        Ok(fields)
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

        #[test]
        fn lexer_comment_test() {
            let s = "# this is a comment\n val x = 1u64";
            let mut l = lexer::Lexer::new(&s, 1u64);
            assert_eq!(l.yylex().unwrap().kind, Kind::Comment(" this is a comment".to_string()));
            assert_eq!(l.yylex().unwrap().kind, Kind::NewLine);
            assert_eq!(l.yylex().unwrap().kind, Kind::Val);
            assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("x".to_string()));
            assert_eq!(l.yylex().unwrap().kind, Kind::Equal);
            assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(1));
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

        #[test]
        fn parser_comment_skip_test() {
            let mut p = Parser::new("1u64 + 2u64 # another comment");
            let _ = p.parse_stmt().unwrap();
            assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
        }

        #[test]
        fn parser_simple_expr_test1() {
            let mut p = Parser::new("1u64 + 2u64 ");
            let _ = p.parse_stmt().unwrap();
            assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);

            println!("p.stmt: {:?}", p.get_stmt_pool());
            println!("INSTRUCTION {:?}", p.get_stmt_pool().get(0));
            println!("INSTRUCTION {:?}", p.get_stmt_pool().get(1));
            assert_eq!(1, p.get_stmt_pool().len(), "stmt.len must be 1");

            let d = p.get_stmt_pool().get(0).unwrap();
            assert_eq!(Stmt::Expression(ExprRef(2)), *d);
        }

        #[test]
        fn parser_simple_expr_mul() {
            let mut p = Parser::new("(1u64) + 2u64 * 3u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::UInt64(3), *c);

            let d = p.get_expr_pool().get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::IMul, ExprRef(1), ExprRef(2)), *d);
            let e = p.get_expr_pool().get(4).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(3)), *e);
        }

        #[test]
        fn parser_simple_relational_expr() {
            let mut p = Parser::new("0u64 < 2u64 + 4u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::UInt64(0), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::UInt64(4), *c);

            let d = p.get_expr_pool().get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(1), ExprRef(2)), *d);
            let e = p.get_expr_pool().get(4).unwrap();
            assert_eq!(Expr::Binary(Operator::LT, ExprRef(0), ExprRef(3)), *e);
        }

        #[test]
        fn parser_simple_logical_expr() {
            let mut p = Parser::new("1u64 && 2u64 < 3u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);
            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::UInt64(3), *c);

            let d = p.get_expr_pool().get(3).unwrap();
            assert_eq!(Expr::Binary(Operator::LT, ExprRef(1), ExprRef(2)), *d);
            let e = p.get_expr_pool().get(4).unwrap();
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

            assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::Identifier(expected_symbol), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(1), *b);

            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);
        }

        #[test]
        fn parser_simple_apply_empty() {
            let mut p = Parser::new("abc()");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(2, p.get_expr_pool().len(), "ExprPool.len must be 2");
            let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::ExprList(vec![]), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::Call(expected_symbol, ExprRef(0)), *b);
        }

        #[test]
        fn parser_simple_assign_expr() {
            let mut p = Parser::new("a = 1u64");
            let e = p.parse_stmt();
            assert!(e.is_ok());

            assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
            let expected_symbol = p.get_string_interner().get_or_intern("a".to_string());
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::Identifier(expected_symbol), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(1u64), *b);
            let c = p.get_expr_pool().get(2).unwrap();
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
            assert!(e.is_ok(), "{:?}", p.get_expr_pool());

            assert_eq!(4, p.get_expr_pool().len(), "ExprPool.len must be 4");
            let a = p.get_expr_pool().get(0).unwrap();
            assert_eq!(Expr::UInt64(1), *a);
            let b = p.get_expr_pool().get(1).unwrap();
            assert_eq!(Expr::UInt64(2), *b);

            let c = p.get_expr_pool().get(2).unwrap();
            assert_eq!(Expr::ExprList(vec![ExprRef(0), ExprRef(1)]), *c);
            let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
            let d = p.get_expr_pool().get(3).unwrap();
            assert_eq!(Expr::Call(expected_symbol, ExprRef(2)), *d);
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

        #[test]
        fn parser_struct_decl_simple() {
            let input = "struct Point { x: i64, y: i64 }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert_eq!(1, program.statement.len(), "should have one struct declaration");
            
            match program.statement.get(0).unwrap() {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Point", name);
                    assert_eq!(2, fields.len());
                    
                    assert_eq!("x", fields[0].name);
                    assert_eq!(TypeDecl::Int64, fields[0].type_decl);
                    assert_eq!(Visibility::Private, fields[0].visibility);
                    
                    assert_eq!("y", fields[1].name);
                    assert_eq!(TypeDecl::Int64, fields[1].type_decl);
                    assert_eq!(Visibility::Private, fields[1].visibility);
                }
                _ => panic!("Expected struct declaration"),
            }
        }

        #[test]
        fn parser_struct_decl_with_visibility() {
            let input = "struct Person { pub name: str, age: u64 }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert_eq!(1, program.statement.len(), "should have one struct declaration");
            
            match program.statement.get(0).unwrap() {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Person", name);
                    assert_eq!(2, fields.len());
                    
                    assert_eq!("name", fields[0].name);
                    assert_eq!(TypeDecl::String, fields[0].type_decl);
                    assert_eq!(Visibility::Public, fields[0].visibility);
                    
                    assert_eq!("age", fields[1].name);
                    assert_eq!(TypeDecl::UInt64, fields[1].type_decl);
                    assert_eq!(Visibility::Private, fields[1].visibility);
                }
                _ => panic!("Expected struct declaration"),
            }
        }

        #[test]
        fn parser_struct_decl_empty() {
            let input = "struct Empty { }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert_eq!(1, program.statement.len(), "should have one struct declaration");
            
            match program.statement.get(0).unwrap() {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Empty", name);
                    assert_eq!(0, fields.len());
                }
                _ => panic!("Expected struct declaration"),
            }
        }

        #[test]
        fn parser_struct_decl_with_newlines() {
            let input = "struct Point {\n    x: i64,\n    y: i64\n}";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert_eq!(1, program.statement.len(), "should have one struct declaration");
            
            match program.statement.get(0).unwrap() {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Point", name);
                    assert_eq!(2, fields.len());
                    assert_eq!("x", fields[0].name);
                    assert_eq!("y", fields[1].name);
                }
                _ => panic!("Expected struct declaration"),
            }
        }

        #[test]
        fn parser_struct_with_function() {
            let input = "struct Point { x: i64, y: i64 }\nfn main() -> u64 { 42u64 }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            // Program contains: struct decl, function body as expression stmt, and literal expr
            assert!(program.statement.len() >= 1, "should have at least one struct declaration");
            assert_eq!(1, program.function.len(), "should have one function");
            
            // Check struct - should be the first statement
            match program.statement.get(0).unwrap() {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Point", name);
                    assert_eq!(2, fields.len());
                }
                _ => panic!("Expected struct declaration as first statement"),
            }
            
            // Check function
            let func = &program.function[0];
            assert_eq!(program.string_interner.resolve(func.name), Some("main"));
        }

        #[test]
        fn parser_impl_block_simple() {
            let input = "impl Point { fn new(x: i64, y: i64) -> i64 { 42i64 } }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert!(program.statement.len() >= 1, "should have at least one impl block");
            
            // Find the ImplBlock in statements
            let impl_stmt = program.statement.0.iter().find(|stmt| {
                matches!(stmt, Stmt::ImplBlock { .. })
            }).expect("Should have impl block");
            
            match impl_stmt {
                Stmt::ImplBlock { target_type, methods } => {
                    assert_eq!("Point", target_type);
                    assert_eq!(1, methods.len());
                    
                    let method = &methods[0];
                    assert_eq!(program.string_interner.resolve(method.name), Some("new"));
                    assert!(!method.has_self_param);
                    assert_eq!(2, method.parameter.len());
                }
                _ => panic!("Expected impl block declaration"),
            }
        }

        #[test]
        fn parser_impl_block_with_self() {
            let input = "impl Point { fn distance(&self) -> i64 { 42i64 } }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert!(program.statement.len() >= 1, "should have at least one impl block");
            
            let impl_stmt = program.statement.0.iter().find(|stmt| {
                matches!(stmt, Stmt::ImplBlock { .. })
            }).expect("Should have impl block");
            
            match impl_stmt {
                Stmt::ImplBlock { target_type, methods } => {
                    assert_eq!("Point", target_type);
                    assert_eq!(1, methods.len());
                    
                    let method = &methods[0];
                    assert_eq!(program.string_interner.resolve(method.name), Some("distance"));
                    assert!(method.has_self_param);
                    assert_eq!(0, method.parameter.len()); // &self is not counted in regular parameters
                }
                _ => panic!("Expected impl block declaration"),
            }
        }

        #[test]
        fn parser_impl_block_multiple_methods() {
            let input = "impl Point { fn new() -> i64 { 42i64 } fn get_x(&self) -> i64 { 0i64 } }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert!(program.statement.len() >= 1, "should have at least one impl block");
            
            let impl_stmt = program.statement.0.iter().find(|stmt| {
                matches!(stmt, Stmt::ImplBlock { .. })
            }).expect("Should have impl block");
            
            match impl_stmt {
                Stmt::ImplBlock { target_type, methods } => {
                    assert_eq!("Point", target_type);
                    assert_eq!(2, methods.len());
                    
                    let method1 = &methods[0];
                    assert_eq!(program.string_interner.resolve(method1.name), Some("new"));
                    assert!(!method1.has_self_param);
                    
                    let method2 = &methods[1];
                    assert_eq!(program.string_interner.resolve(method2.name), Some("get_x"));
                    assert!(method2.has_self_param);
                }
                _ => panic!("Expected impl block declaration"),
            }
        }

        #[test]
        fn parser_struct_with_impl() {
            let input = "struct Point { x: i64, y: i64 }\nimpl Point { fn new() -> i64 { 42i64 } }";
            let mut parser = Parser::new(input);
            let result = parser.parse_program();
            assert!(result.is_ok(), "parse err {:?}", result.err());
            
            let program = result.unwrap();
            assert!(program.statement.len() >= 2, "should have struct and impl declarations");
            
            // Find struct and impl blocks
            let struct_stmt = program.statement.0.iter().find(|stmt| {
                matches!(stmt, Stmt::StructDecl { .. })
            }).expect("Should have struct declaration");
            
            let impl_stmt = program.statement.0.iter().find(|stmt| {
                matches!(stmt, Stmt::ImplBlock { .. })
            }).expect("Should have impl block");
            
            // Check struct
            match struct_stmt {
                Stmt::StructDecl { name, fields } => {
                    assert_eq!("Point", name);
                    assert_eq!(2, fields.len());
                }
                _ => panic!("Expected struct declaration"),
            }
            
            // Check impl
            match impl_stmt {
                Stmt::ImplBlock { target_type, methods } => {
                    assert_eq!("Point", target_type);
                    assert_eq!(1, methods.len());
                }
                _ => panic!("Expected impl block declaration"),
            }
        }
    }
}
