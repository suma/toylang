use crate::ast::*;
use crate::token::Kind;
use super::core::Parser;
use crate::parser::error::{ParserResult, ParserError};


mod match_;
pub use match_::*;

mod macros;
pub(crate) use macros::*;

mod control;
pub use control::*;

#[derive(Debug)]
pub struct OperatorGroup<'a> {
    pub tokens: Vec<(Kind, Operator)>,
    pub next_precedence: fn(&mut Parser<'a>) -> ParserResult<ExprRef>,
}

impl<'a> Parser<'a> {
    pub fn parse_expr(&mut self) -> ParserResult<StmtRef> {
        let e = self.parse_expr_impl();
        Ok(self.expr_to_stmt(e?))
    }

    pub fn parse_expr_impl(&mut self) -> ParserResult<ExprRef> {
        self.check_and_increment_recursion()?;
        
        let result = self.parse_expr_impl_internal();
        
        self.decrement_recursion();
        result
    }

    fn parse_expr_impl_internal(&mut self) -> ParserResult<ExprRef> {
        // Check for tokens that should not start an expression
        match self.peek() {
            Some(Kind::ParenClose) | Some(Kind::BracketClose) | Some(Kind::BraceClose) => {
                // These tokens should not start an expression
                // Don't consume them - let the parent handle them
                let token = self.peek().cloned();
                let line = self.line_count();
                let location = self.current_source_location();
                return Err(ParserError::generic_error(location, 
                    format!("unexpected token {:?} at line {}, expected expression", token, line)));
            }
            _ => {}
        }
        
        let lhs = parse_range_expr(self);
        if lhs.is_ok() {
            return match self.peek() {
                Some(Kind::Equal)
                | Some(Kind::PlusEqual)
                | Some(Kind::MinusEqual)
                | Some(Kind::StarEqual)
                | Some(Kind::SlashEqual)
                | Some(Kind::PercentEqual) => parse_assign(self, lhs?),
                _ => lhs,
            };
        }

        match self.peek() {
            Some(Kind::If) => {
                self.next();
                parse_if(self)
            }
            Some(x) => {
                let x = x.clone();
                let line = self.line_count();
                self.collect_error(&format!("expected expression but found {:?} at line {}", x, line));
                // Skip the problematic token to avoid infinite loop
                self.next();
                // Return a dummy expression to continue parsing
                Ok(self.ast_builder.null_expr(None))
            }
            None => {
                self.collect_error("unexpected EOF while parsing expression");
                // Return a dummy expression to continue parsing
                Ok(self.ast_builder.null_expr(None))
            }
        }
    }

    fn expr_to_stmt(&mut self, e: ExprRef) -> StmtRef {
        self.ast_builder.expression_stmt(e, None)
    }
}


pub fn parse_assign(parser: &mut Parser, mut lhs: ExprRef) -> ParserResult<ExprRef> {
    loop {
        // Compound-assignment desugaring: `lhs op= rhs` lowers to
        // `lhs = lhs op rhs`. The lhs is duplicated through the
        // expression pool, which is fine for `Identifier` / `FieldAccess`
        // / `TupleAccess` / `SliceAccess` shapes since their evaluation
        // is cheap and the AST nodes are reusable. We deliberately do
        // not capture rhs into a temporary, so any side effects on rhs
        // run exactly once.
        let compound = match parser.peek() {
            Some(Kind::PlusEqual) => Some(Operator::IAdd),
            Some(Kind::MinusEqual) => Some(Operator::ISub),
            Some(Kind::StarEqual) => Some(Operator::IMul),
            Some(Kind::SlashEqual) => Some(Operator::IDiv),
            Some(Kind::PercentEqual) => Some(Operator::IMod),
            _ => None,
        };
        if let Some(op) = compound {
            parser.next();
            let rhs = parse_logical_expr(parser)?;
            let location = parser.current_source_location();
            let combined = parser
                .ast_builder
                .binary_expr(op, lhs, rhs, Some(location));
            if let Some(Expr::SliceAccess(object, slice_info)) =
                parser.ast_builder.expr_pool.get(&lhs)
            {
                let start = slice_info.start;
                let end = slice_info.end;
                lhs = parser.ast_builder.slice_assign_expr(
                    object,
                    start,
                    end,
                    combined,
                    Some(location),
                );
            } else {
                lhs = parser
                    .ast_builder
                    .assign_expr(lhs, combined, Some(location));
            }
            continue;
        }
        match parser.peek() {
            Some(Kind::Equal) => {
                parser.next();
                let new_rhs = parse_logical_expr(parser)?;
                let location = parser.current_source_location();

                // Check if lhs is a SliceAccess expression and convert to SliceAssign
                if let Some(Expr::SliceAccess(object, slice_info)) = parser.ast_builder.expr_pool.get(&lhs) {
                    let object = object;
                    let start = slice_info.start;
                    let end = slice_info.end;
                    lhs = parser.ast_builder.slice_assign_expr(object, start, end, new_rhs, Some(location));
                } else {
                    lhs = parser.ast_builder.assign_expr(lhs, new_rhs, Some(location));
                }
            }
            _ => return Ok(lhs),
        }
    }
}

