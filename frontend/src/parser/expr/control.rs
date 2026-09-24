use crate::ast::*;
use crate::token::Kind;
use crate::parser::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use crate::type_checker::SourceLocation;
use super::{parse_logical_expr, parse_block, parse_match_pattern};

/// Reject `else if`, which is not toylang syntax — `elif` is.
///
/// Called with the `else` already consumed and `else_location` pointing
/// at it. Without this check the `else` arm hands an `if` token to
/// `parse_block`, which fails, and the parser's error recovery swallows
/// the rest of the file. The result is a diagnostic that blames
/// something innocent:
///
///   * a function declared after the `else if` simply disappears, and
///     the user is told `Function 'main' not found`
///   * when nothing follows, the mis-parse survives to runtime and
///     surfaces as `Internal error: Null reference error` — and only
///     when the first condition is false, so it can sit undetected
///
/// The span covers `else if` as a unit so the caret marks exactly what
/// has to be replaced.
fn reject_else_if(parser: &mut Parser, else_location: SourceLocation) -> ParserResult<()> {
    if !matches!(parser.peek(), Some(Kind::If)) {
        return Ok(());
    }
    let if_location = parser.current_source_location();
    let span = SourceLocation::new(
        else_location.line,
        else_location.column,
        else_location.offset,
        if_location.end_offset,
    );
    let error = ParserError::generic_error(
        span,
        "`else if` is not supported; write `elif` instead (`} elif cond {`)".to_string(),
    );
    // Recorded as well as returned. Expression parsing has recovery
    // paths that swallow a returned `Err` and carry on, which would put
    // us right back to a silent mis-parse; `parse_program` refuses to
    // hand back a tree while `errors` is non-empty, so this is what
    // makes the rejection stick.
    parser.report_error(error.clone());
    Err(error)
}

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
    let location = parser.span_to_cursor(location);
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
            let else_location = parser.current_source_location();
            parser.next();
            reject_else_if(parser, else_location)?;
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
    // PATTERN-EXTEND: `if val A | B = x` gets one arm per
    // alternative, the same expansion a `match` arm does.
    let patterns = parse_match_pattern(parser)?;
    parser.expect_err(&Kind::Equal)?;
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let scrutinee = parse_logical_expr(parser)?;
    parser.pop_context();
    let then_block = parse_block(parser)?;
    let (then_arm_body, else_arm_body): (ExprRef, ExprRef) = match parser.peek() {
        Some(Kind::Else) => {
            let else_location = parser.current_source_location();
            parser.next();
            reject_else_if(parser, else_location)?;
            let else_block = parse_block(parser)?;
            (then_block, else_block)
        }
        _ => {
            // No `else`: the arm must be `()` whatever the block's
            // value, so the block runs as a statement and a trailing
            // `()` is the arm's value. This used to discard the value
            // through `val __ifval_dummy_N: Unknown = { .. }`, which the
            // compiled lanes cannot lower when the block is itself `()`
            // (an assignment) -- "could not infer scalar type for
            // val/var rhs" for `if val Some(v) = o { x = v }`.
            let then_stmt = parser.ast_builder.expression_stmt(then_block, Some(start_location));
            let unit = parser.ast_builder.tuple_literal_expr(vec![], Some(start_location));
            let unit_stmt = parser.ast_builder.expression_stmt(unit, Some(start_location));
            let then_wrapped = parser
                .ast_builder
                .block_expr(vec![then_stmt, unit_stmt], Some(start_location));
            let else_empty = parser
                .ast_builder
                .block_expr(vec![], Some(start_location));
            (then_wrapped, else_empty)
        }
    };
    let mut arms: Vec<crate::ast::MatchArm> = patterns
        .into_iter()
        .map(|pattern| crate::ast::MatchArm {
            pattern,
            guard: None,
            body: then_arm_body,
        })
        .collect();
    arms.push(crate::ast::MatchArm {
        pattern: crate::ast::Pattern::Wildcard,
        guard: None,
        body: else_arm_body,
    });
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
