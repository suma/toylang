use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::Kind;
use super::core::Parser;
use anyhow::{anyhow, Result};
use string_interner::DefaultSymbol;

impl<'a> Parser<'a> {
    pub fn parse_stmt(&mut self) -> Result<StmtRef> {
        parse_stmt(self)
    }
}

pub fn parse_stmt(parser: &mut Parser) -> Result<StmtRef> {
    match parser.peek() {
        Some(Kind::Val) | Some(Kind::Var) => {
            parse_var_def(parser)
        }
        Some(Kind::Break) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.break_stmt(location))
        }
        Some(Kind::Continue) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.continue_stmt(location))
        }
        Some(Kind::Return) => {
            parser.next();
            match parser.peek() {
                Some(&Kind::NewLine) | Some(&Kind::BracketClose) | Some(Kind::EOF) => {
                    let location = parser.current_source_location();
                    parser.next();
                    Ok(parser.ast_builder.return_stmt(None, location))
                }
                None => {
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.return_stmt(None, location))
                },
                Some(_expr) => {
                    let location = parser.current_source_location();
                    let expr = parser.parse_expr_impl()?;
                    Ok(parser.ast_builder.return_stmt(Some(expr), location))
                }
            }
        }
        Some(Kind::For) => {
            parser.next();
            match parser.peek() {
                Some(Kind::Identifier(s)) => {
                    let s = s.to_string();
                    let ident = parser.string_interner.get_or_intern(s);
                    parser.next();
                    parser.expect_err(&Kind::In)?;
                    let start = super::expr::parse_relational(parser)?;
                    parser.expect_err(&Kind::To)?;
                    let end = super::expr::parse_relational(parser)?;
                    let block = super::expr::parse_block(parser)?;
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.for_stmt(ident, start, end, block, location))
                }
                x => Err(anyhow!("parse_stmt for: expected identifier but {:?}", x)),
            }
        }
        Some(Kind::While) => {
            parser.next();
            let cond = super::expr::parse_logical_expr(parser)?;
            let block = super::expr::parse_block(parser)?;
            let location = parser.current_source_location();
            Ok(parser.ast_builder.while_stmt(cond, block, location))
        }
        _ => parser.parse_expr(),
    }
}

pub fn parse_var_def(parser: &mut Parser) -> Result<StmtRef> {
    let is_val = match parser.peek() {
        Some(Kind::Val) => true,
        Some(Kind::Var) => false,
        _ => return Err(anyhow!("parse_var_def: expected val or var")),
    };
    parser.next();

    let ident: DefaultSymbol = match parser.peek() {
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let s = parser.string_interner.get_or_intern(s);
            parser.next();
            s
        }
        x => return Err(anyhow!("parse_var_def: expected identifier but {:?}", x)),
    };

    let ty: TypeDecl = match parser.peek() {
        Some(Kind::Colon) => {
            parser.next();
            parser.parse_type_declaration()?
        }
        _ => TypeDecl::Unknown,
    };

    let rhs = match parser.peek() {
        Some(Kind::Equal) => {
            parser.next();
            let expr = super::expr::parse_logical_expr(parser);
            if expr.is_err() {
                return Err(expr.err().unwrap());
            }
            Some(expr.unwrap())
        }
        Some(Kind::NewLine) => None,
        _ => return Err(anyhow!("parse_var_def: expected expression but {:?}", parser.peek())),
    };
    let location = parser.current_source_location();
    if is_val {
        Ok(parser.ast_builder.val_stmt(ident, Some(ty), rhs.unwrap(), location))
    } else {
        Ok(parser.ast_builder.var_stmt(ident, Some(ty), rhs, location))
    }
}