pub fn parse_block(parser: &mut Parser) -> ParserResult<ExprRef> {
    parser.expect_err(&Kind::BraceOpen)?;
    match parser.peek() {
        Some(Kind::BraceClose) | None => {
            parser.next();
            let location = parser.current_source_location();
            Ok(parser.ast_builder.block_expr(vec![], Some(location)))
        }
        _ => {
            let block = parse_block_impl(parser, vec![])?;
            parser.expect_err(&Kind::BraceClose)?;
            let location = parser.current_source_location();
            Ok(parser.ast_builder.block_expr(block, Some(location)))
        }
    }
}

pub fn parse_block_impl(parser: &mut Parser, mut statements: Vec<StmtRef>) -> ParserResult<Vec<StmtRef>> {
    // Add maximum iteration limit to prevent infinite loops
    const MAX_ITERATIONS: usize = 1000;
    let mut iteration_count = 0;
    
    loop {
        // Safety check for infinite loop prevention
        iteration_count += 1;
        if iteration_count > MAX_ITERATIONS {
            parser.collect_error("Maximum parse iterations reached in block - possible infinite loop");
            return Ok(statements);
        }
        
        // Skip newlines
        while parser.peek() == Some(&Kind::NewLine) {
            parser.next();
        }
        
        // Check for end of block
        match parser.peek() {
            Some(Kind::BraceClose) | Some(Kind::EOF) | None => {
                return Ok(statements);
            }
            _ => {}
        }
        
        // Store current state before parsing
        let token_before = parser.peek().cloned();
        
        // Parse statement
        let lhs = super::stmt::parse_stmt(parser);

        match lhs {
            Ok(stmt) => {
                // Drain any synthetic prelude statements the stmt
                // parser appended (used by tuple destructuring) so
                // they land in source order before the main stmt.
                if !parser.pending_prelude_stmts.is_empty() {
                    let prelude = std::mem::take(&mut parser.pending_prelude_stmts);
                    statements.extend(prelude);
                }
                statements.push(stmt);
            }
            Err(err) => {
                let error_token = parser.peek().cloned();
                parser.collect_error(&format!("expected statement in block: {:?} at token {:?}", err, error_token));
                
                // Critical: Always ensure we make progress to avoid infinite loop
                match parser.peek() {
                    Some(Kind::BraceClose) | Some(Kind::EOF) | None => {
                        return Ok(statements);
                    }
                    _ => {
                        // ALWAYS consume a token on error to guarantee progress
                        if parser.peek() == token_before.as_ref() {
                            parser.next(); // Skip the problematic token
                        } else {
                            // If token changed but we still have an error, skip current token anyway
                            parser.next();
                        }
                    }
                }
            }
        }
    }
}

/// Parse a range literal `start..end`. Range binds weaker than any arithmetic
/// or logical operator, so `a + 1 .. b + 1` groups as `(a + 1) .. (b + 1)`.
/// Ranges do not chain: `a..b..c` is a parse error.
pub fn parse_range_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let start = parse_logical_expr(parser)?;
    if parser.peek() == Some(&Kind::DotDot) {
        let location = parser.current_source_location();
        parser.next();
        let end = parse_logical_expr(parser)?;
        if parser.peek() == Some(&Kind::DotDot) {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(
                location,
                "range operator `..` is not associative; parenthesize to combine ranges".to_string(),
            ));
        }
        Ok(parser.ast_builder.add_expr_with_location(Expr::Range(start, end), Some(location)))
    } else {
        Ok(start)
    }
}

