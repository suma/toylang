use crate::ast::*;
use crate::token::Kind;
use crate::parser::core::Parser;
use crate::parser::error::ParserResult;
use string_interner::DefaultSymbol;

/// Recognise and rewrite parser-level macros. Returns
/// `Ok(Some(rewritten))` when `name` is one of the recognised macro
/// names and the parser successfully consumed `(` … `)`. Returns
/// `Ok(None)` when `name` is unrelated, leaving the cursor untouched
/// (the caller continues with normal call / builtin dispatch). Errors
/// propagate.
///
/// Each macro is rewritten **in place** into ordinary AST shapes so
/// the type checker / interpreter / JIT / AOT never see a macro node.
pub(crate) fn try_intercept_parser_macro(
    parser: &mut Parser,
    name: DefaultSymbol,
    location: crate::type_checker::SourceLocation,
) -> ParserResult<Option<ExprRef>> {
    let symbols = parser.builtin_symbols.clone();
    if name == symbols.source_line {
        parser.next(); // consume `(`
        parser.expect_err(&Kind::ParenClose)?;
        return Ok(Some(parser.ast_builder.uint64_expr(location.line as u64, Some(location))));
    }
    if name == symbols.source_column {
        parser.next();
        parser.expect_err(&Kind::ParenClose)?;
        return Ok(Some(parser.ast_builder.uint64_expr(location.column as u64, Some(location))));
    }
    if name == symbols.source_file {
        parser.next();
        parser.expect_err(&Kind::ParenClose)?;
        let path = parser.source_file.clone().unwrap_or_else(|| "<source>".to_string());
        let sym = parser.string_interner.get_or_intern(path);
        return Ok(Some(parser.ast_builder.string_expr(sym, Some(location))));
    }
    if name == symbols.dbg {
        return Ok(Some(parse_dbg_macro(parser, location)?));
    }
    if name == symbols.assert_eq {
        return Ok(Some(parse_assert_cmp_macro(parser, location, /*equal=*/ true)?));
    }
    if name == symbols.assert_ne {
        return Ok(Some(parse_assert_cmp_macro(parser, location, /*equal=*/ false)?));
    }
    Ok(None)
}

/// Desugar `__builtin_dbg(EXPR)` to a block that binds EXPR, prints it,
/// and returns the value. See `mod.rs` for full doc comment.
fn parse_dbg_macro(
    parser: &mut Parser,
    call_location: crate::type_checker::SourceLocation,
) -> ParserResult<ExprRef> {
    parser.next(); // consume `(`
    let expr_start = parser
        .current_position()
        .map(|r| r.start)
        .unwrap_or(call_location.offset as usize);
    let inner = parser.parse_expr_impl()?;
    let expr_end = parser
        .current_position()
        .map(|r| r.start)
        .unwrap_or(expr_start);
    parser.expect_err(&Kind::ParenClose)?;

    let captured_text = parser.source_substring(expr_start..expr_end).trim().to_string();
    let file_path = parser.source_file.clone().unwrap_or_else(|| "<source>".to_string());

    let prefix = format!("[{}:{}] {} = ", file_path, call_location.line, captured_text);
    let prefix_sym = parser.string_interner.get_or_intern(prefix);
    let prefix_expr = parser.ast_builder.string_expr(prefix_sym, Some(call_location));

    let n = parser.synthetic_counter;
    parser.synthetic_counter += 1;
    let tmp_name = format!("__dbg_{}", n);
    let tmp_sym = parser.string_interner.get_or_intern(tmp_name);

    let val_stmt = parser.ast_builder.val_stmt(tmp_sym, None, inner, Some(call_location));

    let tmp_ident_for_tostr = parser.ast_builder.identifier_expr(tmp_sym, Some(call_location));
    let to_string_call = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::ToString,
        vec![tmp_ident_for_tostr],
        Some(call_location),
    );

    let concat = parser.ast_builder.builtin_method_call_expr(
        prefix_expr,
        BuiltinMethod::StrConcat,
        vec![to_string_call],
        Some(call_location),
    );

    let println_call = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::Println,
        vec![concat],
        Some(call_location),
    );
    let println_stmt = parser.ast_builder.expression_stmt(println_call, Some(call_location));

    let tmp_ident_value = parser.ast_builder.identifier_expr(tmp_sym, Some(call_location));
    let value_stmt = parser.ast_builder.expression_stmt(tmp_ident_value, Some(call_location));

    Ok(parser.ast_builder.block_expr(vec![val_stmt, println_stmt, value_stmt], Some(call_location)))
}

