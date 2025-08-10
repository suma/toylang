use crate::ast::*;
use crate::token::Kind;
use super::core::Parser;
use anyhow::Result;
use string_interner::DefaultSymbol;

#[derive(Debug)]
pub struct OperatorGroup<'a> {
    pub tokens: Vec<(Kind, Operator)>,
    pub next_precedence: fn(&mut Parser<'a>) -> Result<ExprRef>,
}

impl<'a> Parser<'a> {
    pub fn parse_expr(&mut self) -> Result<StmtRef> {
        let e = self.parse_expr_impl();
        Ok(self.expr_to_stmt(e?))
    }

    pub fn parse_expr_impl(&mut self) -> Result<ExprRef> {
        self.check_and_increment_recursion()?;
        
        let result = self.parse_expr_impl_internal();
        
        self.decrement_recursion();
        result
    }

    fn parse_expr_impl_internal(&mut self) -> Result<ExprRef> {
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
                let line = self.line_count();
                self.collect_error(&format!("expected expression but found {:?} at line {}", x, line));
                // Return a dummy expression to continue parsing
                Ok(self.ast_builder.null_expr(None))
            }
            None => {
                self.collect_error("unexpected EOF while parsing expression");
                // Return a dummy expression to continue parsing
                Ok(self.ast_builder.null_expr(None))
            }
        }
    }

    fn expr_to_stmt(&mut self, e: ExprRef) -> StmtRef {
        self.ast_builder.expression_stmt(e, None)
    }
}

pub fn parse_assign(parser: &mut Parser, mut lhs: ExprRef) -> Result<ExprRef> {
    loop {
        match parser.peek() {
            Some(Kind::Equal) => {
                parser.next();
                let new_rhs = parse_logical_expr(parser)?;
                let location = parser.current_source_location();
                lhs = parser.ast_builder.assign_expr(lhs, new_rhs, Some(location));
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
        _ => {
            let location = parser.current_source_location();
            parser.ast_builder.block_expr(vec![], Some(location))
        }
    };

    let location = parser.current_source_location();
    Ok(parser.ast_builder.if_elif_else_expr(cond, if_block, elif_pairs, else_block, Some(location)))
}

pub fn parse_block(parser: &mut Parser) -> Result<ExprRef> {
    parser.expect_err(&Kind::BraceOpen)?;
    match parser.peek() {
        Some(Kind::BraceClose) | None => {
            parser.next();
            let location = parser.current_source_location();
            Ok(parser.ast_builder.block_expr(vec![], Some(location)))
        }
        _ => {
            let block = parse_block_impl(parser, vec![])?;
            parser.expect_err(&Kind::BraceClose)?;
            let location = parser.current_source_location();
            Ok(parser.ast_builder.block_expr(block, Some(location)))
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
        parser.collect_error(&format!("expected statement in block: {:?}", lhs.err()));
        return Ok(statements); // Return current statements and continue
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
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_binary_impl(parser, group);
    
    parser.decrement_recursion();
    result
}

fn parse_binary_impl<'a>(parser: &mut Parser<'a>, group: &OperatorGroup<'a>) -> Result<ExprRef> {
    let mut lhs = (group.next_precedence)(parser)?;

    loop {
        let next_token = parser.peek();
        let matched_op = group.tokens.iter()
            .find(|(kind, _)| next_token == Some(kind));

        match matched_op {
            Some((_, op)) => {
                let location = parser.current_source_location();
                parser.next();
                let rhs = (group.next_precedence)(parser)?;
                lhs = parser.ast_builder.binary_expr(op.clone(), lhs, rhs, Some(location));
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
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_postfix_impl(parser);
    
    parser.decrement_recursion();
    result
}

fn parse_postfix_impl(parser: &mut Parser) -> Result<ExprRef> {
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
                            let location = parser.current_source_location();
                            parser.next();
                            let args = parse_expr_list(parser, vec![])?;
                            parser.expect_err(&Kind::ParenClose)?;
                            expr = parser.ast_builder.method_call_expr(expr, field_symbol, args, Some(location));
                        } else {
                            let location = parser.current_source_location();
                            expr = parser.ast_builder.field_access_expr(expr, field_symbol, Some(location));
                        }
                    }
                    _ => {
                        parser.collect_error("expected field name after '.'");
                        break; // Stop processing and return current expr
                    }
                }
            }
            // Array access is handled in parse_primary for identifiers
            _ => break,
        }
    }
    
    Ok(expr)
}

pub fn parse_primary(parser: &mut Parser) -> Result<ExprRef> {
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_primary_impl(parser);
    
    parser.decrement_recursion();
    result
}

