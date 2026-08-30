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
        // (`1i64 | 2i64 =>`), which expand into one arm each. They
        // share the body and the guard: only one of them ever runs.
        let alternatives = parse_match_pattern(parser)?;
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
        for pattern in alternatives {
            arms.push(crate::ast::MatchArm { pattern, guard, body });
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

/// The most alternatives one pattern may expand into. Slots multiply
/// (`(a|b, c|d)` is four), so an accidental cross product is caught
/// here rather than as a `match` with thousands of arms.
const MAX_PATTERN_ALTERNATIVES: usize = 64;

/// PATTERN-EXTEND: parse `pat` or `pat | pat | ...`, returning one
/// pattern per alternative. Every caller — the arms of a `match`, the
/// pattern of an `if val` / `while val`, and every sub-pattern
/// position — goes through here, which is what makes `|` legal at any
/// depth: a slot hands back its alternatives and the container it sits
/// in takes the cross product.
///
/// The alternatives share the arm's body and guard, so `1i64 | 2i64 =>
/// body` costs one body rather than a copy per alternative.
pub(crate) fn parse_match_pattern(
    parser: &mut Parser,
) -> ParserResult<Vec<crate::ast::Pattern>> {
    let location = parser.current_source_location();
    let mut out = parse_one_pattern(parser)?;
    if !matches!(parser.peek(), Some(Kind::Or)) {
        return Ok(out);
    }
    while matches!(parser.peek(), Some(Kind::Or)) {
        parser.next();
        parser.skip_newlines();
        out.extend(parse_one_pattern(parser)?);
    }
    check_alternatives_bind_alike(parser, &out, location);
    cap_alternatives(parser, &mut out, location);
    Ok(out)
}

/// Every alternative of a `|` must bind the same names: only one of
/// them runs, and they share a body, so a name that some of them miss
/// would be unreadable half the time. Catching it here beats the
/// diagnostic the body would otherwise produce ("undefined variable",
/// pointing at the body rather than the pattern).
fn check_alternatives_bind_alike(
    parser: &mut Parser,
    alternatives: &[crate::ast::Pattern],
    location: crate::type_checker::SourceLocation,
) {
    let names_of = |pat: &crate::ast::Pattern| {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        names.sort_unstable();
        names.dedup();
        names
    };
    let first = names_of(&alternatives[0]);
    for alt in &alternatives[1..] {
        if names_of(alt) != first {
            // Reported rather than returned: an `Err` from this depth
            // is replaced by whatever the recovery path trips over
            // next (`expected expression but found FatArrow`), and the
            // parse continues fine without one — the alternatives are
            // already built.
            parser.report_error(ParserError::generic_error(
                location,
                "the alternatives of a `|` pattern must bind the same names — \
                 only one of them runs, and they share the arm body"
                    .to_string(),
            ));
            return;
        }
    }
}

/// Report and truncate a pattern that expanded past the cap, so a
/// runaway cross product cannot turn into thousands of arms.
fn cap_alternatives(
    parser: &mut Parser,
    alternatives: &mut Vec<crate::ast::Pattern>,
    location: crate::type_checker::SourceLocation,
) {
    if alternatives.len() <= MAX_PATTERN_ALTERNATIVES {
        return;
    }
    parser.report_error(ParserError::generic_error(
        location,
        format!(
            "this pattern expands into {} alternatives (limit {}) — \
             nested `|` positions multiply",
            alternatives.len(),
            MAX_PATTERN_ALTERNATIVES
        ),
    ));
    alternatives.truncate(MAX_PATTERN_ALTERNATIVES);
}

fn collect_pattern_names(pat: &crate::ast::Pattern, out: &mut Vec<DefaultSymbol>) {
    use crate::ast::Pattern;
    match pat {
        Pattern::Name(s) => out.push(*s),
        Pattern::Binding(s, inner) => {
            out.push(*s);
            collect_pattern_names(inner, out);
        }
        Pattern::EnumVariant(_, _, subs) | Pattern::Tuple(subs) => {
            for sp in subs {
                collect_pattern_names(sp, out);
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (_, sp) in fields {
                collect_pattern_names(sp, out);
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Range(_, _) => {}
    }
}

/// One pattern per combination of its slots' alternatives, in slot
/// order — `(a|b, c|d)` gives `(a,c) (a,d) (b,c) (b,d)`. The slots are
/// already parsed, so this is pure recombination.
fn expand_slots(slots: Vec<Vec<crate::ast::Pattern>>) -> Vec<Vec<crate::ast::Pattern>> {
    let mut out: Vec<Vec<crate::ast::Pattern>> = vec![Vec::with_capacity(slots.len())];
    for slot in slots {
        let mut next = Vec::with_capacity(out.len() * slot.len());
        for prefix in &out {
            for alt in &slot {
                let mut row = prefix.clone();
                row.push(alt.clone());
                next.push(row);
            }
        }
        out = next;
    }
    out
}

/// True when the next tokens are `<literal> ..`, i.e. a range pattern
/// rather than a plain literal. Two tokens of lookahead is enough:
/// range endpoints are integer literals.
fn pattern_starts_a_range(parser: &mut Parser) -> bool {
    let starts_literal = matches!(
        parser.peek(),
        Some(Kind::UInt64(_) | Kind::Int64(_) | Kind::Integer(_) | Kind::CharLiteral(_))
    );
    starts_literal && matches!(parser.peek_n(1), Some(Kind::DotDot))
}

/// One pattern, with no `|` at its own level. It still returns a
/// list, because a `|` nested inside it — `Circle(1i64 | 2i64)` —
/// expands the whole pattern.
fn parse_one_pattern(parser: &mut Parser) -> ParserResult<Vec<crate::ast::Pattern>> {
    // PATTERN-EXTEND: `name @ pat`, at any depth — `Some(n @ 3i64)`
    // reads the payload and tests it in one pattern.
    if let Some(Kind::Identifier(name)) = parser.peek().cloned()
        && name != "_"
        && matches!(parser.peek_n(1), Some(Kind::At))
    {
        let sym = parser.string_interner.get_or_intern(name);
        parser.next(); // name
        parser.next(); // `@`
        // `parse_one_pattern`, not the list form: `n @ a | b` binds
        // only `a`, matching how `|` binds looser than `@`.
        return Ok(parse_one_pattern(parser)?
            .into_iter()
            .map(|inner| crate::ast::Pattern::Binding(sym, Box::new(inner)))
            .collect());
    }
    if let Some(Kind::Identifier(s)) = parser.peek()
        && s == "_" {
            parser.next();
            return Ok(vec![crate::ast::Pattern::Wildcard]);
        }
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        return parse_pattern_tuple(parser);
    }
    // PATTERN-EXTEND: `lo..hi`, half-open like the `..` expression
    // form. Checked before the plain literal, which is the same first
    // token.
    if pattern_starts_a_range(parser) {
        let location = parser.current_source_location();
        let Some(crate::ast::Pattern::Literal(low)) = parse_pattern_literal(parser)? else {
            unreachable!("pattern_starts_a_range checked for an integer literal")
        };
        parser.expect_err(&Kind::DotDot)?;
        let Some(crate::ast::Pattern::Literal(high)) = parse_pattern_literal(parser)? else {
            return Err(ParserError::generic_error(
                location,
                "expected an integer literal after `..` in a range pattern".to_string(),
            ));
        };
        return Ok(vec![crate::ast::Pattern::Range(low, high)]);
    }
    if let Some(pat) = parse_pattern_literal(parser)? {
        return Ok(vec![pat]);
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
    // NEWTYPE: `Meters(v)` -- the pattern form of a tuple struct. Same
    // disambiguation as the `{` above: a bare name in pattern position
    // is followed by `=>` or a guard, never by `(`. Whether `first`
    // really is a tuple struct is the type checker's call; an enum
    // variant is spelled with `::` and took the branch below.
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        return parse_pattern_tuple_struct(parser, first);
    }
    if parser.peek() != Some(&Kind::DoubleColon) {
        return Ok(vec![crate::ast::Pattern::Name(first)]);
    }
    parse_pattern_enum_variant_tail(parser, first)
}

/// NEWTYPE: the `( ... )` half of `Meters(v)`.
///
/// A tuple struct's fields are named by their index, so the pattern
/// lowers to the same `Pattern::Struct` a named struct produces --
/// exhaustiveness, reachability and every backend's matching code are
/// shared with `Point { x, y }` rather than reimplemented.
///
/// `..` is accepted in trailing position exactly as it is there, so
/// `Sample(v, ..)` binds the first field and ignores the rest.
fn parse_pattern_tuple_struct(
    parser: &mut Parser,
    name: DefaultSymbol,
) -> ParserResult<Vec<crate::ast::Pattern>> {
    let location = parser.current_source_location();
    parser.expect_err(&Kind::ParenOpen)?;
    let mut field_names: Vec<DefaultSymbol> = Vec::new();
    let mut slots: Vec<Vec<crate::ast::Pattern>> = Vec::new();
    let mut has_rest = false;
    loop {
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::ParenClose)) {
            break;
        }
        if matches!(parser.peek(), Some(Kind::DotDot)) {
            parser.next();
            has_rest = true;
            parser.skip_newlines();
            break;
        }
        let field = parser
            .string_interner
            .get_or_intern(field_names.len().to_string().as_str());
        field_names.push(field);
        slots.push(parse_match_pattern(parser)?);
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
        } else {
            break;
        }
    }
    parser.skip_newlines();
    parser.expect_err(&Kind::ParenClose)?;
    let mut out: Vec<crate::ast::Pattern> = expand_slots(slots)
        .into_iter()
        .map(|row| {
            crate::ast::Pattern::Struct(
                name,
                field_names.iter().copied().zip(row).collect(),
                has_rest,
            )
        })
        .collect();
    cap_alternatives(parser, &mut out, location);
    Ok(out)
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
) -> ParserResult<Vec<crate::ast::Pattern>> {
    let location = parser.current_source_location();
    parser.expect_err(&Kind::BraceOpen)?;
    let mut field_names: Vec<DefaultSymbol> = Vec::new();
    let mut slots: Vec<Vec<crate::ast::Pattern>> = Vec::new();
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
            vec![crate::ast::Pattern::Name(field)]
        };
        field_names.push(field);
        slots.push(sub);
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
        } else {
            break;
        }
    }
    parser.skip_newlines();
    parser.expect_err(&Kind::BraceClose)?;
    let mut out: Vec<crate::ast::Pattern> = expand_slots(slots)
        .into_iter()
        .map(|row| {
            crate::ast::Pattern::Struct(
                name,
                field_names.iter().copied().zip(row).collect(),
                has_rest,
            )
        })
        .collect();
    cap_alternatives(parser, &mut out, location);
    Ok(out)
}

