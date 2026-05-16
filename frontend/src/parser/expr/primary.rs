use crate::ast::*;
use crate::token::{Kind, StringPart};
use crate::parser::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use string_interner::DefaultSymbol;

use super::{
    parse_logical_expr, parse_block, parse_expr_list,
    parse_if, parse_with, parse_dict_literal, parse_match,
    parse_postfix, try_intercept_parser_macro,
};

/// Parse bracket access syntax: [index], [start..end], [..end], [start..], [..]
pub(super) fn parse_bracket_access(parser: &mut Parser, object_expr: ExprRef, location: crate::type_checker::SourceLocation) -> ParserResult<ExprRef> {
    if parser.peek() == Some(&Kind::DotDot) {
        parser.next();
        if parser.peek() == Some(&Kind::BracketClose) {
            parser.next();
            let slice_info = SliceInfo::range_slice(None, None);
            Ok(parser.ast_builder.slice_access_expr(object_expr, slice_info, Some(location)))
        } else {
            let end = parse_logical_expr(parser)?;
            parser.expect_err(&Kind::BracketClose)?;
            let slice_info = SliceInfo::range_slice(None, Some(end));
            Ok(parser.ast_builder.slice_access_expr(object_expr, slice_info, Some(location)))
        }
    } else {
        let first_expr = parse_logical_expr(parser)?;
        if parser.peek() == Some(&Kind::DotDot) {
            parser.next();
            if parser.peek() == Some(&Kind::BracketClose) {
                parser.next();
                let slice_info = SliceInfo::range_slice(Some(first_expr), None);
                Ok(parser.ast_builder.slice_access_expr(object_expr, slice_info, Some(location)))
            } else {
                let end = parse_logical_expr(parser)?;
                parser.expect_err(&Kind::BracketClose)?;
                let slice_info = SliceInfo::range_slice(Some(first_expr), Some(end));
                Ok(parser.ast_builder.slice_access_expr(object_expr, slice_info, Some(location)))
            }
        } else {
            parser.expect_err(&Kind::BracketClose)?;
            let slice_info = SliceInfo::single_element(first_expr);
            Ok(parser.ast_builder.slice_access_expr(object_expr, slice_info, Some(location)))
        }
    }
}

/// Parse a closure / lambda literal: `fn(params) -> Ret { body }`.
fn parse_closure_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let location = parser.current_source_location();
    parser.expect_err(&Kind::Function)?;
    parser.expect_err(&Kind::ParenOpen)?;
    let params = parser.parse_param_def_list(vec![])?;
    parser.expect_err(&Kind::ParenClose)?;
    let return_type = if parser.peek() == Some(&Kind::Arrow) {
        parser.next();
        Some(parser.parse_type_declaration()?)
    } else {
        None
    };
    let body = parse_block(parser)?;
    Ok(parser.ast_builder.closure_expr(params, return_type, body, Some(location)))
}

/// String interpolation desugaring: `"hello {x}"` → concat chain.
fn parse_interpolated_string(parser: &mut Parser) -> ParserResult<ExprRef> {
    let parts: Vec<StringPart> = match parser.peek() {
        Some(Kind::InterpolatedString(p)) => p.clone(),
        _ => return Err(ParserError::generic_error(
            parser.current_source_location(),
            "parse_interpolated_string called without InterpolatedString token".to_string(),
        )),
    };
    parser.next();

    let parts: Vec<StringPart> = parts
        .into_iter()
        .filter(|p| !matches!(p, StringPart::Literal(s) if s.is_empty()))
        .collect();

    if parts.is_empty() {
        let location = parser.current_source_location();
        let sym = parser.string_interner.get_or_intern("");
        return Ok(parser.ast_builder.string_expr(sym, Some(location)));
    }

    let mut tokens: Vec<Kind> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            tokens.push(Kind::Dot);
            tokens.push(Kind::Identifier("concat".to_string()));
            tokens.push(Kind::ParenOpen);
        }
        match part {
            StringPart::Literal(s) => {
                tokens.push(Kind::String(s.clone()));
            }
            StringPart::Expr(expr_text) => {
                tokens.push(Kind::Identifier("__builtin_to_string".to_string()));
                tokens.push(Kind::ParenOpen);
                let mut sub_lex = crate::parser::core::lexer::Lexer::new(expr_text, 1);
                loop {
                    match sub_lex.yylex() {
                        Ok(tok) => {
                            if matches!(tok.kind, Kind::NewLine | Kind::Comment(_)) {
                                continue;
                            }
                            tokens.push(tok.kind);
                        }
                        Err(crate::parser::core::lexer::Error::EOF) => break,
                        Err(e) => {
                            return Err(ParserError::generic_error(
                                parser.current_source_location(),
                                format!(
                                    "invalid expression in string interpolation `{}`: lex error {:?}",
                                    expr_text, e
                                ),
                            ));
                        }
                    }
                }
                tokens.push(Kind::ParenClose);
            }
        }
        if i > 0 {
            tokens.push(Kind::ParenClose);
        }
    }

    for tok in tokens.into_iter().rev() {
        parser.insert_token(tok);
    }

    parse_postfix(parser)
}