pub fn parse_logical_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::DoubleAnd, Operator::LogicalAnd),
            (Kind::DoubleOr, Operator::LogicalOr),
        ],
        next_precedence: parse_bitwise_or
    };
    parse_binary(parser, &group)
}

pub fn parse_bitwise_or(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::Or, Operator::BitwiseOr),
        ],
        next_precedence: parse_bitwise_xor
    };
    parse_binary(parser, &group)
}

pub fn parse_bitwise_xor(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::Xor, Operator::BitwiseXor),
        ],
        next_precedence: parse_bitwise_and
    };
    parse_binary(parser, &group)
}

pub fn parse_bitwise_and(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::And, Operator::BitwiseAnd),
        ],
        next_precedence: parse_equality
    };
    parse_binary(parser, &group)
}

pub fn parse_equality(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::DoubleEqual, Operator::EQ),
            (Kind::NotEqual, Operator::NE),
        ],
        next_precedence: parse_relational
    };
    parse_binary(parser, &group)
}

pub fn parse_relational(parser: &mut Parser) -> ParserResult<ExprRef> {
    let lhs = parse_shift(parser)?;
    let op1 = match parser.peek() {
        Some(Kind::LT) => Operator::LT,
        Some(Kind::LE) => Operator::LE,
        Some(Kind::GT) => Operator::GT,
        Some(Kind::GE) => Operator::GE,
        _ => return Ok(lhs),
    };

    let location = parser.current_source_location();
    parser.next();
    let rhs1 = parse_shift(parser)?;

    // Single comparison: no chain → plain binary expr.
    if !matches!(parser.peek(), Some(Kind::LT) | Some(Kind::LE) | Some(Kind::GT) | Some(Kind::GE)) {
        return Ok(parser.ast_builder.binary_expr(op1, lhs, rhs1, Some(location)));
    }

    // Comparison chain `a < b < c < d` desugars to:
    //   {
    //     val __cmp_0 = b
    //     val __cmp_1 = c
    //     a < __cmp_0 && __cmp_0 < __cmp_1 && __cmp_1 < d
    //   }
    // Each intermediate operand is stored in a synthetic temporary so it is
    // evaluated exactly once and side-effects run in left-to-right order.
    let mut stmts: Vec<StmtRef> = Vec::new();
    let mut comparisons: Vec<(ExprRef, Operator, ExprRef)> = Vec::new();

    let counter = parser.synthetic_counter;
    parser.synthetic_counter += 1;
    let tmp_name = format!("__cmp_{counter}");
    let tmp_sym = parser.string_interner.get_or_intern(tmp_name.as_str());
    let val_stmt = parser
        .ast_builder
        .val_stmt(tmp_sym, None, rhs1, Some(location));
    stmts.push(val_stmt);

    let tmp_ident = parser
        .ast_builder
        .identifier_expr(tmp_sym, Some(location));
    comparisons.push((lhs, op1, tmp_ident));
    let mut last_tmp = tmp_ident;

    while matches!(parser.peek(), Some(Kind::LT) | Some(Kind::LE) | Some(Kind::GT) | Some(Kind::GE)) {
        let op = match parser.peek() {
            Some(Kind::LT) => Operator::LT,
            Some(Kind::LE) => Operator::LE,
            Some(Kind::GT) => Operator::GT,
            Some(Kind::GE) => Operator::GE,
            _ => break,
        };
        parser.next();
        let rhs = parse_shift(parser)?;

        let counter = parser.synthetic_counter;
        parser.synthetic_counter += 1;
        let tmp_name = format!("__cmp_{counter}");
        let tmp_sym = parser.string_interner.get_or_intern(tmp_name.as_str());
        let val_stmt = parser
            .ast_builder
            .val_stmt(tmp_sym, None, rhs, Some(location));
        stmts.push(val_stmt);

        let new_tmp = parser
            .ast_builder
            .identifier_expr(tmp_sym, Some(location));
        comparisons.push((last_tmp, op, new_tmp));
        last_tmp = new_tmp;
    }

    let (lhs0, op0, rhs0) = &comparisons[0];
    let mut result = parser.ast_builder.binary_expr(
        op0.clone(),
        *lhs0,
        *rhs0,
        Some(location),
    );
    for i in 1..comparisons.len() {
        let (lhs_i, op_i, rhs_i) = &comparisons[i];
        let cmp = parser.ast_builder.binary_expr(
            op_i.clone(),
            *lhs_i,
            *rhs_i,
            Some(location),
        );
        result = parser.ast_builder.binary_expr(
            Operator::LogicalAnd,
            result,
            cmp,
            Some(location),
        );
    }

    let result_stmt = parser.ast_builder.add_stmt_with_location(
        crate::ast::Stmt::Expression(result),
        Some(location),
    );
    stmts.push(result_stmt);

    Ok(parser.ast_builder.block_expr(stmts, Some(location)))
}