fn parse_pattern_tuple(parser: &mut Parser) -> ParserResult<Vec<crate::ast::Pattern>> {
    let location = parser.current_source_location();
    parser.next();
    let mut slots: Vec<Vec<crate::ast::Pattern>> = Vec::new();
    loop {
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::ParenClose)) {
            break;
        }
        slots.push(parse_match_pattern(parser)?);
        parser.skip_newlines();
        if matches!(parser.peek(), Some(Kind::Comma)) {
            parser.next();
        } else {
            break;
        }
    }
    parser.expect_err(&Kind::ParenClose)?;
    if slots.len() < 2 {
        return Err(ParserError::generic_error(
            parser.current_source_location(),
            "tuple pattern requires at least two sub-patterns".to_string(),
        ));
    }
    let mut out: Vec<crate::ast::Pattern> = expand_slots(slots)
        .into_iter()
        .map(crate::ast::Pattern::Tuple)
        .collect();
    cap_alternatives(parser, &mut out, location);
    Ok(out)
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
        // CHAR-LITERAL-NUM: `match b { 'h' => ... }` over a string's
        // bytes. The literal is `u32` like anywhere else, and the
        // type checker narrows it to the scrutinee's width when the
        // code point fits.
        Some(&Kind::CharLiteral(n)) => {
            let location = parser.current_source_location();
            parser.next();
            parser.ast_builder.char_literal_expr(n, Some(location))
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
) -> ParserResult<Vec<crate::ast::Pattern>> {
    let location = parser.current_source_location();
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
    let mut slots: Vec<Vec<crate::ast::Pattern>> = Vec::new();
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        parser.next();
        loop {
            parser.skip_newlines();
            if matches!(parser.peek(), Some(Kind::ParenClose)) {
                break;
            }
            slots.push(parse_match_pattern(parser)?);
            parser.skip_newlines();
            if matches!(parser.peek(), Some(Kind::Comma)) {
                parser.next();
            } else {
                break;
            }
        }
        parser.expect_err(&Kind::ParenClose)?;
    }
    let mut out: Vec<crate::ast::Pattern> = expand_slots(slots)
        .into_iter()
        .map(|row| crate::ast::Pattern::EnumVariant(enum_name, variant, row))
        .collect();
    cap_alternatives(parser, &mut out, location);
    Ok(out)
}