/// Desugar `assert_eq(A, B)` (when `equal == true`) or `assert_ne(A, B)`
/// (when `equal == false`) to a block with temporary bindings and a
/// descriptive assert message.
fn parse_assert_cmp_macro(
    parser: &mut Parser,
    call_location: crate::type_checker::SourceLocation,
    equal: bool,
) -> ParserResult<ExprRef> {
    parser.next(); // consume `(`
    let lhs = parser.parse_expr_impl()?;
    parser.expect_err(&Kind::Comma)?;
    let rhs = parser.parse_expr_impl()?;
    parser.expect_err(&Kind::ParenClose)?;

    let n = parser.synthetic_counter;
    parser.synthetic_counter += 1;
    let l_name = format!("__ae_l_{}", n);
    let r_name = format!("__ae_r_{}", n);
    let l_sym = parser.string_interner.get_or_intern(l_name);
    let r_sym = parser.string_interner.get_or_intern(r_name);

    let l_val_stmt = parser.ast_builder.val_stmt(l_sym, None, lhs, Some(call_location));
    let r_val_stmt = parser.ast_builder.val_stmt(r_sym, None, rhs, Some(call_location));

    let l_for_cmp = parser.ast_builder.identifier_expr(l_sym, Some(call_location));
    let r_for_cmp = parser.ast_builder.identifier_expr(r_sym, Some(call_location));
    let cmp_op = if equal { Operator::EQ } else { Operator::NE };
    let cmp = parser.ast_builder.binary_expr(cmp_op, l_for_cmp, r_for_cmp, Some(call_location));

    let header_op = if equal { "==" } else { "!=" };
    let header = format!(
        "assertion `left {} right` failed at line {}\n  left:  ",
        header_op, call_location.line
    );
    let header_sym = parser.string_interner.get_or_intern(header);
    let header_expr = parser.ast_builder.string_expr(header_sym, Some(call_location));

    let l_for_str = parser.ast_builder.identifier_expr(l_sym, Some(call_location));
    let l_to_str = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::ToString,
        vec![l_for_str],
        Some(call_location),
    );
    let after_l = parser.ast_builder.builtin_method_call_expr(
        header_expr,
        BuiltinMethod::StrConcat,
        vec![l_to_str],
        Some(call_location),
    );

    let mid_sym = parser.string_interner.get_or_intern("\n  right: ");
    let mid_expr = parser.ast_builder.string_expr(mid_sym, Some(call_location));
    let after_mid = parser.ast_builder.builtin_method_call_expr(
        after_l,
        BuiltinMethod::StrConcat,
        vec![mid_expr],
        Some(call_location),
    );

    let r_for_str = parser.ast_builder.identifier_expr(r_sym, Some(call_location));
    let r_to_str = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::ToString,
        vec![r_for_str],
        Some(call_location),
    );
    let final_msg = parser.ast_builder.builtin_method_call_expr(
        after_mid,
        BuiltinMethod::StrConcat,
        vec![r_to_str],
        Some(call_location),
    );

    let assert_call = parser.ast_builder.builtin_call_expr(
        BuiltinFunction::Assert,
        vec![cmp, final_msg],
        Some(call_location),
    );
    let assert_stmt = parser.ast_builder.expression_stmt(assert_call, Some(call_location));

    Ok(parser.ast_builder.block_expr(
        vec![l_val_stmt, r_val_stmt, assert_stmt],
        Some(call_location),
    ))
}
