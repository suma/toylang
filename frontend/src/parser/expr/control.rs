use crate::token::DescribeToken;
use crate::ast::*;
use crate::token::Kind;
use crate::parser::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use crate::type_checker::SourceLocation;
use super::{parse_logical_expr, parse_block, parse_match_pattern};

/// Report `else if`, which is not toylang syntax — `elif` is. Returns
/// whether it was one; the caller then reads it as `elif`.
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
fn report_else_if(parser: &mut Parser, else_location: SourceLocation) -> bool {
    if !matches!(parser.peek(), Some(Kind::If)) {
        return false;
    }
    let if_location = parser.current_source_location();
    let span = SourceLocation::new(
        else_location.line,
        else_location.column,
        else_location.offset,
        if_location.end_offset,
    );
    // A recovered error: the parse continues exactly as for `elif`, so
    // nothing later is a consequence of it. `parse_program` still
    // refuses to hand back a tree while any error is recorded, which is
    // what keeps the program from running as if it were fine.
    parser.report_recovered_error(ParserError::else_if(span));
    true
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
    let else_block: ExprRef = loop {
        match parser.peek() {
            Some(Kind::Elif) => {
                parser.next();
            }
            Some(Kind::Else) => {
                let else_location = parser.current_source_location();
                parser.next();
                // `else if` is reported, then read as the `elif` it
                // means, so the rest of the chain — and of the file —
                // parses as written (LLM-TOOLING #6).
                if !report_else_if(parser, else_location) {
                    break parse_block(parser)?;
                }
                parser.next(); // `if`
            }
            _ => {
                let location = parser.current_source_location();
                break parser.ast_builder.block_expr(vec![], Some(location));
            }
        }
        parser.push_context(crate::parser::core::ParseContext::Condition);
        let elif_cond = parse_logical_expr(parser)?;
        parser.pop_context();
        let elif_block = parse_block(parser)?;
        elif_pairs.push((elif_cond, elif_block));
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
    let sites = parser.pattern_sites.len();
    let patterns = parse_match_pattern(parser)?;
    let bound = parser.pattern_sites.len();
    parser.expect_err(&Kind::Equal)?;
    parser.push_context(crate::parser::core::ParseContext::Condition);
    let scrutinee = parse_logical_expr(parser)?;
    parser.pop_context();
    let then_block = parse_block(parser)?;
    parser.scope_pattern_bindings(sites, bound);
    let (then_arm_body, else_arm_body): (ExprRef, ExprRef) = match parser.peek() {
        Some(Kind::Else) => {
            let else_location = parser.current_source_location();
            parser.next();
            let else_block = if report_else_if(parser, else_location) {
                parser.next(); // `if`: read the rest as the `elif` it means
                parse_if(parser)?
            } else {
                parse_block(parser)?
            };
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
                format!("expected `allocator` after `with`, found {}", other_cloned.describe_token()),
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
