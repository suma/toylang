use crate::ast::*;
use crate::token::{Kind, StringPart};
use crate::format_spec::FormatSpec;
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
    // INTERP-DIAG-SPAN: the literal's own span, used for the tokens
    // this desugaring invents (`concat`, the builtin name, parens).
    // Sub-expression tokens get their real position instead — see
    // below — so only the scaffolding points at the literal as a
    // whole.
    let literal_span = parser
        .peek_position_n(0)
        .cloned()
        .unwrap_or(0..0);
    // The scaffolding tokens sit at the literal's *closing* quote
    // rather than its start. Position-sensitive parse rules read the
    // source immediately before a token — `has_newline_before_current_token`
    // decides whether a `(` opens this field's argument list or a new
    // statement — and a literal that starts a line (`\n    "a {b}"`)
    // would otherwise make the synthesized `.concat(` look like a
    // fresh expression. Anchoring at the closing quote puts a
    // non-whitespace byte immediately before every synthesized token.
    let scaffold_span = literal_span.end.saturating_sub(1)..literal_span.end;
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

    // (token, span) pairs — see `literal_span` / `scaffold_span` above.
    let mut tokens: Vec<(Kind, std::ops::Range<usize>)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            tokens.push((Kind::Dot, scaffold_span.clone()));
            tokens.push((Kind::Identifier("concat".to_string()), scaffold_span.clone()));
            tokens.push((Kind::ParenOpen, scaffold_span.clone()));
        }
        match part {
            StringPart::Literal(s) => {
                tokens.push((Kind::String(s.clone()), scaffold_span.clone()));
            }
            StringPart::Expr { text: expr_text, spec, offset } => {
                // STR-INTERP-FMT: a segment carrying a spec lowers to
                // `__builtin_format(expr, <packed>)` instead of
                // `__builtin_to_string(expr)`. The spec is constant
                // here, so it is validated and packed at parse time —
                // the backends only ever see one extra u64 argument.
                // A spec that asks for nothing (`"{x:}"`) keeps the
                // plain `to_string` shape.
                let packed = match spec {
                    Some(spec_text) => match FormatSpec::parse(spec_text) {
                        Ok(parsed) if parsed.is_default() => None,
                        Ok(parsed) => Some(parsed.pack()),
                        Err(reason) => {
                            // Collect rather than bail: returning `Err`
                            // here leaves the synthesized token stream
                            // half-built, and the recovery path then
                            // reports whatever it trips over next
                            // instead of the spec that is actually
                            // wrong. Recording the diagnostic and
                            // rendering the segment without a spec
                            // keeps the rest of the parse honest.
                            let location = parser.location_from_span(&literal_span);
                            parser.report_error(ParserError::unexpected_token(
                                location,
                                format!(
                                    "invalid format spec `{{{}:{}}}`: {}",
                                    expr_text, spec_text, reason
                                ),
                            ));
                            None
                        }
                    },
                    None => None,
                };
                let builtin_name = if packed.is_some() {
                    "__builtin_format"
                } else {
                    "__builtin_to_string"
                };
                tokens.push((Kind::Identifier(builtin_name.to_string()), scaffold_span.clone()));
                tokens.push((Kind::ParenOpen, scaffold_span.clone()));
                let mut sub_lex = crate::parser::core::lexer::Lexer::new(expr_text, 1, None);
                loop {
                    match sub_lex.yylex() {
                        Ok(tok) => {
                            if matches!(tok.kind, Kind::NewLine | Kind::Comment(_)) {
                                continue;
                            }
                            // INTERP-DIAG-SPAN: the sub-lexer counts
                            // from zero within `expr_text`; shift by
                            // where that text starts in the file so
                            // diagnostics land on the sub-expression
                            // the user wrote.
                            let span = (offset + tok.position.start)..(offset + tok.position.end);
                            tokens.push((tok.kind, span));
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
                if let Some(code) = packed {
                    tokens.push((Kind::Comma, scaffold_span.clone()));
                    tokens.push((Kind::UInt64(code), scaffold_span.clone()));
                }
                tokens.push((Kind::ParenClose, scaffold_span.clone()));
            }
        }
        if i > 0 {
            tokens.push((Kind::ParenClose, scaffold_span.clone()));
        }
    }

    for (tok, span) in tokens.into_iter().rev() {
        parser.insert_token_at(tok, span);
    }

    parse_postfix(parser)
}

/// Parse `(a, b)` tuple or `(expr)` grouped expression.
fn parse_tuple_or_grouped_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let open = parser.current_source_location();
    parser.next();
    parser.skip_newlines();
    if parser.peek() == Some(&Kind::ParenClose) {
        parser.next();
        let location = parser.span_to_cursor(open);
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
        let location = parser.span_to_cursor(open);
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
            // LLM-LOOP P2: capture the identifier's own span before
            // consuming it. Everything built below (calls, indexing,
            // qualified paths) used to be located at whatever token
            // followed the name -- so a `foo(...)` diagnostic pointed
            // its caret at the `(` instead of at `foo`.
            let name_location = parser.current_source_location();
            parser.next();
            parse_primary_after_identifier(parser, s, name_location)
        }
        _ => parse_primary_atom_or_form(parser),
    }
}

