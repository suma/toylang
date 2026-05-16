use crate::ast::*;
use crate::token::Kind;
use crate::parser::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use string_interner::DefaultSymbol;
use super::parse_logical_expr;

pub fn parse_match(parser: &mut Parser) -> ParserResult<ExprRef> {
    let start_location = parser.current_source_location();
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let scrutinee = parse_logical_expr(parser)?;
    parser.pop_context();

    parser.expect_err(&Kind::BraceOpen)?;
    parser.skip_newlines();
    let mut arms: Vec<crate::ast::MatchArm> = Vec::new();
    loop {
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::BraceClose)) {
            break;
        }
        let pattern = parse_match_pattern(parser)?;
        let guard = if matches!(parser.peek(), Some(Kind::If)) {
            parser.next();
            parser.push_context(crate::parser::core::ParseContext::Condition);
            let g = parse_logical_expr(parser)?;
            parser.pop_context();
            Some(g)
        } else {
            None
        };
        parser.expect_err(&Kind::FatArrow)?;
        let body = parse_logical_expr(parser)?;
        arms.push(crate::ast::MatchArm { pattern, guard, body });
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
            parser.skip_newlines();
        }
    }
    parser.expect_err(&Kind::BraceClose)?;

    let expr_ref = parser.ast_builder.add_expr_with_location(
        crate::ast::Expr::Match(scrutinee, arms),
        Some(start_location),
    );
    Ok(expr_ref)
}

pub(crate) fn parse_match_pattern(parser: &mut Parser) -> ParserResult<crate::ast::Pattern> {
    if let Some(Kind::Identifier(s)) = parser.peek()
        && s == "_" {
            parser.next();
            return Ok(crate::ast::Pattern::Wildcard);
        }
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        return parse_pattern_tuple(parser);
    }
    if let Some(pat) = parse_pattern_literal(parser)? {
        return Ok(pat);
    }
    let first = match parser.peek() {
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let sym = parser.string_interner.get_or_intern(s);
            parser.next();
            sym
        }
        other => {
            let other_str = format!("{:?}", other);
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(
                location,
                format!("expected pattern, got {}", other_str),
            ));
        }
    };
    if parser.peek() != Some(&Kind::DoubleColon) {
        return Ok(crate::ast::Pattern::Name(first));
    }
    parse_pattern_enum_variant_tail(parser, first)
}

fn parse_pattern_tuple(parser: &mut Parser) -> ParserResult<crate::ast::Pattern> {
    parser.next();
    let mut sub_patterns: Vec<crate::ast::Pattern> = Vec::new();
    loop {
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::ParenClose)) {
            break;
        }
        let sub = parse_match_pattern(parser)?;
        sub_patterns.push(sub);
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
        } else {
            break;
        }
    }
    parser.expect_err(&Kind::ParenClose)?;
    if sub_patterns.len() < 2 {
        let location = parser.current_source_location();
        return Err(ParserError::generic_error(
            location,
            "tuple pattern requires at least two sub-patterns".to_string(),
        ));
    }
    Ok(crate::ast::Pattern::Tuple(sub_patterns))
}

fn parse_pattern_literal(parser: &mut Parser) -> ParserResult<Option<crate::ast::Pattern>> {
    let expr_ref = match parser.peek() {
        Some(&Kind::UInt64(n)) => {
            let location = parser.current_source_location();
            parser.next();
            parser.ast_builder.uint64_expr(n, Some(location))
        }
        Some(&Kind::Int64(n)) => {
            let location = parser.current_source_location();
            parser.next();
            parser.ast_builder.int64_expr(n, Some(location))
        }
        Some(Kind::Integer(s)) => {
            let s_copy = s.to_string();
            let location = parser.current_source_location();
            parser.next();
            let sym = parser.string_interner.get_or_intern(s_copy);
            parser.ast_builder.number_expr(sym, Some(location))
        }
        Some(&Kind::True) => {
            let location = parser.current_source_location();
            parser.next();
            parser.ast_builder.bool_true_expr(Some(location))
        }
        Some(&Kind::False) => {
            let location = parser.current_source_location();
            parser.next();
            parser.ast_builder.bool_false_expr(Some(location))
        }
        Some(Kind::String(s)) => {
            let s_copy = s.to_string();
            let location = parser.current_source_location();
            parser.next();
            let sym = parser.string_interner.get_or_intern(s_copy);
            parser.ast_builder.string_expr(sym, Some(location))
        }
        _ => return Ok(None),
    };
    Ok(Some(crate::ast::Pattern::Literal(expr_ref)))
}

fn parse_pattern_enum_variant_tail(
    parser: &mut Parser,
    enum_name: DefaultSymbol,
) -> ParserResult<crate::ast::Pattern> {
    parser.expect_err(&Kind::DoubleColon)?;
    let variant = match parser.peek() {
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let sym = parser.string_interner.get_or_intern(s);
            parser.next();
            sym
        }
        other => {
            let other_str = format!("{:?}", other);
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(
                location,
                format!("expected variant name after `::`, got {}", other_str),
            ));
        }
    };
    let mut sub_patterns: Vec<crate::ast::Pattern> = Vec::new();
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        parser.next();
        loop {
            parser.skip_newlines();
            if matches!(parser.peek(), Some(Kind::ParenClose)) {
                break;
            }
            let sub = parse_match_pattern(parser)?;
            sub_patterns.push(sub);
            parser.skip_newlines();
            if matches!(parser.peek(), Some(Kind::Comma)) {
                parser.next();
            } else {
                break;
            }
        }
        parser.expect_err(&Kind::ParenClose)?;
    }
    Ok(crate::ast::Pattern::EnumVariant(enum_name, variant, sub_patterns))
}
