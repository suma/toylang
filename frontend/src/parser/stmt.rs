use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::Kind;
use super::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use string_interner::DefaultSymbol;

/// Skip tokens until matching '>' is found (for generic argument parsing)
fn skip_until_matching_gt(parser: &mut Parser) {
    let mut depth = 1;
    parser.next(); // Skip the initial '<'
    
    while let Some(token) = parser.peek() {
        match token {
            Kind::LT => depth += 1,
            Kind::GT => {
                depth -= 1;
                if depth == 0 {
                    parser.next(); // Consume the matching '>'
                    break;
                }
            }
            Kind::EOF => break,
            _ => {}
        }
        parser.next();
    }
}

impl<'a> Parser<'a> {
    pub fn parse_stmt(&mut self) -> ParserResult<StmtRef> {
        parse_stmt(self)
    }
}

pub fn parse_stmt(parser: &mut Parser) -> ParserResult<StmtRef> {
    match parser.peek() {
        Some(Kind::Val) | Some(Kind::Var) => {
            parse_var_def(parser)
        }
        Some(Kind::Break) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.break_stmt(Some(location)))
        }
        Some(Kind::Continue) => {
            let location = parser.current_source_location();
            parser.next();
            Ok(parser.ast_builder.continue_stmt(Some(location)))
        }
        Some(Kind::Return) => {
            parser.next();
            match parser.peek() {
                Some(&Kind::NewLine) | Some(&Kind::BracketClose) | Some(Kind::EOF) => {
                    let location = parser.current_source_location();
                    parser.next();
                    Ok(parser.ast_builder.return_stmt(None, Some(location)))
                }
                None => {
                    let location = parser.current_source_location();
                    Ok(parser.ast_builder.return_stmt(None, Some(location)))
                },
                Some(_expr) => {
                    let location = parser.current_source_location();
                    let expr = parser.parse_expr_impl()?;
                    Ok(parser.ast_builder.return_stmt(Some(expr), Some(location)))
                }
            }
        }
        Some(Kind::For) => {
            parser.next();
            let current_token = parser.peek().cloned();
            match current_token {
                Some(Kind::Identifier(s)) => {
                    let ident = parser.string_interner.get_or_intern(s);
                    parser.next();
                    parser.expect_err(&Kind::In)?;
                    // Forbid struct literals in the iterable expression so
                    // `for x in MyIter { ... }` parses the `{` as the body
                    // block, not a `MyIter {}` struct literal — same trick
                    // as `if` / `while` conditions.
                    parser.push_context(crate::parser::core::ParseContext::Condition);
                    let start = super::expr::parse_logical_expr(parser)?;
                    parser.pop_context();
                    // Three-way fork on the next token:
                    //   `to` / `..`  → integer range fast path (Stmt::For)
                    //   `{`          → iterator-protocol form (desugar)
                    //   else         → error
                    match parser.peek() {
                        Some(Kind::To) | Some(Kind::DotDot) => {
                            parser.next();
                            let end = super::expr::parse_logical_expr(parser)?;
                            let block = super::expr::parse_block(parser)?;
                            let location = parser.current_source_location();
                            Ok(parser.ast_builder.for_stmt(ident, start, end, block, Some(location)))
                        }
                        Some(Kind::BraceOpen) => {
                            let body = super::expr::parse_block(parser)?;
                            let location = parser.current_source_location();
                            Ok(desugar_for_in_iterator(parser, ident, start, body, location))
                        }
                        other => {
                            let other_str = format!("{:?}", other);
                            let location = parser.current_source_location();
                            Err(ParserError::generic_error(
                                location,
                                format!("expected `to`, `..`, or `{{` in for header, got {}", other_str),
                            ))
                        }
                    }
                }
                x => {
                    let location = parser.current_source_location();
                    Err(ParserError::generic_error(location, format!("parse_stmt for: expected identifier but {:?}", x)))
                },
            }
        }
        Some(Kind::While) => {
            parser.next();
            // Push condition context to prevent struct literals in while conditions
            parser.push_context(crate::parser::core::ParseContext::Condition);
            let cond = super::expr::parse_logical_expr(parser)?;
            parser.pop_context();
            
            let block = super::expr::parse_block(parser)?;
            let location = parser.current_source_location();
            Ok(parser.ast_builder.while_stmt(cond, block, Some(location)))
        }
        _ => parser.parse_expr(),
    }
}