fn parse_primary_impl(parser: &mut Parser) -> Result<ExprRef> {
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
                    let location = parser.current_source_location();
                    parser.next();
                    let args = parse_expr_list(parser, vec![])?;
                    parser.expect_err(&Kind::ParenClose)?;
                    let expr = parser.ast_builder.call_expr(s, args, Some(location));
                    Ok(expr)
                }
                Some(Kind::BracketOpen) => {
                    let location = parser.current_source_location();
                    parser.next();
                    let index = parser.parse_expr_impl()?;
                    parser.expect_err(&Kind::BracketClose)?;
                    let array_ref = parser.ast_builder.identifier_expr(s, None);
                    Ok(parser.ast_builder.array_access_expr(array_ref, index, Some(location)))
                }
                Some(Kind::BraceOpen) => {
                    let location = parser.current_source_location();
                    parser.next();
                    let fields = parse_struct_literal_fields(parser, vec![])?;
                    parser.expect_err(&Kind::BraceClose)?;
                    Ok(parser.ast_builder.struct_literal_expr(s, fields, Some(location)))
                }
                _ => {
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.identifier_expr(s, Some(location)))
                }
            }
        }
        x => {
            let e = Ok(match x {
                Some(&Kind::UInt64(num)) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.uint64_expr(num, Some(location))
                },
                Some(&Kind::Int64(num)) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.int64_expr(num, Some(location))
                },
                Some(&Kind::Null) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.null_expr(Some(location))
                },
                Some(&Kind::True) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.bool_true_expr(Some(location))
                },
                Some(&Kind::False) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.bool_false_expr(Some(location))
                },
                Some(Kind::String(s)) => {
                    let s_copy = s.to_string();
                    let location = parser.current_source_location();
                    let s = parser.string_interner.get_or_intern(s_copy);
                    parser.ast_builder.string_expr(s, Some(location))
                }
                Some(Kind::Integer(s)) => {
                    let s_copy = s.to_string();
                    let location = parser.current_source_location();
                    let s = parser.string_interner.get_or_intern(s_copy);
                    parser.ast_builder.number_expr(s, Some(location))
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
                            let location = parser.current_source_location();
                            parser.next();
                            let elements = parse_array_elements(parser, vec![])?;
                            parser.expect_err(&Kind::BracketClose)?;
                            Ok(parser.ast_builder.array_literal_expr(elements, Some(location)))
                        }
                        Some(Kind::If) => {
                            parser.next();
                            parse_if(parser)
                        }
                        _ => {
                            let x_cloned = x.cloned();
                            parser.collect_error(&format!("unexpected token in primary expression: {:?}", x_cloned));
                            Ok(parser.ast_builder.null_expr(None)) // Return dummy expression
                        }
                    }
                }
            });
            parser.next();
            e
        }
    }
}

pub fn parse_expr_list(parser: &mut Parser, args: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
    // Add recursion protection for expression list parsing
    parser.check_and_increment_recursion()?;
    
    let result = parse_expr_list_impl(parser, args);
    
    parser.decrement_recursion();
    result
}

fn parse_expr_list_impl(parser: &mut Parser, mut args: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
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
        x => {
            let x_cloned = x.cloned();
            parser.collect_error(&format!("unexpected token in expression list: {:?}", x_cloned));
            Ok(args) // Return current args and stop
        }
    }
}

pub fn parse_array_elements(parser: &mut Parser, mut elements: Vec<ExprRef>) -> Result<Vec<ExprRef>> {
    // Limit maximum elements to prevent infinite loops
    const MAX_ELEMENTS: usize = 1000;
    let mut element_count = 0;

    loop {
        parser.skip_newlines();
        
        element_count += 1;
        if element_count > MAX_ELEMENTS {
            parser.collect_error("too many elements in array literal");
            return Ok(elements);
        }
        
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
                    Some(Kind::BracketClose) => return Ok(elements),
                    _ => continue, // Continue the loop for next element
                }
            }
            Some(Kind::BracketClose) => return Ok(elements),
            Some(Kind::NewLine) => {
                parser.skip_newlines();
                match parser.peek() {
                    Some(Kind::BracketClose) => return Ok(elements),
                    _ => continue, // Continue the loop for next element
                }
            }
            x => {
                let x_cloned = x.cloned();
                parser.collect_error(&format!("unexpected token in array elements: {:?}", x_cloned));
                return Ok(elements); // Return current elements and stop
            }
        }
    }
}

pub fn parse_struct_literal_fields(parser: &mut Parser, fields: Vec<(DefaultSymbol, ExprRef)>) -> Result<Vec<(DefaultSymbol, ExprRef)>> {
    // Add recursion protection for struct literal field parsing
    parser.check_and_increment_recursion()?;
    
    let result = parse_struct_literal_fields_impl(parser, fields);
    
    parser.decrement_recursion();
    result
}

fn parse_struct_literal_fields_impl(parser: &mut Parser, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> Result<Vec<(DefaultSymbol, ExprRef)>> {
    if parser.peek() == Some(&Kind::BraceClose) {
        return Ok(fields);
    }

    // Limit maximum fields to prevent infinite loops in malformed input
    const MAX_FIELDS: usize = 100;
    let mut field_count = 0;

    loop {
        field_count += 1;
        if field_count > MAX_FIELDS {
            parser.collect_error("too many fields in struct literal");
            return Ok(fields);
        }

        let field_name = match parser.peek() {
            Some(Kind::Identifier(name)) => {
                let name = name.to_string();
                let symbol = parser.string_interner.get_or_intern(name);
                parser.next();
                symbol
            }
            _ => {
                parser.collect_error("expected field name in struct literal");
                return Ok(fields); // Return current fields and stop
            }
        };

        let has_colon = parser.peek() == Some(&Kind::Colon);
        if !parser.expect_or_collect(has_colon, "expected ':' after field name") {
            return Ok(fields);
        }
        parser.next();

        let field_value = match parser.parse_expr_impl() {
            Ok(expr) => expr,
            Err(_) => {
                parser.collect_error("failed to parse field value");
                return Ok(fields);
            }
        };

        fields.push((field_name, field_value));

        match parser.peek() {
            Some(&Kind::Comma) => {
                parser.next();
                if parser.peek() == Some(&Kind::BraceClose) {
                    break;
                }
            }
            Some(&Kind::BraceClose) => break,
            _ => {
                parser.collect_error("expected ',' or '}' in struct literal fields");
                break; // Stop processing and return current fields
            }
        }
    }

    Ok(fields)
}