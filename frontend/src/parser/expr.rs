use crate::ast::*;
use crate::token::Kind;
use super::core::Parser;
use anyhow::{anyhow, Result};
use string_interner::DefaultSymbol;

#[derive(Debug)]
pub struct OperatorGroup<'a> {
    pub tokens: Vec<(Kind, Operator)>,
    pub next_precedence: fn(&mut Parser<'a>) -> Result<ExprRef>,
}

impl<'a> Parser<'a> {
    pub fn parse_expr(&mut self) -> Result<StmtRef> {
        let e = self.parse_expr_impl();
        if e.is_err() {
            return Err(e.err().unwrap());
        }
        Ok(self.expr_to_stmt(e.unwrap()))
    }

    pub fn parse_expr_impl(&mut self) -> Result<ExprRef> {
        let lhs = parse_logical_expr(self);
        if lhs.is_ok() {
            return match self.peek() {
                Some(Kind::Equal) => {
                    parse_assign(self, lhs?)
                }
                _ => lhs,
            };
        }

        match self.peek() {
            Some(Kind::If) => {
                self.next();
                parse_if(self)
            }
            Some(x) => {
                let x = x.clone();
                let line = *((&mut (self.lexer)).get_line_count());
                Err(anyhow!("parse_expr: expected expression but Kind ({:?}) at {}", x, line))
            }
            None => Err(anyhow!("parse_expr: unexpected EOF")),
        }
    }

    fn expr_to_stmt(&mut self, e: ExprRef) -> StmtRef {
        self.ast_builder.expression_stmt(e)
    }
}

pub fn parse_assign(parser: &mut Parser, mut lhs: ExprRef) -> Result<ExprRef> {
    loop {
        match parser.peek() {
            Some(Kind::Equal) => {
                parser.next();
                let new_rhs = parse_logical_expr(parser)?;
                lhs = parser.ast_builder.assign_expr(lhs, new_rhs);
            }
            _ => return Ok(lhs),
        }
    }
}

pub fn parse_if(parser: &mut Parser) -> Result<ExprRef> {
    let cond = parse_logical_expr(parser)?;
    let if_block = parse_block(parser)?;

    let mut elif_pairs = Vec::new();
    while let Some(Kind::Elif) = parser.peek() {
        parser.next();
        let elif_cond = parse_logical_expr(parser)?;
        let elif_block = parse_block(parser)?;
        elif_pairs.push((elif_cond, elif_block));
    }

    let else_block: ExprRef = match parser.peek() {
        Some(Kind::Else) => {
            parser.next();
            parse_block(parser)?
        }
        _ => parser.ast_builder.block_expr(vec![]),
    };

    Ok(parser.ast_builder.if_elif_else_expr(cond, if_block, elif_pairs, else_block))
}

pub fn parse_block(parser: &mut Parser) -> Result<ExprRef> {
    parser.expect_err(&Kind::BraceOpen)?;
    match parser.peek() {
        Some(Kind::BraceClose) | None => {
            parser.next();
            Ok(parser.ast_builder.block_expr(vec![]))
        }
        _ => {
            let block = parse_block_impl(parser, vec![])?;
            parser.expect_err(&Kind::BraceClose)?;
            Ok(parser.ast_builder.block_expr(block))
        }
    }
}

pub fn parse_block_impl(parser: &mut Parser, mut statements: Vec<StmtRef>) -> Result<Vec<StmtRef>> {
    match parser.peek() {
        Some(Kind::BraceClose) | Some(Kind::EOF) | None =>
            return Ok(statements),
        _ => (),
    }

    loop {
        match parser.peek() {
            Some(Kind::NewLine) =>
                parser.next(),
            Some(_) | None =>
                break,
        }
    }

    match parser.peek() {
        Some(Kind::BraceClose) | Some(Kind::EOF) | None => {
            return Ok(statements);
        }
        _ => (),
    }

    let lhs = super::stmt::parse_stmt(parser);
    if lhs.is_err() {
        return Err(anyhow!("parse_expression_block: expected stmt: {:?}", lhs.err()));
    }
    statements.push(lhs?);

    parse_block_impl(parser, statements)
}