/// Parse `(a, b)` tuple or `(expr)` grouped expression.
fn parse_tuple_or_grouped_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let location = parser.current_source_location();
    parser.next();
    parser.skip_newlines();
    if parser.peek() == Some(&Kind::ParenClose) {
        parser.next();
        return Ok(parser.ast_builder.tuple_literal_expr(vec![], Some(location)));
    }
    let first = parser.parse_expr_impl()?;
    parser.skip_newlines();
    if parser.peek() == Some(&Kind::Comma) {
        let mut elements = vec![first];
        loop {
            parser.next();
            parser.skip_newlines();
            if parser.peek() == Some(&Kind::ParenClose) {
                break;
            }
            elements.push(parser.parse_expr_impl()?);
            parser.skip_newlines();
            if parser.peek() != Some(&Kind::Comma) {
                break;
            }
        }
        parser.expect_err(&Kind::ParenClose)?;
        Ok(parser.ast_builder.tuple_literal_expr(elements, Some(location)))
    } else {
        parser.expect_err(&Kind::ParenClose)?;
        Ok(first)
    }
}

/// Top-level primary dispatch.
pub fn parse_primary(parser: &mut Parser) -> ParserResult<ExprRef> {
    parse_primary_impl(parser)
}

fn parse_primary_impl(parser: &mut Parser) -> ParserResult<ExprRef> {
    if matches!(parser.peek(), Some(Kind::Function))
        && matches!(parser.peek_n(1), Some(Kind::ParenOpen))
    {
        return parse_closure_expr(parser);
    }
    if matches!(parser.peek(), Some(Kind::InterpolatedString(_))) {
        return parse_interpolated_string(parser);
    }
    match parser.peek() {
        Some(Kind::ParenOpen) => parse_tuple_or_grouped_expr(parser),
        Some(ref kind) if kind.is_keyword() && !matches!(kind, Kind::True | Kind::False | Kind::Null | Kind::If | Kind::Dict | Kind::Self_ | Kind::With | Kind::Ambient | Kind::Match) => {
            let location = parser.current_source_location();
            Err(ParserError::generic_error(location, "parse_primary_impl: reserved keyword cannot be used as identifier".to_string()))
        }
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let s = parser.string_interner.get_or_intern(s);
            parser.next();
            parse_primary_after_identifier(parser, s)
        }
        _ => parse_primary_atom_or_form(parser),
    }
}