pub fn parse_struct_fields(parser: &mut Parser, mut fields: Vec<StructField>) -> Result<Vec<StructField>> {
    parser.skip_newlines();
    
    match parser.peek() {
        Some(Kind::BraceClose) => return Ok(fields),
        _ => (),
    }

    let visibility = match parser.peek() {
        Some(Kind::Public) => {
            parser.next();
            Visibility::Public
        }
        _ => Visibility::Private,
    };

    let field_name = match parser.peek() {
        Some(Kind::Identifier(s)) => {
            let name = s.to_string();
            parser.next();
            name
        }
        _ => return Err(anyhow!("expected field name")),
    };

    parser.expect_err(&Kind::Colon)?;
    let field_type = parser.parse_type_declaration()?;

    fields.push(StructField {
        name: field_name,
        type_decl: field_type,
        visibility,
    });

    parser.skip_newlines();
    match parser.peek() {
        Some(Kind::Comma) => {
            parser.next();
            parser.skip_newlines();
            match parser.peek() {
                Some(Kind::BraceClose) => Ok(fields),
                _ => parse_struct_fields(parser, fields)
            }
        }
        Some(Kind::BraceClose) => Ok(fields),
        _ => parse_struct_fields(parser, fields),
    }
}

pub fn parse_impl_methods(parser: &mut Parser, mut methods: Vec<Rc<MethodFunction>>) -> Result<Vec<Rc<MethodFunction>>> {
    parser.skip_newlines();
    
    match parser.peek() {
        Some(Kind::BraceClose) => return Ok(methods),
        _ => (),
    }

    match parser.peek() {
        Some(Kind::Function) => {
            let fn_start_pos = parser.peek_position_n(0).unwrap().start;
            let location = parser.current_source_location();
            parser.next();
            match parser.peek() {
                Some(Kind::Identifier(s)) => {
                    let s = s.to_string();
                    parser.next();
                    let method_name = parser.string_interner.get_or_intern(s);
                    
                    parser.expect_err(&Kind::ParenOpen)?;
                    let (params, has_self) = parse_method_param_list(parser, vec![])?;
                    parser.expect_err(&Kind::ParenClose)?;
                    
                    let mut ret_ty: Option<TypeDecl> = None;
                    match parser.peek() {
                        Some(Kind::Arrow) => {
                            parser.expect_err(&Kind::Arrow)?;
                            ret_ty = Some(parser.parse_type_declaration()?);
                        }
                        _ => (),
                    }
                    
                    let block = super::expr::parse_block(parser)?;
                    let fn_end_pos = parser.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                    
                    methods.push(Rc::new(MethodFunction {
                        node: Node::new(fn_start_pos, fn_end_pos),
                        name: method_name,
                        parameter: params,
                        return_type: ret_ty,
                        code: parser.ast_builder.expression_stmt(block, location),
                        has_self_param: has_self,
                    }));
                    
                    parser.skip_newlines();
                    parse_impl_methods(parser, methods)
                }
                _ => Err(anyhow!("expected method name after fn")),
            }
        }
        _ => Ok(methods),
    }
}

pub fn parse_method_param_list(parser: &mut Parser, args: Vec<Parameter>) -> Result<(Vec<Parameter>, bool)> {
    let mut has_self = false;
    
    match parser.peek() {
        Some(Kind::ParenClose) => return Ok((args, has_self)),
        _ => (),
    }

    if let Some(Kind::And) = parser.peek() {
        if let Some(Kind::Identifier(name)) = parser.peek_n(1) {
            if name == "self" {
                parser.next();
                parser.next();
                has_self = true;
                
                match parser.peek() {
                    Some(Kind::Comma) => {
                        parser.next();
                        let (rest_params, _) = parse_param_def_list_impl(parser, args)?;
                        return Ok((rest_params, has_self));
                    }
                    Some(Kind::ParenClose) => return Ok((args, has_self)),
                    _ => return Err(anyhow!("expected comma or closing paren after &self")),
                }
            }
        }
    }

    let (params, _) = parse_param_def_list_impl(parser, args)?;
    Ok((params, has_self))
}

pub fn parse_param_def_list_impl(parser: &mut Parser, mut args: Vec<Parameter>) -> Result<(Vec<Parameter>, bool)> {
    match parser.peek() {
        Some(Kind::ParenClose) => return Ok((args, false)),
        _ => (),
    }

    let def = parser.parse_param_def();
    if def.is_err() {
        return Ok((args, false));
    }
    args.push(def?);

    match parser.peek() {
        Some(Kind::Comma) => {
            parser.next();
            parse_param_def_list_impl(parser, args)
        }
        _ => Ok((args, false)),
    }
}