/// ALLOC-CONTRACT: `old(expr)` in an `ensures` clause — the value
/// `expr` had on entry to the function.
///
/// Desugared here rather than carried as its own AST node: the
/// expression is moved to the function's `old_exprs` list and the call
/// is replaced by a reference to the synthetic binding `__old_<index>`
/// that every backend materialises on entry. Backends therefore see an
/// ordinary identifier, and `old` stays a contextual name that
/// programs can still use for their own functions and variables
/// everywhere else.
fn parse_old_snapshot(
    parser: &mut Parser,
    location: crate::type_checker::SourceLocation,
) -> ParserResult<ExprRef> {
    debug_assert!(parser.in_ensures_clause, "caller checks the context");
    parser.expect_err(&Kind::ParenOpen)?;
    // A nested `old` would snapshot the same instant as the outer one,
    // so it is refused rather than silently accepted as a no-op.
    parser.in_ensures_clause = false;
    let inner = parser.parse_expr_impl();
    parser.in_ensures_clause = true;
    let inner = inner?;
    parser.expect_err(&Kind::ParenClose)?;

    let index = parser.old_exprs.len();
    parser.old_exprs.push(inner);
    let sym = parser
        .string_interner
        .get_or_intern(format!("__old_{index}"));
    Ok(parser.ast_builder.identifier_expr(sym, Some(location)))
}

/// ALLOC-CONTRACT-SUGAR: the counter an allocation-budget clause reads,
/// or `None` for any other name.
///
/// Three axes, because they answer different questions and collapsing
/// them would make the sugar say less than the expression it replaces:
/// a zero `retains` delta also holds for a function that allocated and
/// freed, while a zero `allocates` delta means nothing was requested
/// at all.
fn alloc_budget_stat(name: &str) -> Option<crate::ast::MemStat> {
    match name {
        // "requested this much, in total"
        "allocates" => Some(crate::ast::MemStat::CumulativeBytes),
        // "did not hand back this much"
        "retains" => Some(crate::ast::MemStat::LiveBytes),
        // "asked the allocator this many times"
        "allocations" => Some(crate::ast::MemStat::AllocCount),
        _ => None,
    }
}

/// ALLOC-CONTRACT-SUGAR: `allocates(N)` / `retains(N)` /
/// `allocations(N)` in an `ensures` clause.
///
/// Desugars to `counter() <= old(counter()) + N`, reusing the
/// `old(...)` machinery for the entry snapshot. Note the shape:
/// **not** `counter() - old(counter()) <= N`, which is what one writes
/// by hand and which panics with a u64 underflow whenever the counter
/// goes *down* — a function that frees a pointer it was handed leaves
/// `live_bytes` below where it started. Moving the term to the other
/// side says the same thing and cannot wrap.
fn parse_alloc_budget(
    parser: &mut Parser,
    stat: crate::ast::MemStat,
    location: crate::type_checker::SourceLocation,
) -> ParserResult<ExprRef> {
    use crate::ast::BuiltinFunction;

    parser.expect_err(&Kind::ParenOpen)?;
    // The budget is an ordinary expression, but it is evaluated at
    // exit like the rest of the clause — `old` inside it would be a
    // different question, so the context is dropped while it parses.
    parser.in_ensures_clause = false;
    let budget = parser.parse_expr_impl();
    parser.in_ensures_clause = true;
    let budget = budget?;
    parser.expect_err(&Kind::ParenClose)?;

    let entry_read = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::MemStat(stat),
        vec![],
        Some(location),
    );
    let index = parser.old_exprs.len();
    parser.old_exprs.push(entry_read);
    let old_sym = parser
        .string_interner
        .get_or_intern(format!("__old_{index}"));
    let entry_value = parser.ast_builder.identifier_expr(old_sym, Some(location));

    let limit = parser.ast_builder.binary_expr(
        Operator::IAdd,
        entry_value,
        budget,
        Some(location),
    );
    let current = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::MemStat(stat),
        vec![],
        Some(location),
    );
    let clause = parser
        .ast_builder
        .binary_expr(Operator::LE, current, limit, Some(location));
    parser.last_alloc_budget = Some((
        clause,
        crate::ast::EnsuresKind::AllocBudget { stat, old_index: index },
    ));
    Ok(clause)
}