/// Parse what follows an identifier head in primary position.
fn parse_primary_after_identifier(parser: &mut Parser, name: DefaultSymbol) -> ParserResult<ExprRef> {
    if parser.peek() == Some(&Kind::DoubleColon) {
        let mut qualified_path = vec![name];
        while parser.peek() == Some(&Kind::DoubleColon) {
            parser.next();
            if let Some(Kind::Identifier(next_part)) = parser.peek() {
                let next_part = next_part.to_string();
                let next_symbol = parser.string_interner.get_or_intern(next_part);
                qualified_path.push(next_symbol);
                parser.next();
            } else {
                parser.collect_error("expected identifier after '::'");
                break;
            }
        }
        return match parser.peek() {
            Some(Kind::ParenOpen) => {
                let location = parser.current_source_location();
                parser.next();
                let args = parse_expr_list(parser, vec![])?;
                parser.expect_err(&Kind::ParenClose)?;
                if qualified_path.len() == 2 {
                    let struct_name = qualified_path[0];
                    let function_name = qualified_path[1];
                    Ok(parser.ast_builder.associated_function_call_expr(struct_name, function_name, args, Some(location)))
                } else {
                    let function_name = qualified_path.last().copied().unwrap_or(name);
                    Ok(parser.ast_builder.call_expr(function_name, args, Some(location)))
                }
            }
            _ => {
                let location = parser.current_source_location();
                Ok(parser.ast_builder.qualified_identifier_expr(qualified_path, Some(location)))
            }
        };
    }

    let struct_literal_allowed = parser.is_struct_literal_allowed();
    match parser.peek() {
        Some(Kind::ParenOpen) => {
            let location = parser.current_source_location();
            if let Some(rewritten) = try_intercept_parser_macro(parser, name, location)? {
                return Ok(rewritten);
            }
            parser.next();
            let args = parse_expr_list(parser, vec![])?;
            parser.expect_err(&Kind::ParenClose)?;
            if let Some(builtin_func) = parser.builtin_symbols.symbol_to_builtin(name) {
                Ok(parser.ast_builder.builtin_call_expr(builtin_func, args, Some(location)))
            } else {
                Ok(parser.ast_builder.call_expr(name, args, Some(location)))
            }
        }
        Some(Kind::BracketOpen) => {
            let location = parser.current_source_location();
            parser.next();
            let object_ref = parser.ast_builder.identifier_expr(name, None);
            parse_bracket_access(parser, object_ref, location)
        }
        Some(Kind::BraceOpen) if struct_literal_allowed => {
            let location = parser.current_source_location();
            parser.next();
            let fields = parse_struct_literal_fields(parser, vec![])?;
            parser.expect_err(&Kind::BraceClose)?;
            Ok(parser.ast_builder.struct_literal_expr(name, fields, Some(location)))
        }
        _ => {
            let location = parser.current_source_location();
            Ok(parser.ast_builder.identifier_expr(name, Some(location)))
        }
    }
}

