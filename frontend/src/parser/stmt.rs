use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::Kind;
use super::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use string_interner::DefaultSymbol;

impl<'a> Parser<'a> {
    pub fn parse_stmt(&mut self) -> ParserResult<StmtRef> {
        parse_stmt(self)
    }
}

pub fn parse_stmt(parser: &mut Parser) -> ParserResult<StmtRef> {
    match parser.peek() {
        Some(Kind::Val) | Some(Kind::Var) => {
            parse_var_def(parser)
        }
        Some(Kind::Break) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.break_stmt(Some(location)))
        }
        Some(Kind::Continue) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.continue_stmt(Some(location)))
        }
        Some(Kind::Return) => {
            parser.next();
            match parser.peek() {
                Some(&Kind::NewLine) | Some(&Kind::BracketClose) | Some(Kind::EOF) => {
                    let location = parser.current_source_location();
                    parser.next();
                    Ok(parser.ast_builder.return_stmt(None, Some(location)))
                }
                None => {
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.return_stmt(None, Some(location)))
                },
                Some(_expr) => {
                    let location = parser.current_source_location();
                    let expr = parser.parse_expr_impl()?;
                    Ok(parser.ast_builder.return_stmt(Some(expr), Some(location)))
                }
            }
        }
        Some(Kind::For) => {
            parser.next();
            let current_token = parser.peek().cloned();
            match current_token {
                Some(Kind::Identifier(s)) => {
                    let ident = parser.string_interner.get_or_intern(s);
                    parser.next();
                    parser.expect_err(&Kind::In)?;
                    let start = super::expr::parse_relational(parser)?;
                    parser.expect_err(&Kind::To)?;
                    let end = super::expr::parse_relational(parser)?;
                    let block = super::expr::parse_block(parser)?;
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.for_stmt(ident, start, end, block, Some(location)))
                }
                x => {
                    let location = parser.current_source_location();
                    Err(ParserError::generic_error(location, format!("parse_stmt for: expected identifier but {:?}", x)))
                },
            }
        }
        Some(Kind::While) => {
            parser.next();
            let cond = super::expr::parse_logical_expr(parser)?;
            let block = super::expr::parse_block(parser)?;
            let location = parser.current_source_location();
            Ok(parser.ast_builder.while_stmt(cond, block, Some(location)))
        }
        _ => parser.parse_expr(),
    }
}

pub fn parse_var_def(parser: &mut Parser) -> ParserResult<StmtRef> {
    let is_val = match parser.peek() {
        Some(Kind::Val) => true,
        Some(Kind::Var) => false,
        _ => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, "parse_var_def: expected val or var".to_string()))
        },
    };
    parser.next();

    let current_token = parser.peek().cloned();
    let ident: DefaultSymbol = match current_token {
        Some(Kind::Identifier(s)) => {
            let sym = parser.string_interner.get_or_intern(s);
            parser.next();
            sym
        }
        Some(ref kind) if kind.is_keyword() => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: reserved keyword '{}' cannot be used as identifier", 
                match kind {
                    Kind::If => "if",
                    Kind::Else => "else", 
                    Kind::While => "while",
                    Kind::For => "for",
                    Kind::Function => "fn",
                    Kind::Return => "return",
                    Kind::Break => "break", 
                    Kind::Continue => "continue",
                    Kind::Val => "val",
                    Kind::Var => "var",
                    Kind::Struct => "struct",
                    Kind::Impl => "impl",
                    _ => "keyword"
                }
            )))
        }
        x => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: expected identifier but {:?}", x)))
        },
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
            Some(expr?)
        }
        Some(Kind::NewLine) => None,
        _ => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: expected expression but {:?}", parser.peek())))
        },
    };
    let location = parser.current_source_location();
    if is_val {
        Ok(parser.ast_builder.val_stmt(ident, Some(ty), rhs.unwrap(), Some(location)))
    } else {
        Ok(parser.ast_builder.var_stmt(ident, Some(ty), rhs, Some(location)))
    }
}