pub fn parse_shift(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::LeftShift, Operator::LeftShift),
            (Kind::RightShift, Operator::RightShift),
        ],
        next_precedence: parse_add
    };
    parse_binary(parser, &group)
}

pub fn parse_binary<'a>(parser: &mut Parser<'a>, group: &OperatorGroup<'a>) -> ParserResult<ExprRef> {
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_binary_impl(parser, group);
    
    parser.decrement_recursion();
    result
}

fn parse_binary_impl<'a>(parser: &mut Parser<'a>, group: &OperatorGroup<'a>) -> ParserResult<ExprRef> {
    let mut lhs = (group.next_precedence)(parser)?;

    loop {
        let next_token = parser.peek();
        let matched_op = group.tokens.iter()
            .find(|(kind, _)| next_token == Some(kind));

        match matched_op {
            Some((kind, op)) => {
                // `-` is both binary subtraction and unary negation. When it
                // appears at the start of a new source line, treat it as the
                // start of a new expression so `val x = 7\n-y` parses as two
                // statements, not `7 - y`.
                if matches!(kind, Kind::ISub) && parser.has_newline_before_current_token() {
                    return Ok(lhs);
                }
                let location = parser.current_source_location();
                parser.next();
                let rhs = (group.next_precedence)(parser)?;
                lhs = parser.ast_builder.binary_expr(op.clone(), lhs, rhs, Some(location));
            }
            None => return Ok(lhs),
        }
    }
}

pub fn parse_add(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::IAdd, Operator::IAdd),
            (Kind::ISub, Operator::ISub),
        ],
        next_precedence: parse_mul
    };
    parse_binary(parser, &group)
}

pub fn parse_mul(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::IMul, Operator::IMul),
            (Kind::IDiv, Operator::IDiv),
            (Kind::IMod, Operator::IMod),
        ],
        next_precedence: parse_unary,
    };
    parse_binary(parser, &group)
}

pub fn parse_unary(parser: &mut Parser) -> ParserResult<ExprRef> {
    match parser.peek() {
        Some(Kind::Tilde) => {
            let location = parser.current_source_location();
            parser.next();
            let operand = parse_unary(parser)?;
            Ok(parser.ast_builder.unary_expr(UnaryOp::BitwiseNot, operand, Some(location)))
        }
        Some(Kind::Exclamation) => {
            let location = parser.current_source_location();
            parser.next();
            let operand = parse_unary(parser)?;
            Ok(parser.ast_builder.unary_expr(UnaryOp::LogicalNot, operand, Some(location)))
        }
        // `-` at expression start is unary negation. Binary subtraction uses the
        // same token but appears after an operand, which is handled by
        // parse_add / parse_binary — those call parse_unary only when a new
        // primary is expected, so there is no ambiguity here.
        Some(Kind::ISub) => {
            let location = parser.current_source_location();
            parser.next();
            let operand = parse_unary(parser)?;
            Ok(parser.ast_builder.unary_expr(UnaryOp::Negate, operand, Some(location)))
        }
        // REF-Stage-2: prefix `&` / `&mut` at expression start is an
        // explicit borrow. Binary `&` (bitwise AND) appears only after
        // an operand and is reached via parse_bitwise_and -> ... ->
        // parse_unary, so there is no ambiguity here.
        Some(Kind::And) => {
            let location = parser.current_source_location();
            parser.next();
            let op = if parser.peek() == Some(&Kind::Mut) {
                parser.next();
                UnaryOp::BorrowMut
            } else {
                UnaryOp::Borrow
            };
            let operand = parse_unary(parser)?;
            Ok(parser.ast_builder.unary_expr(op, operand, Some(location)))
        }
        _ => parse_postfix(parser)
    }
}

pub fn parse_postfix(parser: &mut Parser) -> ParserResult<ExprRef> {
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_postfix_impl(parser);
    
    parser.decrement_recursion();
    result
}