pub fn parse_logical_expr(parser: &mut Parser) -> Result<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::DoubleAnd, Operator::LogicalAnd),
            (Kind::DoubleOr, Operator::LogicalOr),
        ],
        next_precedence: parse_equality
    };
    parse_binary(parser, &group)
}

pub fn parse_equality(parser: &mut Parser) -> Result<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::DoubleEqual, Operator::EQ),
            (Kind::NotEqual, Operator::NE),
        ],
        next_precedence: parse_relational
    };
    parse_binary(parser, &group)
}

pub fn parse_relational(parser: &mut Parser) -> Result<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::LT, Operator::LT),
            (Kind::LE, Operator::LE),
            (Kind::GT, Operator::GT),
            (Kind::GE, Operator::GE),
        ],
        next_precedence: parse_add
    };
    parse_binary(parser, &group)
}

pub fn parse_binary<'a>(parser: &mut Parser<'a>, group: &OperatorGroup<'a>) -> Result<ExprRef> {
    let mut lhs = (group.next_precedence)(parser)?;

    loop {
        let next_token = parser.peek();
        let matched_op = group.tokens.iter()
            .find(|(kind, _)| next_token == Some(kind));

        match matched_op {
            Some((_, op)) => {
                parser.next();
                let rhs = (group.next_precedence)(parser)?;
                lhs = parser.ast_builder.binary_expr(op.clone(), lhs, rhs);
            }
            None => return Ok(lhs),
        }
    }
}

pub fn parse_add(parser: &mut Parser) -> Result<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::IAdd, Operator::IAdd),
            (Kind::ISub, Operator::ISub),
        ],
        next_precedence: parse_mul
    };
    parse_binary(parser, &group)
}

pub fn parse_mul(parser: &mut Parser) -> Result<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::IMul, Operator::IMul),
            (Kind::IDiv, Operator::IDiv),
        ],
        next_precedence: parse_postfix,
    };
    parse_binary(parser, &group)
}

