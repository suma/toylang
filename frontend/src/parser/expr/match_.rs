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
        // PATTERN-EXTEND: one arm can carry several alternatives
        // (`1i64 | 2i64 =>`), and a range or `@` binding contributes a
        // synthesized guard. Both lower onto the existing arm shape,
        // so nothing downstream of the parser learns a new pattern
        // form.
        let alternatives = parse_match_pattern_alternatives(parser)?;
        let user_guard = if matches!(parser.peek(), Some(Kind::If)) {
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
        for alt in alternatives {
            let guard = combine_guards(parser, alt.guard, user_guard);
            arms.push(crate::ast::MatchArm {
                pattern: alt.pattern,
                guard,
                body,
            });
        }
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

/// PATTERN-EXTEND: one alternative of an arm's pattern list, plus the
/// guard the parser had to synthesize for it.
///
/// A range (`0i64..5i64`) and an `@` binding (`n @ 2i64`) are both
/// expressed as an irrefutable `Name` pattern with a comparison guard,
/// so the type checker and all three backends keep seeing only the
/// pattern forms they already handle. `Name` is irrefutable, but a
/// guarded arm never counts as exhaustive, so a `_` arm is still
/// required exactly where it was before.
struct PatternAlternative {
    pattern: crate::ast::Pattern,
    guard: Option<ExprRef>,
}

/// Parse `pat` or `pat | pat | ...`, expanding each alternative into
/// its own arm. Alternatives share the body: only one arm ever runs,
/// and the shared `ExprRef` is what makes `1i64 | 2i64 => body` cost
/// one body rather than a copy per alternative.
fn parse_match_pattern_alternatives(
    parser: &mut Parser,
) -> ParserResult<Vec<PatternAlternative>> {
    let mut out = vec![parse_match_pattern_with_guard(parser)?];
    while matches!(parser.peek(), Some(Kind::Or)) {
        parser.next();
        parser.skip_newlines();
        out.push(parse_match_pattern_with_guard(parser)?);
    }
    Ok(out)
}

/// `a && b`, or whichever half exists.
fn combine_guards(
    parser: &mut Parser,
    synthesized: Option<ExprRef>,
    user: Option<ExprRef>,
) -> Option<ExprRef> {
    match (synthesized, user) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => Some(parser.ast_builder.binary_expr(
            Operator::LogicalAnd,
            a,
            b,
            None,
        )),
    }
}

/// One alternative of an arm's pattern, plus any guard the parser had
/// to synthesize for it.
///
/// Ranges are the only shape that needs one: `0i64..5i64` and
/// `n @ 0i64..5i64` become an irrefutable name plus a comparison,
/// because there is no range pattern form. Everything else — `@`
/// included — is a pattern the rest of the compiler understands.
fn parse_match_pattern_with_guard(
    parser: &mut Parser,
) -> ParserResult<PatternAlternative> {
    // `name @ <range>` — the one `@` shape that is still a guard,
    // because a range has no pattern form of its own. Every other `@`
    // is a `Pattern::Binding`, which `parse_match_pattern` recognises
    // at any depth.
    if at_binding_over_a_range(parser) {
        let location = parser.current_source_location();
        let Some(Kind::Identifier(name)) = parser.peek().cloned() else {
            unreachable!("at_binding_over_a_range checked for an identifier")
        };
        let sym = parser.string_interner.get_or_intern(name);
        parser.next(); // name
        parser.next(); // `@`
        let bound = parser.ast_builder.identifier_expr(sym, Some(location));
        let guard = parse_range_guard(parser, bound)?.ok_or_else(|| {
            ParserError::generic_error(location, "expected a range after `@`".to_string())
        })?;
        return Ok(PatternAlternative {
            pattern: crate::ast::Pattern::Name(sym),
            guard: Some(guard),
        });
    }

    // A bare range needs a name to compare against, so it binds the
    // scrutinee to a fresh one the user cannot collide with.
    if pattern_starts_a_range(parser) {
        let location = parser.current_source_location();
        let sym = parser.fresh_pattern_binding();
        let bound = parser.ast_builder.identifier_expr(sym, Some(location));
        let guard = parse_range_guard(parser, bound)?.ok_or_else(|| {
            ParserError::generic_error(location, "expected a range pattern".to_string())
        })?;
        return Ok(PatternAlternative {
            pattern: crate::ast::Pattern::Name(sym),
            guard: Some(guard),
        });
    }

    Ok(PatternAlternative {
        pattern: parse_match_pattern(parser)?,
        guard: None,
    })
}

/// True when the tokens ahead are `name @ <literal> ..`. Four tokens
/// of lookahead so the decision is made before anything is consumed:
/// every other `@` shape is a pattern and takes the ordinary path.
fn at_binding_over_a_range(parser: &mut Parser) -> bool {
    matches!(parser.peek(), Some(Kind::Identifier(n)) if n != "_")
        && matches!(parser.peek_n(1), Some(Kind::At))
        && matches!(
            parser.peek_n(2),
            Some(Kind::UInt64(_) | Kind::Int64(_) | Kind::Integer(_))
        )
        && matches!(parser.peek_n(3), Some(Kind::DotDot))
}