/// Desugar `for x in EXPR { body }` into the iterator-protocol shape:
///
/// ```text
/// {
///     var __iter_for_<n> = EXPR
///     while true {
///         match __iter_for_<n>.next() {
///             Option::Some(x) => body,
///             Option::None    => { break },
///         }
///     }
/// }
/// ```
///
/// `EXPR` must produce a value whose type exposes
/// `fn next(&mut self) -> Option<T>` — by convention, an
/// `impl Iterator<T> for ...`. Range expressions (`0..10`) keep
/// the dedicated `Stmt::For` integer fast path; only the bare
/// `for x in EXPR { body }` shape (no `..` / `to`) lands here.
fn desugar_for_in_iterator(
    parser: &mut Parser,
    loop_var: DefaultSymbol,
    iter_expr: ExprRef,
    body: ExprRef,
    location: crate::type_checker::SourceLocation,
) -> StmtRef {
    let counter = parser.synthetic_counter;
    parser.synthetic_counter += 1;
    let iter_name = format!("__iter_for_{counter}");
    let iter_sym = parser.string_interner.get_or_intern(iter_name.as_str());
    let option_sym = parser.string_interner.get_or_intern("Option");
    let some_sym = parser.string_interner.get_or_intern("Some");
    let none_sym = parser.string_interner.get_or_intern("None");
    let next_sym = parser.string_interner.get_or_intern("next");

    // var __iter_for_n = iter_expr
    let iter_decl = parser.ast_builder.var_stmt(
        iter_sym,
        None,
        Some(iter_expr),
        Some(location.clone()),
    );

    // __iter_for_n.next()
    let iter_ident = parser
        .ast_builder
        .identifier_expr(iter_sym, Some(location.clone()));
    let next_call = parser.ast_builder.method_call_expr(
        iter_ident,
        next_sym,
        vec![],
        Some(location.clone()),
    );

    // Some(x) arm — body is the user's block, wrapped in
    // `{ user_body; continue }` so the arm type unifies with
    // the None-arm's `{ break }` (both Unit). The trailing
    // `continue` is a no-op semantically (the while body has
    // nothing after the match), but its `Unit` return type
    // discards whatever the user's last statement produced
    // — without it, `for x in iter { sum = sum + x }` fails
    // type-check because Assign returns its rhs type.
    let user_body_stmt = parser
        .ast_builder
        .add_stmt_with_location(Stmt::Expression(body), Some(location.clone()));
    let continue_stmt = parser.ast_builder.continue_stmt(Some(location.clone()));
    let some_arm_body = parser.ast_builder.block_expr(
        vec![user_body_stmt, continue_stmt],
        Some(location.clone()),
    );
    let some_arm = MatchArm {
        pattern: Pattern::EnumVariant(option_sym, some_sym, vec![Pattern::Name(loop_var)]),
        guard: None,
        body: some_arm_body,
    };

    // None => { break }
    let break_stmt = parser.ast_builder.break_stmt(Some(location.clone()));
    let break_block = parser
        .ast_builder
        .block_expr(vec![break_stmt], Some(location.clone()));
    let none_arm = MatchArm {
        pattern: Pattern::EnumVariant(option_sym, none_sym, vec![]),
        guard: None,
        body: break_block,
    };

    // match __iter.next() { Some(x) => body, None => { break } }
    let match_expr = parser.ast_builder.add_expr_with_location(
        Expr::Match(next_call, vec![some_arm, none_arm]),
        Some(location.clone()),
    );

    // while true { <match-stmt> }
    let true_expr = parser.ast_builder.bool_true_expr(Some(location.clone()));
    let match_stmt = parser.ast_builder.add_stmt_with_location(
        Stmt::Expression(match_expr),
        Some(location.clone()),
    );
    let while_body = parser
        .ast_builder
        .block_expr(vec![match_stmt], Some(location.clone()));
    let while_stmt = parser
        .ast_builder
        .while_stmt(true_expr, while_body, Some(location.clone()));

    // Outer block: { var __iter = ...; while ... { ... } }
    let outer_block = parser
        .ast_builder
        .block_expr(vec![iter_decl, while_stmt], Some(location.clone()));
    parser
        .ast_builder
        .add_stmt_with_location(Stmt::Expression(outer_block), Some(location))
}

