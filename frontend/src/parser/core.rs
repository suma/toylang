use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::Kind;
use super::token_source::{TokenProvider, LexerTokenSource};

use anyhow::{anyhow, Result};
use string_interner::{DefaultStringInterner, DefaultSymbol};

pub mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

pub struct Parser<'a> {
    token_provider: TokenProvider<LexerTokenSource<'a>>,
    pub ast_builder: AstBuilder,
    pub string_interner: DefaultStringInterner,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let source = LexerTokenSource::new(input);
        Parser {
            token_provider: TokenProvider::with_buffer_capacity(source, 128, 64),
            ast_builder: AstBuilder::with_capacity(1024, 1024),
            string_interner: DefaultStringInterner::new(),
        }
    }

    pub fn peek(&mut self) -> Option<&Kind> {
        self.token_provider.peek()
    }

    #[allow(dead_code)]
    pub fn peek_n(&mut self, pos: usize) -> Option<&Kind> {
        self.token_provider.peek_at(pos)
    }

    pub fn peek_position_n(&mut self, pos: usize) -> Option<&std::ops::Range<usize>> {
        self.token_provider.peek_position_at(pos)
    }

    pub fn next(&mut self) {
        self.token_provider.advance();
    }

    pub fn line_count(&mut self) -> u64 {
        self.token_provider.line_count()
    }

    pub fn expect(&mut self, accept: &Kind) -> Result<()> {
        let tk = self.peek();
        if tk.is_some() && *tk.unwrap() == *accept {
            self.next();
            Ok(())
        } else {
            let current = self.peek().unwrap_or(&Kind::EOF);
            Err(anyhow!("Expected {:?} but found {:?}", accept, current))
        }
    }

    #[deprecated(note = "Use expect() instead - this method is redundant")]
    pub fn expect_err(&mut self, accept: &Kind) -> Result<()> {
        self.expect(accept)
    }

    pub fn next_expr(&self) -> u32 {
        self.ast_builder.get_expr_pool().len() as u32
    }

    pub fn get_expr_pool(&self) -> &ExprPool {
        self.ast_builder.get_expr_pool()
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        self.ast_builder.get_stmt_pool()
    }

    pub fn get_string_interner(&mut self) -> &mut DefaultStringInterner {
        &mut self.string_interner
    }

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
                            let mut ret_ty: Option<TypeDecl> = None;
                            match self.peek() {
                                Some(Kind::Arrow) => {
                                    self.expect_err(&Kind::Arrow)?;
                                    ret_ty = Some(self.parse_type_declaration()?);
                                }
                                _ => (),
                            }
                            let block = super::expr::parse_block(self)?;
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
                Some(Kind::Struct) => {
                    let struct_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(struct_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let struct_name = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let fields = super::stmt::parse_struct_fields(self, vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let struct_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(struct_end_pos);
                            
                            self.ast_builder.struct_decl_stmt(struct_name, fields);
                        }
                        _ => return Err(anyhow!("expected struct name")),
                    }
                }
                Some(Kind::Impl) => {
                    let impl_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(impl_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let target_type = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let methods = super::stmt::parse_impl_methods(self, vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let impl_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(impl_end_pos);
                            
                            self.ast_builder.impl_block_stmt(target_type, methods);
                        }
                        _ => return Err(anyhow!("expected type name for impl block")),
                    }
                }
                Some(Kind::NewLine) => {
                    self.next()
                }
                None | Some(Kind::EOF) => break,
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

    pub fn parse_param_def_list(&mut self, mut args: Vec<Parameter>) -> Result<Vec<Parameter>> {
        match self.peek() {
            Some(Kind::ParenClose) => return Ok(args),
            _ => (),
        }

        let def = self.parse_param_def();
        if def.is_err() {
            return Ok(args);
        }
        args.push(def?);

        match self.peek() {
            Some(Kind::Comma) => {
                self.next();
                self.parse_param_def_list(args)
            }
            _ => Ok(args),
        }
    }

    pub fn parse_type_declaration(&mut self) -> Result<TypeDecl> {
        match self.peek() {
            Some(Kind::BracketOpen) => {
                self.next();
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
                        0
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

    pub fn skip_newlines(&mut self) {
        while let Some(Kind::NewLine) = self.peek() {
            self.next();
        }
    }
}