/// True when the next tokens are `<literal> ..`, i.e. a range pattern
/// rather than a plain literal. Two tokens of lookahead is enough:
/// range endpoints are literals.
fn pattern_starts_a_range(parser: &mut Parser) -> bool {
    let starts_literal = matches!(
        parser.peek(),
        Some(Kind::UInt64(_) | Kind::Int64(_) | Kind::Integer(_))
    );
    starts_literal && matches!(parser.peek_n(1), Some(Kind::DotDot))
}

/// Build the comparison guard for a range appearing at a value
/// position, comparing against `bound` (the name the arm binds the
/// scrutinee to). Returns `None` when the next tokens are not a
/// literal, so the caller can report a shape it cannot express.
///
/// Both callers have already established that a range starts here
/// (`pattern_starts_a_range`), so the `..` is not in doubt — a lone
/// literal is a `Pattern::Literal` and never reaches this function.
fn parse_range_guard(
    parser: &mut Parser,
    bound: ExprRef,
) -> ParserResult<Option<ExprRef>> {
    let location = parser.current_source_location();
    let Some(crate::ast::Pattern::Literal(low)) = parse_pattern_literal(parser)? else {
        return Ok(None);
    };
    parser.expect_err(&Kind::DotDot)?;
    let Some(crate::ast::Pattern::Literal(high)) = parse_pattern_literal(parser)? else {
        // Same recovery reasoning as the `@` arm above: record the
        // diagnostic, then let the caller carry on with a guard that
        // only checks the lower bound.
        let here = parser.current_source_location();
        parser.report_error(ParserError::generic_error(
            here,
            "expected a literal after `..` in a range pattern".to_string(),
        ));
        return Ok(Some(parser.ast_builder.binary_expr(
            Operator::GE,
            bound,
            low,
            Some(location),
        )));
    };
    // Half-open, matching the `..` expression form: `0i64..5i64`
    // covers 0 through 4.
    let ge = parser.ast_builder.binary_expr(Operator::GE, bound, low, Some(location));
    let lt = parser.ast_builder.binary_expr(Operator::LT, bound, high, Some(location));
    Ok(Some(parser.ast_builder.binary_expr(
        Operator::LogicalAnd,
        ge,
        lt,
        Some(location),
    )))
}

pub(crate) fn parse_match_pattern(parser: &mut Parser) -> ParserResult<crate::ast::Pattern> {
    // PATTERN-EXTEND: `name @ pat`, at any depth — `Some(n @ 3i64)`
    // reads the payload and tests it in one pattern.
    if let Some(Kind::Identifier(name)) = parser.peek().cloned()
        && name != "_"
        && matches!(parser.peek_n(1), Some(Kind::At))
    {
        let sym = parser.string_interner.get_or_intern(name);
        parser.next(); // name
        parser.next(); // `@`
        let inner = parse_match_pattern(parser)?;
        return Ok(crate::ast::Pattern::Binding(sym, Box::new(inner)));
    }
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
    // PATTERN-STRUCT: `Point { x: 0i64, y }`. A `{` directly after the
    // name is unambiguous here — a pattern is always followed by `=>`
    // or a guard, never by a block.
    if matches!(parser.peek(), Some(Kind::BraceOpen)) {
        return parse_pattern_struct(parser, first);
    }
    if parser.peek() != Some(&Kind::DoubleColon) {
        return Ok(crate::ast::Pattern::Name(first));
    }
    parse_pattern_enum_variant_tail(parser, first)
}

/// PATTERN-STRUCT: the `{ ... }` half of `Point { x: 0i64, y }`.
///
/// `x: <pattern>` matches the field against a pattern; the shorthand
/// `x` binds the field to a name of its own, which is stored as
/// `x: Name(x)` so everything downstream sees one shape. A trailing
/// `..` means the unlisted fields are not examined.
fn parse_pattern_struct(
    parser: &mut Parser,
    name: DefaultSymbol,
) -> ParserResult<crate::ast::Pattern> {
    parser.expect_err(&Kind::BraceOpen)?;
    let mut fields: Vec<(DefaultSymbol, crate::ast::Pattern)> = Vec::new();
    let mut has_rest = false;
    loop {
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::BraceClose)) {
            break;
        }
        // `..` — ignore whatever was not named.
        if matches!(parser.peek(), Some(Kind::DotDot)) {
            parser.next();
            has_rest = true;
            parser.skip_newlines();
            break;
        }
        let field = match parser.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                parser.next();
                parser.string_interner.get_or_intern(s)
            }
            other => {
                let other_str = format!("{:?}", other);
                let location = parser.current_source_location();
                return Err(ParserError::generic_error(
                    location,
                    format!("expected a field name in struct pattern, got {other_str}"),
                ));
            }
        };
        let sub = if matches!(parser.peek(), Some(Kind::Colon)) {
            parser.next();
            parser.skip_newlines();
            parse_match_pattern(parser)?
        } else {
            crate::ast::Pattern::Name(field)
        };
        fields.push((field, sub));
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
        } else {
            break;
        }
    }
    parser.skip_newlines();
    parser.expect_err(&Kind::BraceClose)?;
    Ok(crate::ast::Pattern::Struct(name, fields, has_rest))
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