pub fn parse_var_def(parser: &mut Parser) -> ParserResult<StmtRef> {
    let is_val = match parser.peek() {
        Some(Kind::Val) => true,
        Some(Kind::Var) => false,
        _ => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, "parse_var_def: expected val or var".to_string()))
        },
    };
    parser.next();

    // Tuple destructuring: `val (a, b, ...) = expr` desugars into a
    // hidden temporary that holds the rhs plus per-name bindings to
    // `tmp.0`, `tmp.1`, … . `var (a, b) = …` works the same way; the
    // resulting bindings inherit the val/var flavor of the outer form.
    if matches!(parser.peek(), Some(Kind::ParenOpen)) {
        return parse_tuple_destructuring(parser, is_val);
    }

    let current_token = parser.peek().cloned();
    let ident: DefaultSymbol = match current_token {
        Some(Kind::Identifier(s)) => {
            let sym = parser.string_interner.get_or_intern(s);
            parser.next();
            sym
        }
        Some(ref kind) if kind.is_keyword() => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: reserved keyword '{}' cannot be used as identifier", 
                match kind {
                    Kind::If => "if",
                    Kind::Else => "else", 
                    Kind::While => "while",
                    Kind::For => "for",
                    Kind::Function => "fn",
                    Kind::Return => "return",
                    Kind::Break => "break", 
                    Kind::Continue => "continue",
                    Kind::Val => "val",
                    Kind::Var => "var",
                    Kind::Struct => "struct",
                    Kind::Impl => "impl",
                    _ => "keyword"
                }
            )))
        }
        x => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: expected identifier but {:?}", x)))
        },
    };

    let ty: TypeDecl = match parser.peek() {
        Some(Kind::Colon) => {
            parser.next();
            parser.parse_type_declaration()?
        }
        _ => TypeDecl::Unknown,
    };

    let rhs = match parser.peek() {
        Some(Kind::Equal) => {
            parser.next();
            let expr = super::expr::parse_range_expr(parser);
            if expr.is_err() {
                return Err(expr.err().unwrap());
            }
            Some(expr?)
        }
        Some(Kind::NewLine) => None,
        _ => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_var_def: expected expression but {:?}", parser.peek())))
        },
    };
    let location = parser.current_source_location();
    if is_val {
        Ok(parser.ast_builder.val_stmt(ident, Some(ty), rhs.unwrap(), Some(location)))
    } else {
        Ok(parser.ast_builder.var_stmt(ident, Some(ty), rhs, Some(location)))
    }
}

/// Internal tree shape used while desugaring `val (...) = expr`.
/// A leaf binds the matched element to `name`; an inner node further
/// destructures through another tuple pattern.
enum DestructPat {
    Name(DefaultSymbol),
    Tuple(Vec<DestructPat>),
}

/// Parse a `(p, q, ...)` tuple sub-pattern, recursively. The leading
/// `(` must already be visible at `parser.peek()`.
fn parse_destruct_tuple(parser: &mut Parser) -> ParserResult<DestructPat> {
    parser.expect_err(&Kind::ParenOpen)?;
    let mut subs: Vec<DestructPat> = Vec::new();
    loop {
        let sub = match parser.peek().cloned() {
            Some(Kind::ParenOpen) => parse_destruct_tuple(parser)?,
            Some(Kind::Identifier(s)) => {
                parser.next();
                DestructPat::Name(parser.string_interner.get_or_intern(s.as_str()))
            }
            other => {
                let loc = parser.current_source_location();
                return Err(ParserError::generic_error(
                    loc,
                    format!("expected identifier or `(` in tuple pattern, got {:?}", other),
                ));
            }
        };
        subs.push(sub);
        match parser.peek().cloned() {
            Some(Kind::Comma) => {
                parser.next();
            }
            Some(Kind::ParenClose) => break,
            other => {
                let loc = parser.current_source_location();
                return Err(ParserError::generic_error(
                    loc,
                    format!("expected `,` or `)` in tuple pattern, got {:?}", other),
                ));
            }
        }
    }
    parser.expect_err(&Kind::ParenClose)?;
    if subs.len() < 2 {
        let loc = parser.current_source_location();
        return Err(ParserError::generic_error(
            loc,
            "tuple destructuring requires at least two sub-patterns".to_string(),
        ));
    }
    Ok(DestructPat::Tuple(subs))
}