pub fn parse_struct_fields(parser: &mut Parser, mut fields: Vec<StructField>) -> ParserResult<Vec<StructField>> {
    // Limit maximum number of fields to prevent infinite loops
    const MAX_FIELDS: usize = 1000;
    
    loop {
        parser.skip_newlines();
        
        // Check for end of fields or too many fields
        if parser.peek() == Some(&Kind::BraceClose) || fields.len() >= MAX_FIELDS {
            if fields.len() >= MAX_FIELDS {
                parser.collect_error(&format!("too many struct fields (max: {})", MAX_FIELDS));
            }
            return Ok(fields);
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
            _ => {
                let location = parser.current_source_location();
                return Err(ParserError::generic_error(location, "expected field name".to_string()))
            },
        };

        parser.expect_err(&Kind::Colon)?;
        let field_type = match parser.parse_type_declaration() {
            Ok(ty) => ty,
            Err(e) => {
                parser.collect_error(&format!("expected type after ':' in struct field: {}", e));
                return Ok(fields);
            }
        };

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
                // Continue loop to parse next field
                if parser.peek() == Some(&Kind::BraceClose) {
                    return Ok(fields);
                }
            }
            Some(Kind::BraceClose) => {
                return Ok(fields);
            }
            _ => {
                let current_token = parser.peek().cloned();
                parser.collect_error(&format!("expected ',' or '}}' after struct field, found {:?}", current_token));
                return Ok(fields);
            }
        }
    }
}

pub fn parse_impl_methods(parser: &mut Parser, mut methods: Vec<Rc<MethodFunction>>) -> ParserResult<Vec<Rc<MethodFunction>>> {
    // Limit maximum number of methods to prevent infinite loops
    const MAX_METHODS: usize = 500;
    
    loop {
        parser.skip_newlines();
        
        // Check for end of methods or too many methods
        if parser.peek() == Some(&Kind::BraceClose) || methods.len() >= MAX_METHODS {
            if methods.len() >= MAX_METHODS {
                parser.collect_error(&format!("too many impl methods (max: {})", MAX_METHODS));
            }
            return Ok(methods);
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
                            code: parser.ast_builder.expression_stmt(block, Some(location)),
                            has_self_param: has_self,
                        }));
                        
                        parser.skip_newlines();
                        // Continue loop to parse next method
                    }
                    _ => {
                        let location = parser.current_source_location();
                        return Err(ParserError::generic_error(location, "expected method name after fn".to_string()));
                    }
                }
            }
            _ => {
                // Not a function, we're done
                return Ok(methods);
            }
        }
    }
}

pub fn parse_method_param_list(parser: &mut Parser, args: Vec<Parameter>) -> ParserResult<(Vec<Parameter>, bool)> {
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
                    _ => {
                        let location = parser.current_source_location();
                        return Err(ParserError::generic_error(location, "expected comma or closing paren after &self".to_string()))
                    },
                }
            }
        }
    }

    let (params, _) = parse_param_def_list_impl(parser, args)?;
    Ok((params, has_self))
}

pub fn parse_param_def_list_impl(parser: &mut Parser, mut args: Vec<Parameter>) -> ParserResult<(Vec<Parameter>, bool)> {
    // Limit maximum number of parameters to prevent infinite loops
    const MAX_PARAMS: usize = 255;
    
    loop {
        if parser.peek() == Some(&Kind::ParenClose) || args.len() >= MAX_PARAMS {
            if args.len() >= MAX_PARAMS {
                parser.collect_error(&format!("too many parameters (max: {})", MAX_PARAMS));
            }
            return Ok((args, false));
        }

        let def = parser.parse_param_def();
        if def.is_err() {
            return Ok((args, false));
        }
        args.push(def?);

        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                // Continue loop to parse next parameter
            }
            _ => {
                return Ok((args, false));
            }
        }
    }
}