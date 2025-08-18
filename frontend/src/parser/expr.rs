use crate::ast::*;
use crate::token::Kind;
use super::core::Parser;
use crate::parser::error::{ParserResult, ParserError};
use string_interner::DefaultSymbol;

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
        
        let lhs = parse_logical_expr(self);
        if lhs.is_ok() {
            return match self.peek() {
                Some(Kind::Equal) => {
                    parse_assign(self, lhs?)
                }
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


fn parse_dict_literal(parser: &mut Parser) -> ParserResult<ExprRef> {
    let location = parser.current_source_location();
    
    // Expect opening brace after 'dict' keyword
    parser.expect_err(&Kind::BraceOpen)?;
    
    parser.skip_newlines(); // Skip newlines after opening brace
    
    // Handle empty dict{}
    if parser.peek() == Some(&Kind::BraceClose) {
        parser.next();
        return Ok(parser.ast_builder.dict_literal_expr(vec![], Some(location)));
    }
    
    let entries = parse_dict_entries(parser, vec![])?;
    
    parser.skip_newlines(); // Skip newlines before closing brace
    parser.expect_err(&Kind::BraceClose)?;
    Ok(parser.ast_builder.dict_literal_expr(entries, Some(location)))
}

fn parse_dict_entries(parser: &mut Parser, mut entries: Vec<(ExprRef, ExprRef)>) -> ParserResult<Vec<(ExprRef, ExprRef)>> {
    loop {
        parser.skip_newlines(); // Skip newlines before key
        
        // Parse key
        let key = parser.parse_expr_impl()?;
        
        parser.skip_newlines(); // Skip newlines before colon
        
        // Expect colon
        parser.expect_err(&Kind::Colon)?;
        
        parser.skip_newlines(); // Skip newlines after colon
        
        // Parse value
        let value = parser.parse_expr_impl()?;
        
        entries.push((key, value));
        
        parser.skip_newlines(); // Skip newlines after value
        
        match parser.peek() {
            Some(Kind::Comma) => {
                parser.next();
                parser.skip_newlines(); // Skip newlines after comma
                // Allow trailing comma
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

pub fn parse_assign(parser: &mut Parser, mut lhs: ExprRef) -> ParserResult<ExprRef> {
    loop {
        match parser.peek() {
            Some(Kind::Equal) => {
                parser.next();
                let new_rhs = parse_logical_expr(parser)?;
                let location = parser.current_source_location();
                
                // Check if lhs is an IndexAccess expression and convert to IndexAssign
                if let Some(Expr::IndexAccess(object, index)) = parser.ast_builder.expr_pool.get(lhs.to_index()) {
                    let object = *object;
                    let index = *index;
                    lhs = parser.ast_builder.index_assign_expr(object, index, new_rhs, Some(location));
                } else {
                    lhs = parser.ast_builder.assign_expr(lhs, new_rhs, Some(location));
                }
            }
            _ => return Ok(lhs),
        }
    }
}

pub fn parse_if(parser: &mut Parser) -> ParserResult<ExprRef> {
    let cond = parse_logical_expr(parser)?;
    let if_block = parse_block(parser)?;

    let mut elif_pairs = Vec::new();
    while let Some(Kind::Elif) = parser.peek() {
        parser.next();
        let elif_cond = parse_logical_expr(parser)?;
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

pub fn parse_logical_expr(parser: &mut Parser) -> ParserResult<ExprRef> {
    let group = OperatorGroup {
        tokens: vec![
            (Kind::DoubleAnd, Operator::LogicalAnd),
            (Kind::DoubleOr, Operator::LogicalOr),
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
    let group = OperatorGroup {
        tokens: vec![
            (Kind::LT, Operator::LT),
            (Kind::LE, Operator::LE),
            (Kind::GT, Operator::GT),
            (Kind::GE, Operator::GE),
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
            Some((_, op)) => {
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
        ],
        next_precedence: parse_postfix,
    };
    parse_binary(parser, &group)
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
                    _ => {
                        parser.collect_error("expected field name after '.'");
                        break; // Stop processing and return current expr
                    }
                }
            }
            Some(Kind::BracketOpen) => {
                // Generic index access - works on any expression
                let location = parser.current_source_location();
                parser.next();
                let index = parser.parse_expr_impl()?;
                parser.expect_err(&Kind::BracketClose)?;
                expr = parser.ast_builder.index_access_expr(expr, index, Some(location));
            }
            _ => break,
        }
    }
    
    Ok(expr)
}

pub fn parse_primary(parser: &mut Parser) -> ParserResult<ExprRef> {
    // Add recursion protection
    parser.check_and_increment_recursion()?;
    
    let result = parse_primary_impl(parser);
    
    parser.decrement_recursion();
    result
}

fn parse_primary_impl(parser: &mut Parser) -> ParserResult<ExprRef> {
    match parser.peek() {
        Some(Kind::ParenOpen) => {
            parser.next();
            let node = parser.parse_expr_impl()?;
            parser.expect_err(&Kind::ParenClose)?;
            Ok(node)
        }
        Some(ref kind) if kind.is_keyword() && !matches!(kind, Kind::True | Kind::False | Kind::Null | Kind::If | Kind::Dict) => {
            let location = parser.current_source_location();
            return Err(ParserError::generic_error(location, format!("parse_primary_impl: reserved keyword cannot be used as identifier")))
        }
        Some(Kind::Identifier(s)) => {
            let s = s.to_string();
            let s = parser.string_interner.get_or_intern(s);
            parser.next();
            
            // Check for qualified identifier (module::function)
            if parser.peek() == Some(&Kind::DoubleColon) {
                let mut qualified_path = vec![s];
                
                while parser.peek() == Some(&Kind::DoubleColon) {
                    parser.next(); // consume '::'
                    
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
                
                // Handle qualified function calls
                match parser.peek() {
                    Some(Kind::ParenOpen) => {
                        let location = parser.current_source_location();
                        parser.next();
                        let args = parse_expr_list(parser, vec![])?;
                        parser.expect_err(&Kind::ParenClose)?;
                        // For qualified function calls, use the last part as function name
                        let function_name = qualified_path.last().copied().unwrap_or(s);
                        let expr = parser.ast_builder.call_expr(function_name, args, Some(location));
                        Ok(expr)
                    }
                    _ => {
                        let location = parser.current_source_location();
                        Ok(parser.ast_builder.qualified_identifier_expr(qualified_path, Some(location)))
                    }
                }
            } else {
                // Regular identifier handling
                match parser.peek() {
                    Some(Kind::ParenOpen) => {
                        let location = parser.current_source_location();
                        parser.next();
                        let args = parse_expr_list(parser, vec![])?;
                        parser.expect_err(&Kind::ParenClose)?;
                        let expr = parser.ast_builder.call_expr(s, args, Some(location));
                        Ok(expr)
                    }
                    Some(Kind::BracketOpen) => {
                        let location = parser.current_source_location();
                        parser.next();
                        let index = parser.parse_expr_impl()?;
                        parser.expect_err(&Kind::BracketClose)?;
                        let object_ref = parser.ast_builder.identifier_expr(s, None);
                        Ok(parser.ast_builder.index_access_expr(object_ref, index, Some(location)))
                    }
                    Some(Kind::BraceOpen) => {
                        let location = parser.current_source_location();
                        parser.next();
                        let fields = parse_struct_literal_fields(parser, vec![])?;
                        parser.expect_err(&Kind::BraceClose)?;
                        Ok(parser.ast_builder.struct_literal_expr(s, fields, Some(location)))
                    }
                    _ => {
                        let location = parser.current_source_location();
                        Ok(parser.ast_builder.identifier_expr(s, Some(location)))
                    }
                }
            }
        }
        x => {
            let e = Ok(match x {
                Some(&Kind::UInt64(num)) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.uint64_expr(num, Some(location))
                },
                Some(&Kind::Int64(num)) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.int64_expr(num, Some(location))
                },
                Some(&Kind::Null) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.null_expr(Some(location))
                },
                Some(&Kind::True) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.bool_true_expr(Some(location))
                },
                Some(&Kind::False) => {
                    let location = parser.current_source_location();
                    parser.ast_builder.bool_false_expr(Some(location))
                },
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
                x => {
                    return match x {
                        Some(Kind::ParenOpen) => {
                            parser.next();
                            let e = parser.parse_expr_impl()?;
                            parser.expect_err(&Kind::ParenClose)?;
                            Ok(e)
                        }
                        Some(Kind::BraceOpen) => {
                            parse_block(parser)
                        }
                        Some(Kind::BracketOpen) => {
                            let location = parser.current_source_location();
                            parser.next();
                            let elements = parse_array_elements(parser, vec![])?;
                            parser.expect_err(&Kind::BracketClose)?;
                            Ok(parser.ast_builder.array_literal_expr(elements, Some(location)))
                        }
                        Some(Kind::If) => {
                            parser.next();
                            parse_if(parser)
                        }
                        Some(Kind::Dict) => {
                            parser.next();
                            parse_dict_literal(parser)
                        }
                        _ => {
                            let x_cloned = x.cloned();
                            parser.collect_error(&format!("unexpected token in primary expression: {:?}", x_cloned));
                            // Don't consume the token here - let the caller handle it
                            // This is inside a nested return statement, so we don't call parser.next()
                            Ok(parser.ast_builder.null_expr(None)) // Return dummy expression
                        }
                    }
                }
            });
            parser.next();
            e
        }
    }
}

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

pub fn parse_array_elements(parser: &mut Parser, mut elements: Vec<ExprRef>) -> ParserResult<Vec<ExprRef>> {
    // Enter array literal context for format-independent parsing
    parser.enter_nested_structure(false);
    
    // Dynamic element limit based on parsing complexity
    let base_max_elements = 2000;
    let complexity_score = parser.get_complexity_score();
    let max_elements = base_max_elements + (complexity_score * 100); // More elements allowed for complex structures
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
        
        match parser.peek() {
            Some(Kind::BracketClose) => {
                parser.exit_nested_structure(false);
                return Ok(elements);
            }
            _ => (),
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
                    _ => continue, // Continue the loop for next element
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
                    _ => continue, // Continue the loop for next element
                }
            }
            x => {
                let x_cloned = x.cloned();
                parser.collect_error(&format!("unexpected token in array elements: {:?}", x_cloned));
                parser.exit_nested_structure(false);
                return Ok(elements); // Return current elements and stop
            }
        }
    }
}

pub fn parse_struct_literal_fields(parser: &mut Parser, fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<Vec<(DefaultSymbol, ExprRef)>> {
    // Add recursion protection for struct literal field parsing
    parser.check_and_increment_recursion()?;
    
    let result = parse_struct_literal_fields_impl(parser, fields);
    
    parser.decrement_recursion();
    result
}

fn parse_struct_literal_fields_impl(parser: &mut Parser, mut fields: Vec<(DefaultSymbol, ExprRef)>) -> ParserResult<Vec<(DefaultSymbol, ExprRef)>> {
    // Enter struct literal context for format-independent parsing
    parser.enter_nested_structure(true);
    
    if parser.peek() == Some(&Kind::BraceClose) {
        parser.exit_nested_structure(true);
        return Ok(fields);
    }

    // Dynamic field limit based on parsing complexity
    let base_max_fields = 200;
    let complexity_score = parser.get_complexity_score();
    let max_fields = base_max_fields + (complexity_score * 20); // More fields allowed for complex structures
    let mut field_count = 0;

    loop {
        field_count += 1;
        if field_count > max_fields {
            parser.collect_error(&format!("too many fields in struct literal (max: {}, complexity: {})", 
                                         max_fields, complexity_score));
            parser.exit_nested_structure(true);
            return Ok(fields);
        }

        let field_name = match parser.peek() {
            Some(Kind::Identifier(name)) => {
                let name = name.to_string();
                let symbol = parser.string_interner.get_or_intern(name);
                parser.next();
                symbol
            }
            _ => {
                parser.collect_error("expected field name in struct literal");
                parser.exit_nested_structure(true);
                return Ok(fields); // Return current fields and stop
            }
        };

        let has_colon = parser.peek() == Some(&Kind::Colon);
        if !parser.expect_or_collect(has_colon, "expected ':' after struct field name") {
            parser.exit_nested_structure(true);
            return Ok(fields);
        }
        parser.next();

        let field_value = match parser.parse_expr_impl() {
            Ok(expr) => expr,
            Err(_) => {
                parser.collect_error("failed to parse field value");
                parser.exit_nested_structure(true);
                return Ok(fields);
            }
        };

        fields.push((field_name, field_value));

        match parser.peek() {
            Some(&Kind::Comma) => {
                parser.next();
                if parser.peek() == Some(&Kind::BraceClose) {
                    break;
                }
            }
            Some(&Kind::BraceClose) => break,
            _ => {
                parser.collect_error("expected ',' or '}' in struct literal fields");
                break; // Stop processing and return current fields
            }
        }
    }

    // Exit struct literal context before returning
    parser.exit_nested_structure(true);
    Ok(fields)
}