/// Walk the parsed pattern and emit the desugared `val` / `var`
/// statements in source-order into `out`. `rhs_expr` is the expression
/// to bind at this level: at the top call it is the user-written rhs;
/// at recursive calls it is `__tuple_tmp_N.i`. Synthesized tmp
/// bindings are always `val` (they are internal).
fn emit_destructure(
    parser: &mut Parser,
    pat: &DestructPat,
    rhs_expr: ExprRef,
    is_val: bool,
    location: &crate::type_checker::SourceLocation,
    out: &mut Vec<StmtRef>,
) {
    match pat {
        DestructPat::Name(sym) => {
            let stmt = if is_val {
                parser.ast_builder.val_stmt(*sym, Some(TypeDecl::Unknown), rhs_expr, Some(location.clone()))
            } else {
                parser.ast_builder.var_stmt(*sym, Some(TypeDecl::Unknown), Some(rhs_expr), Some(location.clone()))
            };
            out.push(stmt);
        }
        DestructPat::Tuple(subs) => {
            let counter = parser.synthetic_counter;
            parser.synthetic_counter += 1;
            let tmp_name = format!("__tuple_tmp_{counter}");
            let tmp_sym = parser.string_interner.get_or_intern(tmp_name.as_str());
            let tmp_stmt = parser
                .ast_builder
                .val_stmt(tmp_sym, Some(TypeDecl::Unknown), rhs_expr, Some(location.clone()));
            out.push(tmp_stmt);
            for (i, sub) in subs.iter().enumerate() {
                let tmp_id = parser.ast_builder.identifier_expr(tmp_sym, Some(location.clone()));
                let access = parser
                    .ast_builder
                    .tuple_access_expr(tmp_id, i, Some(location.clone()));
                emit_destructure(parser, sub, access, is_val, location, out);
            }
        }
    }
}

/// Lower `val (a, b, ...) = expr` (or `var (...) = ...`) into a series
/// of plain `Val` / `Var` statements. Sub-patterns may themselves be
/// tuple patterns, so `val ((a, b), c) = expr` decomposes through an
/// extra synthetic temporary. The final per-name statement is returned
/// to `parse_block_impl` while the others land in
/// `pending_prelude_stmts` so they appear ahead of the primary one in
/// the resulting block.
fn parse_tuple_destructuring(parser: &mut Parser, is_val: bool) -> ParserResult<StmtRef> {
    let location = parser.current_source_location();
    let pat = parse_destruct_tuple(parser)?;

    // Optional whole-tuple type annotation, e.g. `val (a, b): (i64, i64) = ...`.
    // We currently parse-and-discard it; the rhs's element types
    // determine each binding's inferred type.
    if matches!(parser.peek(), Some(Kind::Colon)) {
        parser.next();
        let _ = parser.parse_type_declaration()?;
    }

    parser.expect_err(&Kind::Equal)?;
    let rhs = super::expr::parse_range_expr(parser)?;

    let mut stmts: Vec<StmtRef> = Vec::new();
    emit_destructure(parser, &pat, rhs, is_val, &location, &mut stmts);

    // The last emitted statement (the rightmost leaf binding) is the
    // primary; everything else is prelude.
    let primary = stmts
        .pop()
        .expect("destructure emitted at least one statement");
    for stmt in stmts {
        parser.pending_prelude_stmts.push(stmt);
    }
    Ok(primary)
}

pub fn parse_struct_fields(parser: &mut Parser, fields: Vec<StructField>) -> ParserResult<Vec<StructField>> {
    parse_struct_fields_with_generic_context(parser, fields, &[])
}