fn parse_postfix_impl(parser: &mut Parser) -> ParserResult<ExprRef> {
    let mut expr = parse_primary(parser)?;
    
    loop {
        match parser.peek() {
            Some(Kind::Dot) => {
                parser.next();
                match parser.peek() {
                    Some(Kind::Identifier(field_name)) => {
                        let field_name = field_name.to_string();
                        let field_symbol = parser.string_interner.get_or_intern(field_name);
                        parser.next();
                        
                        if parser.peek() == Some(&Kind::ParenOpen) {
                            let location = parser.current_source_location();
                            parser.next();
                            let args = parse_expr_list(parser, vec![])?;
                            parser.expect_err(&Kind::ParenClose)?;
                            // Always parse as regular method call - let type checker decide if it's builtin
                            expr = parser.ast_builder.method_call_expr(expr, field_symbol, args, Some(location));
                        } else {
                            let location = parser.current_source_location();
                            expr = parser.ast_builder.field_access_expr(expr, field_symbol, Some(location));
                        }
                    }
                    Some(Kind::Integer(index_str)) => {
                        // Handle tuple access like tuple.0, tuple.1
                        let index_str = index_str.to_string();
                        if let Ok(index) = index_str.parse::<usize>() {
                            parser.next();
                            let location = parser.current_source_location();
                            expr = parser.ast_builder.tuple_access_expr(expr, index, Some(location));
                        } else {
                            parser.collect_error(&format!("invalid tuple index: {}", index_str));
                            break;
                        }
                    }
                    _ => {
                        parser.collect_error("expected field name or tuple index after '.'");
                        break; // Stop processing and return current expr
                    }
                }
            }
            Some(Kind::BracketOpen) => {
                // If there's a newline before '[' in the original source,
                // treat it as a new expression (array literal), not bracket access.
                // This disambiguates `val x = [1, 2]\n[a, b]` from `arr[0]`.
                if parser.has_newline_before_current_token() {
                    break;
                }
                // Generic index access or slice - works on any expression
                let location = parser.current_source_location();
                parser.next();

                expr = primary::parse_bracket_access(parser, expr, location)?;
            }
            Some(Kind::As) => {
                // Type cast expression: expr as type
                let location = parser.current_source_location();
                parser.next(); // consume 'as'

                let target_type = parser.parse_type_declaration()?;
                expr = parser.ast_builder.cast_expr(expr, target_type, Some(location));
            }
            _ => break,
        }
    }

    Ok(expr)
}

/// Lower a `Kind::InterpolatedString(parts)` token into the
/// equivalent `.concat()` chain at parse time. For
/// `parts = [Lit("a"), Expr("x + 1"), Lit("b")]`, the synthesized
/// token sequence is:
///
/// ```text
/// "a" . concat ( __builtin_to_string ( x + 1 ) ) . concat ( "b" )
/// ```
///
/// The expression text inside each `{...}` is re-tokenized with a
/// fresh `Lexer` instance, then the whole synthetic stream is
mod primary;
pub use primary::*;

pub fn parse_expr_list(parser: &mut Parser, args: Vec<ExprRef>) -> ParserResult<Vec<ExprRef>> {
    // Add recursion protection for expression list parsing
    parser.check_and_increment_recursion()?;
    
    let result = parse_expr_list_impl(parser, args);
    
    parser.decrement_recursion();
    result
}

fn parse_expr_list_impl(parser: &mut Parser, mut args: Vec<ExprRef>) -> ParserResult<Vec<ExprRef>> {
    // Limit maximum number of arguments to prevent infinite loops
    const MAX_ARGS: usize = 255;
    
    loop {
        if parser.peek() == Some(&Kind::ParenClose) || args.len() >= MAX_ARGS {
            if args.len() >= MAX_ARGS {
                parser.collect_error(&format!("too many arguments (max: {})", MAX_ARGS));
            }
            return Ok(args);
        }

        let expr = parser.parse_expr_impl();
        if expr.is_err() {
            return Ok(args);
        }
        args.push(expr?);

        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                // Continue loop to parse next argument
            }
            Some(Kind::ParenClose) => {
                return Ok(args);
            }
            x => {
                let x_cloned = x.cloned();
                parser.collect_error(&format!("unexpected token in expression list: {:?}", x_cloned));
                return Ok(args); // Return current args and stop
            }
        }
    }
}