/// Parse atomic literal or structured form.
fn parse_primary_atom_or_form(parser: &mut Parser) -> ParserResult<ExprRef> {
    let x = parser.peek();
    let e = Ok(match x {
        Some(&Kind::UInt64(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.uint64_expr(num, Some(location))
        }
        Some(&Kind::Int64(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.int64_expr(num, Some(location))
        }
        Some(&Kind::UInt32(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.uint32_expr(num, Some(location))
        }
        Some(&Kind::Int32(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.int32_expr(num, Some(location))
        }
        Some(&Kind::UInt16(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.uint16_expr(num, Some(location))
        }
        Some(&Kind::Int16(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.int16_expr(num, Some(location))
        }
        Some(&Kind::UInt8(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.uint8_expr(num, Some(location))
        }
        Some(&Kind::Int8(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.int8_expr(num, Some(location))
        }
        Some(&Kind::Float64(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.float64_expr(num, Some(location))
        }
        Some(&Kind::Null) => {
            let location = parser.current_source_location();
            parser.ast_builder.null_expr(Some(location))
        }
        Some(&Kind::True) => {
            let location = parser.current_source_location();
            parser.ast_builder.bool_true_expr(Some(location))
        }
        Some(&Kind::False) => {
            let location = parser.current_source_location();
            parser.ast_builder.bool_false_expr(Some(location))
        }
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
        _ => return parse_primary_keyword_form(parser),
    });
    parser.next();
    e
}

/// Parse primary expression starting with keyword or punctuation.
fn parse_primary_keyword_form(parser: &mut Parser) -> ParserResult<ExprRef> {
    let x = parser.peek();
    match x {
        Some(Kind::ParenOpen) => {
            parser.next();
            let e = parser.parse_expr_impl()?;
            parser.expect_err(&Kind::ParenClose)?;
            Ok(e)
        }
        Some(Kind::BraceOpen) => parse_block(parser),
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
        Some(Kind::With) => {
            parser.next();
            parse_with(parser)
        }
        Some(Kind::Ambient) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.builtin_call_expr(
                crate::ast::BuiltinFunction::CurrentAllocator,
                vec![],
                Some(location),
            ))
        }
        Some(Kind::Dict) => {
            parser.next();
            parse_dict_literal(parser)
        }
        Some(Kind::Match) => {
            parser.next();
            parse_match(parser)
        }
        _ => {
            let x_cloned = x.cloned();
            parser.collect_error(&format!("unexpected token in primary expression: {:?}", x_cloned));
            Ok(parser.ast_builder.null_expr(None))
        }
    }
}

/// Parse array literal elements.
pub fn parse_array_elements(parser: &mut Parser, mut elements: Vec<ExprRef>) -> ParserResult<Vec<ExprRef>> {
    parser.enter_nested_structure(false);
    let base_max_elements = 2000;
    let complexity_score = parser.get_complexity_score();
    let max_elements = base_max_elements + (complexity_score * 100);
    let mut element_count = 0;

    loop {
        parser.skip_newlines();
        element_count += 1;
        if element_count > max_elements {
            parser.collect_error(&format!("too many elements in array literal (max: {}, complexity: {})",
                                         max_elements, complexity_score));
            parser.exit_nested_structure(false);
            return Ok(elements);
        }
        if let Some(Kind::BracketClose) = parser.peek() {
            parser.exit_nested_structure(false);
            return Ok(elements);
        }
        let expr = parser.parse_expr_impl();
        if expr.is_err() {
            parser.exit_nested_structure(false);
            return Ok(elements);
        }
        elements.push(expr?);
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                parser.skip_newlines();
                match parser.peek() {
                    Some(Kind::BracketClose) => {
                        parser.exit_nested_structure(false);
                        return Ok(elements);
                    }
                    _ => continue,
                }
            }
            Some(Kind::BracketClose) => {
                parser.exit_nested_structure(false);
                return Ok(elements);
            }
            Some(Kind::NewLine) => {
                parser.skip_newlines();
                match parser.peek() {
                    Some(Kind::BracketClose) => {
                        parser.exit_nested_structure(false);
                        return Ok(elements);
                    }
                    _ => continue,
                }
            }
            x => {
                let x_cloned = x.cloned();
                parser.collect_error(&format!("unexpected token in array elements: {:?}", x_cloned));
                parser.exit_nested_structure(false);
                return Ok(elements);
            }
        }
    }
}

/// Parse struct literal fields.
pub(crate) fn parse_struct_literal_fields(parser: &mut Parser, fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<Vec<(DefaultSymbol, ExprRef)>> {
    
    parse_struct_literal_fields_impl(parser, fields)
}

fn parse_struct_literal_fields_impl(parser: &mut Parser, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<Vec<(DefaultSymbol, ExprRef)>> {
    loop {
        parser.skip_newlines();
        match parser.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => return Ok(fields),
            _ => (),
        }
        let field_name = match parser.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let sym = parser.string_interner.get_or_intern(s);
                parser.next();
                sym
            }
            Some(Kind::NewLine) => {
                parser.next();
                continue;
            }
            x => {
                let x_cloned = x.cloned();
                parser.collect_error(&format!("expected field name in struct literal, got {:?}", x_cloned));
                return Ok(fields);
            }
        };
        parser.expect_err(&Kind::Colon)?;
        let field_value = parser.parse_expr_impl()?;
        fields.push((field_name, field_value));
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
            }
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => return Ok(fields),
            _ => {
                parser.collect_error("expected ',' or '}' in struct literal");
                return Ok(fields);
            }
        }
    }
}