pub fn parse_struct_fields_with_generic_context(parser: &mut Parser, mut fields: Vec<StructField>, generic_params: &[string_interner::DefaultSymbol]) -> ParserResult<Vec<StructField>> {
    // Limit maximum number of fields to prevent infinite loops
    const MAX_FIELDS: usize = 1000;
    
    loop {
        parser.skip_newlines();
        
        // Check for end of fields or too many fields
        if parser.peek() == Some(&Kind::BraceClose) || fields.len() >= MAX_FIELDS {
            if fields.len() >= MAX_FIELDS {
                parser.collect_error(&format!("too many struct fields (max: {})", MAX_FIELDS));
            }
            return Ok(fields);
        }

        let visibility = match parser.peek() {
            Some(Kind::Public) => {
                parser.next();
                Visibility::Public
            }
            _ => Visibility::Private,
        };

        let field_name = match parser.peek() {
            Some(Kind::Identifier(s)) => {
                let name = s.to_string();
                parser.next();
                name
            }
            _ => {
                let location = parser.current_source_location();
                return Err(ParserError::generic_error(location, "expected field name".to_string()))
            },
        };

        parser.expect_err(&Kind::Colon)?;
        
        // Use generic context-aware type parsing
        let generic_context: std::collections::HashSet<string_interner::DefaultSymbol> = generic_params.iter().cloned().collect();
        let field_type = match parser.parse_type_declaration_with_generic_context(&generic_context) {
            Ok(ty) => ty,
            Err(e) => {
                parser.collect_error(&format!("expected type after ':' in struct field: {}", e));
                return Ok(fields);
            }
        };

        fields.push(StructField {
            name: field_name,
            type_decl: field_type,
            visibility,
        });

        parser.skip_newlines();
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                parser.skip_newlines();
                // Continue loop to parse next field
                if parser.peek() == Some(&Kind::BraceClose) {
                    return Ok(fields);
                }
            }
            Some(Kind::BraceClose) => {
                return Ok(fields);
            }
            _ => {
                let current_token = parser.peek().cloned();
                parser.collect_error(&format!("expected ',' or '}}' after struct field, found {:?}", current_token));
                return Ok(fields);
            }
        }
    }
}

pub fn parse_impl_methods(parser: &mut Parser, methods: Vec<Rc<MethodFunction>>) -> ParserResult<Vec<Rc<MethodFunction>>> {
    parse_impl_methods_with_generic_context(parser, methods, &[], &std::collections::HashMap::new())
}

/// Parse the body of a `trait` declaration: a sequence of method
/// signatures (no body block). Each signature is `fn name(params) -> RetTy`
/// optionally followed by `requires` / `ensures` clauses. Methods are
/// terminated by a newline; the loop ends at `}`. Generics on individual
/// trait methods are accepted but their bounds are dropped (the initial
/// trait feature implementation does not propagate them).
pub fn parse_trait_method_signatures(
    parser: &mut Parser,
) -> ParserResult<Vec<TraitMethodSignature>> {
    const MAX_METHODS: usize = 500;
    let mut methods: Vec<TraitMethodSignature> = Vec::new();

    loop {
        parser.skip_newlines();

        if parser.peek() == Some(&Kind::BraceClose) || methods.len() >= MAX_METHODS {
            if methods.len() >= MAX_METHODS {
                parser.collect_error(&format!("too many trait methods (max: {})", MAX_METHODS));
            }
            return Ok(methods);
        }

        match parser.peek() {
            Some(Kind::Function) => {
                let fn_start_pos = parser.peek_position_n(0).unwrap().start;
                parser.next();
                let method_name = match parser.peek() {
                    Some(Kind::Identifier(s)) => {
                        let s = s.to_string();
                        parser.next();
                        parser.string_interner.get_or_intern(s)
                    }
                    _ => {
                        let location = parser.current_source_location();
                        return Err(ParserError::generic_error(location, "expected method name in trait body".to_string()));
                    }
                };
                // Per-method generics are accepted but dropped for now.
                if parser.peek() == Some(&Kind::LT) {
                    skip_until_matching_gt(parser);
                }
                parser.expect_err(&Kind::ParenOpen)?;
                let (params, has_self, self_is_mut) = parse_method_param_list_with_generic_context(parser, vec![], &[])?;
                parser.expect_err(&Kind::ParenClose)?;

                let mut ret_ty: Option<TypeDecl> = None;
                if let Some(Kind::Arrow) = parser.peek() {
                    parser.expect_err(&Kind::Arrow)?;
                    ret_ty = Some(parser.parse_type_declaration()?);
                }

                let (requires, ensures) = parser.parse_contract_clauses()?;
                let fn_end_pos = parser.peek_position_n(0).unwrap_or(&std::ops::Range { start: 0, end: 0 }).end;

                methods.push(TraitMethodSignature {
                    node: Node::new(fn_start_pos, fn_end_pos),
                    name: method_name,
                    generic_params: vec![],
                    generic_bounds: std::collections::HashMap::new(),
                    parameter: params,
                    return_type: ret_ty,
                    requires,
                    ensures,
                    has_self_param: has_self,
                    self_is_mut,
                });
                parser.skip_newlines();
            }
            _ => return Ok(methods),
        }
    }
}

