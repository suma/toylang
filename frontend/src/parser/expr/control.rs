use crate::ast::*;
use crate::token::Kind;
use crate::parser::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use crate::type_decl::TypeDecl;
use super::{parse_logical_expr, parse_block, parse_match_pattern};

/// Parse `dict{key: value, ...}` literal.
pub fn parse_dict_literal(parser: &mut Parser) -> ParserResult<ExprRef> {
    let location = parser.current_source_location();
    parser.expect_err(&Kind::BraceOpen)?;
    parser.skip_newlines();
    if parser.peek() == Some(&Kind::BraceClose) {
        parser.next();
        return Ok(parser.ast_builder.dict_literal_expr(vec![], Some(location)));
    }
    let entries = parse_dict_entries(parser, vec![])?;
    parser.skip_newlines();
    parser.expect_err(&Kind::BraceClose)?;
    Ok(parser.ast_builder.dict_literal_expr(entries, Some(location)))
}

fn parse_dict_entries(parser: &mut Parser, mut entries: Vec<(ExprRef, ExprRef)>) -> ParserResult<Vec<(ExprRef, ExprRef)>> {
    loop {
        parser.skip_newlines();
        let key = parser.parse_expr_impl()?;
        parser.skip_newlines();
        parser.expect_err(&Kind::Colon)?;
        parser.skip_newlines();
        let value = parser.parse_expr_impl()?;
        entries.push((key, value));
        parser.skip_newlines();
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                parser.skip_newlines();
                if parser.peek() == Some(&Kind::BraceClose) {
                    break;
                }
                continue;
            }
            Some(Kind::BraceClose) => break,
            _ => {
                parser.collect_error("Expected ',' or '}' in dict literal");
                break;
            }
        }
    }
    Ok(entries)
}

/// Parse `if` / `elif` / `else` expression.
pub fn parse_if(parser: &mut Parser) -> ParserResult<ExprRef> {
    if matches!(parser.peek(), Some(Kind::Val)) {
        return parse_if_val(parser);
    }
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let cond = parse_logical_expr(parser)?;
    parser.pop_context();
    let if_block = parse_block(parser)?;
    let mut elif_pairs = Vec::new();
    while let Some(Kind::Elif) = parser.peek() {
        parser.next();
        parser.push_context(crate::parser::core::ParseContext::Condition);
        let elif_cond = parse_logical_expr(parser)?;
        parser.pop_context();
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

/// Parse `if val PAT = EXPR { THEN } [else { ELSE }]` — desugars to match.
fn parse_if_val(parser: &mut Parser) -> ParserResult<ExprRef> {
    let start_location = parser.current_source_location();
    parser.expect_err(&Kind::Val)?;
    let pattern = parse_match_pattern(parser)?;
    parser.expect_err(&Kind::Equal)?;
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let scrutinee = parse_logical_expr(parser)?;
    parser.pop_context();
    let then_block = parse_block(parser)?;
    let (then_arm_body, else_arm_body): (ExprRef, ExprRef) = match parser.peek() {
        Some(Kind::Else) => {
            parser.next();
            let else_block = parse_block(parser)?;
            (then_block, else_block)
        }
        _ => {
            let counter = parser.synthetic_counter;
            parser.synthetic_counter += 1;
            let dummy_name = format!("__ifval_dummy_{counter}");
            let dummy_sym = parser.string_interner.get_or_intern(dummy_name.as_str());
            let then_val_stmt = parser.ast_builder.val_stmt(
                dummy_sym,
                Some(TypeDecl::Unknown),
                then_block,
                Some(start_location),
            );
            let then_wrapped = parser
                .ast_builder
                .block_expr(vec![then_val_stmt], Some(start_location));
            let else_empty = parser
                .ast_builder
                .block_expr(vec![], Some(start_location));
            (then_wrapped, else_empty)
        }
    };
    let arms = vec![
        crate::ast::MatchArm { pattern, guard: None, body: then_arm_body },
        crate::ast::MatchArm {
            pattern: crate::ast::Pattern::Wildcard,
            guard: None,
            body: else_arm_body,
        },
    ];
    let match_expr = parser.ast_builder.add_expr_with_location(
        crate::ast::Expr::Match(scrutinee, arms),
        Some(start_location),
    );
    Ok(match_expr)
}

/// Parse `with allocator = expr { body }`.
pub fn parse_with(parser: &mut Parser) -> ParserResult<ExprRef> {
    let location = parser.current_source_location();
    match parser.peek() {
        Some(Kind::Identifier(name)) if name.as_str() == "allocator" => {
            parser.next();
        }
        other => {
            let other_cloned = other.cloned();
            return Err(ParserError::generic_error(
                location,
                format!("expected `allocator` after `with`, found {:?}", other_cloned),
            ));
        }
    }
    parser.expect_err(&Kind::Equal)?;
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let allocator_expr = parse_logical_expr(parser)?;
    parser.pop_context();
    let body = parse_block(parser)?;
    let location = parser.current_source_location();
    Ok(parser.ast_builder.with_expr(allocator_expr, body, Some(location)))
}