pub fn parse_postfix(parser: &mut Parser) -> Result<ExprRef> {
    let mut expr = parse_primary(parser)?;
    
    loop {
        match parser.peek() {
            Some(Kind::Dot) => {
                parser.next();
                match parser.peek() {
                    Some(Kind::Identifier(field_name)) => {
                        let field_name = field_name.to_string();
                        let field_symbol = parser.string_interner.get_or_intern(field_name);
                        parser.next();
                        
                        if parser.peek() == Some(&Kind::ParenOpen) {
                            parser.next();
                            let args = parse_expr_list(parser, vec![])?;
                            parser.expect_err(&Kind::ParenClose)?;
                            expr = parser.ast_builder.method_call_expr(expr, field_symbol, args);
                        } else {
                            expr = parser.ast_builder.field_access_expr(expr, field_symbol);
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

pub fn parse_primary(parser: &mut Parser) -> Result<ExprRef> {
    match parser.peek() {
        Some(Kind::ParenOpen) => {
            parser.next();
            let node = parser.parse_expr_impl()?;
            parser.expect_err(&Kind::ParenClose)?;
            Ok(node)
        }
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let s = parser.string_interner.get_or_intern(s);
            parser.next();
            match parser.peek() {
                Some(Kind::ParenOpen) => {
                    parser.next();
                    let args = parse_expr_list(parser, vec![])?;
                    parser.expect_err(&Kind::ParenClose)?;
                    let expr = parser.ast_builder.call_expr(s, args);
                    Ok(expr)
                }
                Some(Kind::BracketOpen) => {
                    parser.next();
                    let index = parser.parse_expr_impl()?;
                    parser.expect_err(&Kind::BracketClose)?;
                    let array_ref = parser.ast_builder.identifier_expr(s);
                    Ok(parser.ast_builder.array_access_expr(array_ref, index))
                }
                Some(Kind::BraceOpen) => {
                    parser.next();
                    let fields = parse_struct_literal_fields(parser, vec![])?;
                    parser.expect_err(&Kind::BraceClose)?;
                    Ok(parser.ast_builder.struct_literal_expr(s, fields))
                }
                _ => {
                    Ok(parser.ast_builder.identifier_expr(s))
                }
            }
        }
        x => {
            let e = Ok(match x {
                Some(&Kind::UInt64(num)) => parser.ast_builder.uint64_expr(num),
                Some(&Kind::Int64(num)) => parser.ast_builder.int64_expr(num),
                Some(&Kind::Null) => parser.ast_builder.null_expr(),
                Some(&Kind::True) => parser.ast_builder.bool_true_expr(),
                Some(&Kind::False) => parser.ast_builder.bool_false_expr(),
                Some(Kind::String(s)) => {
                    let s = s.to_string();
                    let s = parser.string_interner.get_or_intern(s);
                    parser.ast_builder.string_expr(s)
                }
                Some(Kind::Integer(s)) => {
                    let s = s.to_string();
                    let s = parser.string_interner.get_or_intern(s);
                    parser.ast_builder.number_expr(s)
                }
                x => {
                    return match x {
                        Some(Kind::ParenOpen) => {
                            parser.next();
                            let e = parser.parse_expr_impl()?;
                            parser.expect_err(&Kind::ParenClose)?;
                            Ok(e)
                        }
                        Some(Kind::BraceOpen) => {
                            parse_block(parser)
                        }
                        Some(Kind::BracketOpen) => {
                            parser.next();
                            let elements = parse_array_elements(parser, vec![])?;
                            parser.expect_err(&Kind::BracketClose)?;
                            Ok(parser.ast_builder.array_literal_expr(elements))
                        }
                        Some(Kind::If) => {
                            parser.next();
                            parse_if(parser)
                        }
                        _ => {
                            Err(anyhow!("parse_primary: unexpected token {:?}", x))
                        }
                    }
                }
            });
            parser.next();
            e
        }
    }
}

pub fn parse_expr_list(parser: &mut Parser, mut args: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
    match parser.peek() {
        Some(Kind::ParenClose) => return Ok(args),
        _ => (),
    }

    let expr = parser.parse_expr_impl();
    if expr.is_err() {
        return Ok(args);
    }
    args.push(expr?);

    match parser.peek() {
        Some(Kind::Comma) => {
            parser.next();
            parse_expr_list(parser, args)
        }
        Some(Kind::ParenClose) => Ok(args),
        x => Err(anyhow!("parse_expr_list: unexpected token {:?}", x)),
    }
}

pub fn parse_array_elements(parser: &mut Parser, mut elements: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
    parser.skip_newlines();
    
    match parser.peek() {
        Some(Kind::BracketClose) => return Ok(elements),
        _ => (),
    }

    let expr = parser.parse_expr_impl();
    if expr.is_err() {
        return Ok(elements);
    }
    elements.push(expr?);

    match parser.peek() {
        Some(Kind::Comma) => {
            parser.next();
            parser.skip_newlines();
            match parser.peek() {
                Some(Kind::BracketClose) => Ok(elements),
                _ => parse_array_elements(parser, elements)
            }
        }
        Some(Kind::BracketClose) => Ok(elements),
        x => Err(anyhow!("parse_array_elements: unexpected token {:?}", x)),
    }
}

pub fn parse_struct_literal_fields(parser: &mut Parser, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> Result<Vec<(DefaultSymbol, ExprRef)>> {
    if parser.peek() == Some(&Kind::BraceClose) {
        return Ok(fields);
    }

    loop {
        let field_name = match parser.peek() {
            Some(Kind::Identifier(name)) => {
                let name = name.to_string();
                let symbol = parser.string_interner.get_or_intern(name);
                parser.next();
                symbol
            }
            _ => return Err(anyhow!("parse_struct_literal_fields: expected field name")),
        };

        parser.expect_err(&Kind::Colon)?;

        let field_value = parser.parse_expr_impl()?;

        fields.push((field_name, field_value));

        match parser.peek() {
            Some(&Kind::Comma) => {
                parser.next();
                if parser.peek() == Some(&Kind::BraceClose) {
                    break;
                }
            }
            Some(&Kind::BraceClose) => break,
            _ => return Err(anyhow!("parse_struct_literal_fields: expected ',' or '}}'")),
        }
    }

    Ok(fields)
}