pub fn parse_impl_methods_with_generic_context(
    parser: &mut Parser,
    mut methods: Vec<Rc<MethodFunction>>,
    generic_params: &[string_interner::DefaultSymbol],
    generic_bounds: &std::collections::HashMap<string_interner::DefaultSymbol, TypeDecl>,
) -> ParserResult<Vec<Rc<MethodFunction>>> {
    // Limit maximum number of methods to prevent infinite loops
    const MAX_METHODS: usize = 500;
    
    loop {
        parser.skip_newlines();
        
        // Check for end of methods or too many methods
        if parser.peek() == Some(&Kind::BraceClose) || methods.len() >= MAX_METHODS {
            if methods.len() >= MAX_METHODS {
                parser.collect_error(&format!("too many impl methods (max: {})", MAX_METHODS));
            }
            return Ok(methods);
        }

        // Check for visibility modifier first
        let visibility = if matches!(parser.peek(), Some(Kind::Public)) {
            parser.next(); // consume 'pub'
            crate::ast::Visibility::Public
        } else {
            crate::ast::Visibility::Private
        };
        
        match parser.peek() {
            Some(Kind::Function) => {
                let fn_start_pos = parser.peek_position_n(0).unwrap().start;
                let location = parser.current_source_location();
                parser.next();
                match parser.peek() {
                    Some(Kind::Identifier(s)) => {
                        let s = s.to_string();
                        parser.next();
                        let method_name = parser.string_interner.get_or_intern(s);

                        // Parse optional method generic parameters: fn name<T, U>.
                        // Independent from the impl block's generic params; the
                        // two are merged into `combined_generic_params` below
                        // so the type-checker / monomorphisation pass sees the
                        // full set when binding bodies.
                        let (method_only_params, method_only_bounds) = if parser.peek()
                            == Some(&Kind::LT)
                        {
                            parser.parse_generic_params()?
                        } else {
                            (Vec::new(), std::collections::HashMap::new())
                        };

                        // Parameter / return-type parsing must see both the
                        // impl-level params AND the method-only params as
                        // generics, otherwise method-only `U` parses as a
                        // bare Identifier and downstream type-checking can't
                        // tell it from a struct name.
                        let mut combined_generic_params: Vec<string_interner::DefaultSymbol> =
                            generic_params.to_vec();
                        combined_generic_params.extend(method_only_params.iter().copied());

                        parser.expect_err(&Kind::ParenOpen)?;
                        let (params, has_self, self_is_mut) = parse_method_param_list_with_generic_context(
                            parser,
                            vec![],
                            &combined_generic_params,
                        )?;
                        parser.expect_err(&Kind::ParenClose)?;

                        let mut ret_ty: Option<TypeDecl> = None;
                        match parser.peek() {
                            Some(Kind::Arrow) => {
                                parser.expect_err(&Kind::Arrow)?;
                                let generic_context: std::collections::HashSet<string_interner::DefaultSymbol> =
                                    combined_generic_params.iter().cloned().collect();
                                ret_ty = Some(parser.parse_type_declaration_with_generic_context(&generic_context)?);
                            }
                            _ => (),
                        }

                        let (requires, ensures) = parser.parse_contract_clauses()?;
                        let block = super::expr::parse_block(parser)?;
                        let fn_end_pos = parser.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;

                        // Method-level bounds layer on top of the impl-level
                        // bounds; method bounds win on conflict.
                        let mut merged_bounds = generic_bounds.clone();
                        for (k, v) in &method_only_bounds {
                            merged_bounds.insert(*k, v.clone());
                        }

                        methods.push(Rc::new(MethodFunction {
                            node: Node::new(fn_start_pos, fn_end_pos),
                            name: method_name,
                            generic_params: combined_generic_params,
                            generic_bounds: merged_bounds,
                            parameter: params,
                            return_type: ret_ty,
                            requires,
                            ensures,
                            code: parser.ast_builder.expression_stmt(block, Some(location)),
                            has_self_param: has_self,
                            self_is_mut,
                            visibility,
                        }));
                        
                        parser.skip_newlines();
                        // Continue loop to parse next method
                    }
                    _ => {
                        let location = parser.current_source_location();
                        return Err(ParserError::generic_error(location, "expected method name after fn".to_string()));
                    }
                }
            }
            _ => {
                // Not a function, we're done
                return Ok(methods);
            }
        }
    }
}