/// Parse what follows an identifier head in primary position.
fn parse_primary_after_identifier(
    parser: &mut Parser,
    name: DefaultSymbol,
    name_location: crate::type_checker::SourceLocation,
) -> ParserResult<ExprRef> {
    // ALLOC-CONTRACT: `old(...)` is contextual — only a call spelled
    // `old` directly inside an `ensures` clause means the snapshot.
    if parser.peek() == Some(&Kind::ParenOpen) && parser.in_ensures_clause {
        let spelling = parser.string_interner.resolve(name).map(str::to_string);
        if spelling.as_deref() == Some("old") {
            return parse_old_snapshot(parser, name_location);
        }
        // ALLOC-CONTRACT-SUGAR: same contextual treatment — a program
        // that already has a function called `allocates` keeps it
        // everywhere except inside an `ensures` clause.
        if let Some(stat) = spelling.as_deref().and_then(alloc_budget_stat) {
            return parse_alloc_budget(parser, stat, name_location);
        }
    }
    if parser.peek() == Some(&Kind::DoubleColon) {
        // POINTER P1: the builtin type-argument form,
        // `__builtin_sizeof::<T>()`. Only `sizeof` takes a type
        // argument today, so the interception is keyed on the builtin
        // symbol: any other `name::<` still flows into the
        // qualified-path branch below and reports its own error.
        // The type is parsed without generic context, so a generic
        // parameter arrives as `TypeDecl::Identifier(T)` — every
        // backend resolves that through its active substitution, the
        // same way a named type argument resolves through the
        // struct / enum tables.
        if parser.peek_n(1) == Some(&Kind::LT)
            && matches!(
                parser.builtin_symbols.symbol_to_builtin(name),
                Some(BuiltinFunction::SizeOf)
            )
        {
            parser.next(); // consume `::`
            parser.next(); // consume `<`
            let empty_generic_context = std::collections::HashSet::new();
            let ty = parser.parse_type_declaration_with_generic_context(&empty_generic_context)?;
            parser.expect_err(&Kind::GT)?;
            parser.expect_err(&Kind::ParenOpen)?;
            parser.expect_err(&Kind::ParenClose)?;
            // Span the whole `__builtin_sizeof::<T>()`, not just the name.
            let location = parser.span_to_cursor(name_location);
            return Ok(parser.ast_builder.builtin_call_expr(
                BuiltinFunction::SizeOfType(ty),
                vec![],
                Some(location),
            ));
        }
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
                let location = name_location;
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
                // Span the whole path (`math::add`), not the token
                // after it — same defect as the bare-identifier arm.
                let location = parser.span_to_cursor(name_location);
                Ok(parser.ast_builder.qualified_identifier_expr(qualified_path, Some(location)))
            }
        };
    }

    let struct_literal_allowed = parser.is_struct_literal_allowed();
    match parser.peek() {
        Some(Kind::ParenOpen) => {
            let location = name_location;
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
            let access = parse_bracket_access(parser, object_ref, location)?;
            // `a[0]` never reaches the postfix loop's re-spanning pass,
            // so widen it here: located at the `[`, it quotes as `[`.
            let span = parser.span_to_cursor(name_location);
            parser.ast_builder.get_location_pool_mut().set_expr_location(&access, span);
            Ok(access)
        }
        Some(Kind::BraceOpen) if struct_literal_allowed => {
            parser.next();
            let (fields, base) = parse_struct_literal_fields(parser, vec![])?;
            parser.expect_err(&Kind::BraceClose)?;
            // From the type name through the closing brace, so the
            // caret covers `P { x: 1u64 }` rather than the `{`.
            let location = parser.span_to_cursor(name_location);
            match base {
                // STRUCT-UPDATE: `P { x: 1i64, ..base }`. The parser
                // cannot fill in the omitted fields (the struct may be
                // declared later, or imported), so it emits a node the
                // type checker desugars once it knows the field list.
                // The synthetic binding is pre-interned here because
                // the type checker holds an immutable interner.
                Some(base) => {
                    let counter = parser.synthetic_counter;
                    parser.synthetic_counter += 1;
                    let base_binding = parser
                        .string_interner
                        .get_or_intern(format!("__su_{}", counter).as_str());
                    Ok(parser.ast_builder.struct_update_expr(
                        name,
                        fields,
                        base,
                        base_binding,
                        Some(location),
                    ))
                }
                None => Ok(parser.ast_builder.struct_literal_expr(name, fields, Some(location))),
            }
        }
        _ => {
            // LLM-LOOP P2 (completing it): the name was consumed by the
            // caller, so `current_source_location()` here is the *next*
            // token — every diagnostic about a bare identifier pointed
            // one token to the right. The call / index / struct-literal
            // arms above were already fixed to use `name_location`;
            // this one was missed, which is what made an argument type
            // mismatch on `f(a)` anchor at the `)`.
            Ok(parser.ast_builder.identifier_expr(name, Some(name_location)))
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
        Some(&Kind::Float32(num)) => {
            let location = parser.current_source_location();
            parser.ast_builder.float32_expr(num, Some(location))
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

/// Parse a keyword-introduced form, anchoring the result at the
/// keyword.
///
/// These builders take their location at the end of parsing, by which
/// point the cursor has left the construct entirely — an `if`
/// expression came out located at the *first token of the next
/// statement*, so a diagnostic about it underlined innocent code on
/// another line. That is the failure P2 set out to remove, and it is
/// worse than no location at all.
///
/// The keyword alone, rather than the whole construct: `if` and `match`
/// span several lines, and the caret is clamped to the line it
/// annotates, so a full extent would just underline the rest of the
/// first line.
fn keyword_form(
    parser: &mut Parser,
    parse: fn(&mut Parser) -> ParserResult<ExprRef>,
) -> ParserResult<ExprRef> {
    let keyword = parser.current_source_location();
    parser.next();
    let expr = parse(parser)?;
    parser.ast_builder.get_location_pool_mut().set_expr_location(&expr, keyword);
    Ok(expr)
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
            let open = parser.current_source_location();
            parser.next();
            let elements = parse_array_elements(parser, vec![])?;
            parser.expect_err(&Kind::BracketClose)?;
            let location = parser.span_to_cursor(open);
            Ok(parser.ast_builder.array_literal_expr(elements, Some(location)))
        }
        Some(Kind::If) => keyword_form(parser, parse_if),
        Some(Kind::With) => keyword_form(parser, parse_with),
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
        Some(Kind::Match) => keyword_form(parser, parse_match),
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

/// Parse struct literal fields, plus the optional `..base` tail
/// (STRUCT-UPDATE). The second element of the result is `Some(base)`
/// exactly when the literal ended with `..expr`.
pub(crate) fn parse_struct_literal_fields(parser: &mut Parser, fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<(Vec<(DefaultSymbol, ExprRef)>, Option<ExprRef>)> {
    
    parse_struct_literal_fields_impl(parser, fields)
}

fn parse_struct_literal_fields_impl(parser: &mut Parser, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<(Vec<(DefaultSymbol, ExprRef)>, Option<ExprRef>)> {
    loop {
        parser.skip_newlines();
        match parser.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => return Ok((fields, None)),
            // `..base` — the struct update tail. It must come last, so
            // the only thing accepted after it is the closing brace.
            Some(Kind::DotDot) => {
                parser.next();
                let base = parser.parse_expr_impl()?;
                parser.skip_newlines();
                if let Some(Kind::Comma) = parser.peek() {
                    parser.collect_error("`..base` must be the last item in a struct literal");
                }
                return Ok((fields, Some(base)));
            }
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
                return Ok((fields, None));
            }
        };
        parser.expect_err(&Kind::Colon)?;
        let field_value = parser.parse_expr_impl()?;
        fields.push((field_name, field_value));
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
            }
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => return Ok((fields, None)),
            _ => {
                parser.collect_error("expected ',' or '}' in struct literal");
                return Ok((fields, None));
            }
        }
    }
}