pub fn parse_method_param_list(parser: &mut Parser, args: Vec<Parameter>) -> ParserResult<(Vec<Parameter>, bool, bool)> {
    parse_method_param_list_with_generic_context(parser, args, &[])
}

/// Parse a method parameter list. Returns
/// `(parameters, has_self, self_is_mut)` where `self_is_mut` is
/// only meaningful when `has_self == true`.
///
/// Receiver forms accepted (Stage 1 of `&` references):
///   - `self`           → `(has_self=true,  self_is_mut=false)`
///   - `&self`          → `(has_self=true,  self_is_mut=false)`  (today's behavior; `&` informational only)
///   - `&mut self`      → `(has_self=true,  self_is_mut=true)`   (NEW — drives the AOT Self-out-parameter writeback)
pub fn parse_method_param_list_with_generic_context(parser: &mut Parser, args: Vec<Parameter>, generic_params: &[string_interner::DefaultSymbol]) -> ParserResult<(Vec<Parameter>, bool, bool)> {
    let mut has_self = false;
    let mut self_is_mut = false;

    match parser.peek() {
        Some(Kind::ParenClose) => return Ok((args, has_self, self_is_mut)),
        _ => (),
    }

    if let Some(Kind::And) = parser.peek() {
        // `& [mut] self [, ...]`: peek for an optional `mut` between
        // the `&` and the `self` identifier. The `mut` token was
        // reserved in `frontend/src/lexer.l` specifically for this
        // form; rejecting `& mut foo` (where `foo != self`) keeps
        // the surface unambiguous.
        let (mut_offset, self_offset) = if matches!(parser.peek_n(1), Some(Kind::Mut)) {
            (Some(1usize), 2usize)
        } else {
            (None, 1usize)
        };
        if let Some(Kind::Identifier(name)) = parser.peek_n(self_offset) {
            if name == "self" {
                parser.next(); // consume `&`
                if mut_offset.is_some() {
                    parser.next(); // consume `mut`
                    self_is_mut = true;
                }
                parser.next(); // consume `self`
                has_self = true;

                match parser.peek() {
                    Some(Kind::Comma) => {
                        parser.next();
                        let (rest_params, _) = parse_param_def_list_impl_with_generic_context(parser, args, generic_params)?;
                        return Ok((rest_params, has_self, self_is_mut));
                    }
                    Some(Kind::ParenClose) => return Ok((args, has_self, self_is_mut)),
                    _ => {
                        let location = parser.current_source_location();
                        let receiver = if self_is_mut { "&mut self" } else { "&self" };
                        return Err(ParserError::generic_error(location, format!("expected comma or closing paren after {receiver}")))
                    },
                }
            }
        }
    }

    let (params, _) = parse_param_def_list_impl_with_generic_context(parser, args, generic_params)?;
    Ok((params, has_self, self_is_mut))
}

pub fn parse_param_def_list_impl(parser: &mut Parser, args: Vec<Parameter>) -> ParserResult<(Vec<Parameter>, bool)> {
    parse_param_def_list_impl_with_generic_context(parser, args, &[])
}

pub fn parse_param_def_list_impl_with_generic_context(parser: &mut Parser, mut args: Vec<Parameter>, generic_params: &[string_interner::DefaultSymbol]) -> ParserResult<(Vec<Parameter>, bool)> {
    // Limit maximum number of parameters to prevent infinite loops
    const MAX_PARAMS: usize = 255;
    
    loop {
        if parser.peek() == Some(&Kind::ParenClose) || args.len() >= MAX_PARAMS {
            if args.len() >= MAX_PARAMS {
                parser.collect_error(&format!("too many parameters (max: {})", MAX_PARAMS));
            }
            return Ok((args, false));
        }

        let def = parser.parse_param_def_with_generic_context(generic_params);
        if def.is_err() {
            return Ok((args, false));
        }
        args.push(def?);

        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                // Continue loop to parse next parameter
            }
            _ => {
                return Ok((args, false));
            }
        }
    }
}