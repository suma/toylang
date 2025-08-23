use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::token::Kind;
use crate::type_checker::SourceLocation;
use super::token_source::{TokenProvider, LexerTokenSource, TokenNormalizationContext};

use string_interner::DefaultStringInterner;
use crate::parser::error::{ParserError, ParserErrorKind, ParserResult, MultipleParserResult};

pub mod lexer {
    include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
}

/// Parser wrapper that owns its string interner (for backward compatibility)
pub struct ParserWithInterner {
    input: String,
    string_interner: DefaultStringInterner,
    parser: Option<Parser<'static>>,
    pub errors: Vec<ParserError>,
}

impl ParserWithInterner {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.to_string(),
            string_interner: DefaultStringInterner::with_capacity(256),
            parser: None,
            errors: Vec::with_capacity(16),
        }
    }
    
    fn ensure_parser(&mut self) {
        if self.parser.is_none() {
            // Create parser with 'static lifetime hack - safe because we own the input string
            let parser = unsafe {
                let input_ref: &'static str = std::mem::transmute(self.input.as_str());
                let interner_ref: &'static mut DefaultStringInterner = std::mem::transmute(&mut self.string_interner);
                Parser::new(input_ref, interner_ref)
            };
            self.parser = Some(parser);
        }
    }
    
    fn get_parser(&mut self) -> &mut Parser<'static> {
        self.ensure_parser();
        self.parser.as_mut().unwrap()
    }
    
    /// Helper method to execute parser method and copy errors
    fn call_parser_with_error_copy<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Parser<'static>) -> T,
    {
        let result = f(self.get_parser());
        // Copy errors from the internal parser
        self.errors = self.get_parser().errors.clone();
        result
    }
    
    pub fn parse_program(&mut self) -> ParserResult<Program> {
        self.call_parser_with_error_copy(|parser| parser.parse_program())
    }
    
    pub fn get_string_interner(&mut self) -> &mut DefaultStringInterner {
        &mut self.string_interner
    }
    
    pub fn parse_param_def(&mut self) -> ParserResult<Parameter> {
        self.call_parser_with_error_copy(|parser| parser.parse_param_def())
    }
    
    pub fn parse_param_def_list(&mut self, args: Vec<Parameter>) -> ParserResult<Vec<Parameter>> {
        self.call_parser_with_error_copy(|parser| parser.parse_param_def_list(args))
    }
    
    pub fn parse_program_multiple_errors(&mut self) -> MultipleParserResult<Program> {
        self.call_parser_with_error_copy(|parser| parser.parse_program_multiple_errors())
    }
    
    // Forward methods to internal parser
    pub fn peek(&mut self) -> Option<&Kind> {
        self.get_parser().peek()
    }
    
    pub fn peek_n(&mut self, pos: usize) -> Option<&Kind> {
        self.get_parser().peek_n(pos)
    }
    
    pub fn next(&mut self) -> Option<Kind> {
        let token = self.get_parser().peek().cloned();
        self.get_parser().next();
        token
    }
    
    pub fn parse_stmt(&mut self) -> ParserResult<StmtRef> {
        self.call_parser_with_error_copy(|parser| parser.parse_stmt())
    }
    
    pub fn parse_expr_impl(&mut self) -> ParserResult<ExprRef> {
        self.call_parser_with_error_copy(|parser| parser.parse_expr_impl())
    }
    
    pub fn get_expr_pool(&self) -> &ExprPool {
        match &self.parser {
            Some(parser) => parser.get_expr_pool(),
            None => {
                // Return reference to an empty pool - using thread_local for safety
                thread_local! {
                    static EMPTY_EXPR_POOL: ExprPool = ExprPool(Vec::new());
                }
                EMPTY_EXPR_POOL.with(|pool| unsafe { 
                    std::mem::transmute::<&ExprPool, &'static ExprPool>(pool) 
                })
            }
        }
    }
    
    pub fn get_stmt_pool(&self) -> &StmtPool {
        match &self.parser {
            Some(parser) => parser.get_stmt_pool(),
            None => {
                // Return reference to an empty pool - using thread_local for safety
                thread_local! {
                    static EMPTY_STMT_POOL: StmtPool = StmtPool(Vec::new());
                }
                EMPTY_STMT_POOL.with(|pool| unsafe { 
                    std::mem::transmute::<&StmtPool, &'static StmtPool>(pool) 
                })
            }
        }
    }
}

/// Parsing context to track where we are in the syntax tree
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseContext {
    /// Normal expression context where struct literals are allowed
    Expression,
    /// Condition context (while, if conditions) where struct literals are not allowed
    Condition,
    /// Statement context where struct literals may be restricted
    Statement,
}

pub struct Parser<'a> {
    token_provider: TokenProvider<LexerTokenSource<'a>>,
    pub ast_builder: AstBuilder,
    pub string_interner: &'a mut DefaultStringInterner,
    pub builtin_symbols: BuiltinFunctionSymbols,
    pub errors: Vec<ParserError>,
    input: &'a str,
    recursion_depth: u32,
    max_recursion_depth: u32,
    /// Context for format-independent token processing
    normalization_context: TokenNormalizationContext,
    /// Stack of parsing contexts to track where we are
    context_stack: Vec<ParseContext>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str, string_interner: &'a mut DefaultStringInterner) -> Self {
        let source = LexerTokenSource::new(input);
        let builtin_symbols = BuiltinFunctionSymbols::new(string_interner);
        Parser {
            token_provider: TokenProvider::with_format_normalization(source, 128, 64),
            ast_builder: AstBuilder::with_capacity(1024, 1024),
            string_interner,
            builtin_symbols,
            errors: Vec::with_capacity(4),
            input,
            recursion_depth: 0,
            max_recursion_depth: 500, // Significantly increased for complex nested structures
            normalization_context: TokenNormalizationContext::new(),
            context_stack: vec![ParseContext::Expression], // Start with expression context
        }
    }

    /// Create a new parser with owned string interner (for backward compatibility/testing)
    pub fn new_standalone(input: &str) -> ParserWithInterner {
        ParserWithInterner::new(input)
    }

    /// Push a new parsing context onto the stack
    pub fn push_context(&mut self, context: ParseContext) {
        self.context_stack.push(context);
    }

    /// Pop the current parsing context from the stack
    pub fn pop_context(&mut self) {
        if self.context_stack.len() > 1 {
            self.context_stack.pop();
        }
    }

    /// Get the current parsing context
    pub fn current_context(&self) -> ParseContext {
        *self.context_stack.last().unwrap_or(&ParseContext::Expression)
    }

    /// Check if struct literals are allowed in the current context
    pub fn is_struct_literal_allowed(&self) -> bool {
        match self.current_context() {
            ParseContext::Expression => true,
            ParseContext::Condition => false,
            ParseContext::Statement => false,
        }
    }

    pub fn peek(&mut self) -> Option<&Kind> {
        self.token_provider.peek()
    }

    #[allow(dead_code)]
    pub fn peek_n(&mut self, pos: usize) -> Option<&Kind> {
        self.token_provider.peek_at(pos)
    }

    pub fn peek_position_n(&mut self, pos: usize) -> Option<&std::ops::Range<usize>> {
        self.token_provider.peek_position_at(pos)
    }

    pub fn current_position(&mut self) -> Option<&std::ops::Range<usize>> {
        self.token_provider.peek_position_at(0)
    }

    /// Get current source location with line and column information
    pub fn current_source_location(&mut self) -> SourceLocation {
        if let Some(position) = self.current_position() {
            let offset = position.start;
            let (line, column) = self.offset_to_line_col(offset);
            SourceLocation {
                line,
                column,
                offset: offset as u32,
            }
        } else {
            // Default location when no position is available (e.g., at EOF)
            let input_len = self.input.len();
            let (line, column) = self.offset_to_line_col(input_len);
            SourceLocation {
                line,
                column,
                offset: input_len as u32,
            }
        }
    }

    /// Calculate line and column from absolute offset
    fn offset_to_line_col(&self, offset: usize) -> (u32, u32) {
        let mut line = 1u32;
        let mut column = 1u32;
        
        for (i, ch) in self.input.char_indices() {
            if i >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        
        (line, column)
    }

    pub fn next(&mut self) {
        self.token_provider.advance();
    }

    pub fn line_count(&mut self) -> usize {
        self.token_provider.line_count()
    }

    pub fn expect(&mut self, accept: &Kind) -> ParserResult<()> {
        let tk = self.peek();
        if tk.is_some() && *tk.unwrap() == *accept {
            self.next();
            Ok(())
        } else {
            let current = self.peek().map(|k| k.clone()).unwrap_or(Kind::EOF);
            let location = self.current_source_location();
            Err(ParserError::generic_error(location, 
                format!("Expected {:?} but found {:?}", accept, current)))
        }
    }

    pub fn expect_err(&mut self, accept: &Kind) -> ParserResult<()> {
        let tk = self.peek();
        if tk.is_some() && *tk.unwrap() == *accept {
            self.next();
            Ok(())
        } else {
            let location = self.current_source_location();
            self.errors.push(ParserError::unexpected_token(location, format!("{:?}", accept)));
            self.next();
            Ok(())
        }
    }

    /// Collect error without stopping parse, used for multiple error collection
    pub fn collect_error(&mut self, error_msg: &str) {
        let location = self.current_source_location();
        self.errors.push(ParserError::unexpected_token(location, error_msg.to_string()));
    }

    /// Check condition and collect error if failed, continue parsing  
    pub fn expect_or_collect(&mut self, condition: bool, error_msg: &str) -> bool {
        if !condition {
            self.collect_error(error_msg);
            false
        } else {
            true
        }
    }

    /// Check recursion depth using normalized complexity scoring
    pub fn check_and_increment_recursion(&mut self) -> ParserResult<()> {
        // Use significantly more aggressive depth management for format-independent parsing
        let complexity_score = self.normalization_context.complexity_score();
        
        // For format-normalized parsing, be much more permissive
        let base_depth = if self.token_provider.normalize_formatting { 800 } else { self.max_recursion_depth };
        let adjusted_max_depth = base_depth + (complexity_score / 2) as u32;
        
        if self.recursion_depth >= adjusted_max_depth {
            self.collect_error(&format!("Maximum recursion depth reached in parser (depth: {}, complexity: {}, adjusted_max: {})", 
                                      self.recursion_depth, complexity_score, adjusted_max_depth));
            let location = self.current_source_location();
            return Err(ParserError::recursion_limit_exceeded(location));
        }
        self.recursion_depth += 1;
        Ok(())
    }

    /// Decrement recursion depth
    pub fn decrement_recursion(&mut self) {
        if self.recursion_depth > 0 {
            self.recursion_depth -= 1;
        }
    }

    /// Enter a nested structure context (for format-independent parsing)
    pub fn enter_nested_structure(&mut self, is_struct: bool) {
        self.normalization_context.enter_nested_structure(is_struct);
    }

    /// Exit a nested structure context
    pub fn exit_nested_structure(&mut self, is_struct: bool) {
        self.normalization_context.exit_nested_structure(is_struct);
    }

    /// Get current parsing complexity score
    pub fn get_complexity_score(&self) -> usize {
        self.normalization_context.complexity_score()
    }

    pub fn next_expr(&self) -> u32 {
        self.ast_builder.get_expr_pool().len() as u32
    }

    pub fn get_expr_pool(&self) -> &ExprPool {
        self.ast_builder.get_expr_pool()
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        self.ast_builder.get_stmt_pool()
    }

    pub fn get_string_interner(&mut self) -> &mut DefaultStringInterner {
        &mut self.string_interner
    }


    pub fn parse_program(&mut self) -> ParserResult<Program> {
        let mut start_pos: Option<usize> = None;
        let mut end_pos: Option<usize> = None;
        let mut update_start_pos = |start: usize| {
            if start_pos.is_none() || start_pos.unwrap() < start {
                start_pos = Some(start);
            }
        };
        let mut update_end_pos = |end: usize| {
            end_pos = Some(end);
        };
        let mut def_func = vec![];
        
        // Parse package declaration (optional, at beginning of file)
        let package_decl = if matches!(self.peek(), Some(Kind::Package)) {
            Some(self.parse_package_decl()?)
        } else {
            None
        };
        
        // Parse import declarations (multiple allowed)
        let mut imports = Vec::new();
        while matches!(self.peek(), Some(Kind::Import)) {
            imports.push(self.parse_import_decl()?);
        }

        loop {
            // Check for visibility modifier first
            let visibility = if matches!(self.peek(), Some(Kind::Public)) {
                self.next(); // consume 'pub'
                Visibility::Public
            } else {
                Visibility::Private
            };
            
            match self.peek() {
                Some(Kind::Function) => {
                    let fn_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(fn_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let s = s.to_string();
                            let fn_name = self.string_interner.get_or_intern(s);
                            self.next();

                            self.expect_err(&Kind::ParenOpen)?;
                            let params = self.parse_param_def_list(vec![])?;
                            self.expect_err(&Kind::ParenClose)?;
                            let mut ret_ty: Option<TypeDecl> = None;
                            match self.peek() {
                                Some(Kind::Arrow) => {
                                    self.expect_err(&Kind::Arrow)?;
                                    ret_ty = Some(self.parse_type_declaration()?);
                                }
                                _ => (),
                            }
                            let block = super::expr::parse_block(self)?;
                            let fn_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(fn_end_pos);
                            
                            def_func.push(Rc::new(Function{
                                node: Node::new(fn_start_pos, fn_end_pos),
                                name: fn_name,
                                parameter: params,
                                return_type: ret_ty,
                                code: self.ast_builder.expression_stmt(block, Some(location)),
                                visibility,
                            }));
                        }
                        _ => {
                            self.collect_error("expected function name");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::Struct) => {
                    let struct_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(struct_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let struct_name = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let fields = super::stmt::parse_struct_fields(self, vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let struct_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(struct_end_pos);
                            
                            self.ast_builder.struct_decl_stmt(struct_name, fields, visibility, Some(location));
                        }
                        _ => {
                            self.collect_error("expected struct name");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::Impl) => {
                    let impl_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(impl_start_pos);
                    self.next();
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let target_type = s.to_string();
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let methods = super::stmt::parse_impl_methods(self, vec![])?;
                            self.expect_err(&Kind::BraceClose)?;
                            let impl_end_pos = self.peek_position_n(0).unwrap_or_else(|| &std::ops::Range {start: 0, end: 0}).end;
                            update_end_pos(impl_end_pos);
                            
                            self.ast_builder.impl_block_stmt(target_type, methods, Some(location));
                        }
                        _ => {
                            self.collect_error("expected type name for impl block");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::NewLine) => {
                    self.next()
                }
                None | Some(Kind::EOF) => {
                    // Check if 'pub' was used without any declaration
                    if matches!(visibility, Visibility::Public) {
                        self.collect_error("'pub' keyword must be followed by a function or struct declaration");
                    }
                    break;
                }
                x => {
                    let x_cloned = x.cloned();
                    // Check if 'pub' was used with unsupported elements
                    if matches!(visibility, Visibility::Public) {
                        match &x_cloned {
                            Some(Kind::Impl) => {
                                self.collect_error("'pub' is not yet supported for impl blocks");
                            }
                            _ => {
                                self.collect_error("'pub' can only be used with function and struct declarations");
                            }
                        }
                    }
                    self.collect_error(&format!("unexpected token: {:?}", x_cloned));
                    self.next(); // Skip invalid token and continue
                }
            }
        }

        // Check if there were critical errors during parsing (like keyword usage)
        for error in &self.errors {
            // Check both direct GenericError and nested errors in UnexpectedToken
            match &error.kind {
                ParserErrorKind::GenericError { message } => {
                    if message.contains("reserved keyword") {
                        return Err(error.clone());
                    }
                }
                ParserErrorKind::UnexpectedToken { expected } => {
                    if expected.contains("reserved keyword") {
                        return Err(error.clone());
                    }
                }
                _ => {}
            }
        }

        let mut ast_builder = AstBuilder::new();
        std::mem::swap(&mut ast_builder, &mut self.ast_builder);
        let (expr, stmt, location_pool) = ast_builder.extract_pools();
        Ok(Program{
            node: Node::new(start_pos.unwrap_or(0usize), end_pos.unwrap_or(0usize)),
            package_decl,
            imports,
            function: def_func,
            statement: stmt,
            expression: expr,
            location_pool,
        })
    }

    pub fn parse_param_def(&mut self) -> ParserResult<Parameter> {
        let current_token = self.peek().cloned();
        match current_token {
            Some(Kind::Identifier(s)) => {
                let name = self.string_interner.get_or_intern(s);
                self.next();
                self.expect_err(&Kind::Colon)?;
                let typ = self.parse_type_declaration()?;
                Ok((name, typ))
            }
            x => {
                let location = self.current_source_location();
                Err(ParserError::generic_error(location, format!("expect type parameter of function but: {:?}", x)))
            },
        }
    }

    pub fn parse_param_def_list(&mut self, mut args: Vec<Parameter>) -> ParserResult<Vec<Parameter>> {
        // Limit maximum number of parameters to prevent infinite loops
        const MAX_PARAMS: usize = 255;
        
        loop {
            if self.peek() == Some(&Kind::ParenClose) || args.len() >= MAX_PARAMS {
                if args.len() >= MAX_PARAMS {
                    self.collect_error(&format!("too many parameters (max: {})", MAX_PARAMS));
                }
                return Ok(args);
            }

            let def = self.parse_param_def();
            if def.is_err() {
                return Ok(args);
            }
            args.push(def?);

            match self.peek() {
                Some(Kind::Comma) => {
                    self.next();
                    // Continue loop to parse next parameter
                }
                _ => {
                    return Ok(args);
                }
            }
        }
    }

    pub fn parse_type_declaration(&mut self) -> ParserResult<TypeDecl> {
        match self.peek() {
            Some(Kind::BracketOpen) => {
                self.next();
                let element_type = self.parse_type_declaration()?;
                self.expect_err(&Kind::Semicolon)?;
                
                let size = match self.peek().cloned() {
                    Some(Kind::UInt64(n)) => {
                        self.next();
                        n as usize
                    }
                    Some(Kind::Integer(s)) => {
                        self.next();
                        s.parse::<usize>().map_err(|_| {
                            let location = self.current_source_location();
                            ParserError::generic_error(location, format!("Invalid array size: {}", s))
                        })?
                    }
                    _ => {
                        let location = self.current_source_location();
                        return Err(ParserError::generic_error(location, "Expected array size or underscore".to_string()))
                    }
                };
                
                self.expect_err(&Kind::BracketClose)?;
                Ok(TypeDecl::Array(vec![element_type; size], size))
            }
            Some(Kind::Bool) => {
                self.next();
                Ok(TypeDecl::Bool)
            }
            Some(Kind::U64) => {
                self.next();
                Ok(TypeDecl::UInt64)
            }
            Some(Kind::I64) => {
                self.next();
                Ok(TypeDecl::Int64)
            }
            Some(Kind::Ptr) => {
                self.next();
                Ok(TypeDecl::Ptr)
            }
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let ident = self.string_interner.get_or_intern(s);
                self.next();
                Ok(TypeDecl::Identifier(ident))
            }
            Some(Kind::Str) => {
                self.next();
                Ok(TypeDecl::String)
            }
            Some(Kind::Self_) => {
                self.next();
                Ok(TypeDecl::Self_)
            }
            Some(Kind::Dict) => {
                self.next();
                self.expect_err(&Kind::BracketOpen)?;
                
                // Parse key type
                let key_type = self.parse_type_declaration()?;
                
                self.expect_err(&Kind::Comma)?;
                
                // Parse value type
                let value_type = self.parse_type_declaration()?;
                
                self.expect_err(&Kind::BracketClose)?;
                Ok(TypeDecl::Dict(Box::new(key_type), Box::new(value_type)))
            }
            Some(_) | None => {
                let location = self.current_source_location();
                Err(ParserError::generic_error(location, format!("parse_type_declaration: unexpected token {:?}", self.peek())))
            }
        }
    }

    pub fn skip_newlines(&mut self) {
        while let Some(Kind::NewLine) = self.peek() {
            self.next();
        }
    }
    
    /// Parse package declaration: package math.basic
    pub fn parse_package_decl(&mut self) -> ParserResult<PackageDecl> {
        self.expect_err(&Kind::Package)?;
        
        let mut name_parts = Vec::new();
        
        // Parse first identifier
        if let Some(Kind::Identifier(s)) = self.peek().cloned() {
            let symbol = self.string_interner.get_or_intern(s);
            name_parts.push(symbol);
            self.next();
        } else {
            return Err(ParserError::generic_error(self.current_source_location(), "expected package name".to_string()));
        }
        
        // Parse additional parts separated by dots
        while matches!(self.peek(), Some(Kind::Dot)) {
            self.next(); // consume dot
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let symbol = self.string_interner.get_or_intern(s);
                name_parts.push(symbol);
                self.next();
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected identifier after '.'".to_string()));
            }
        }
        
        self.skip_newlines();
        Ok(PackageDecl { name: name_parts })
    }
    
    /// Parse import declaration: import math.basic [as alias]
    pub fn parse_import_decl(&mut self) -> ParserResult<ImportDecl> {
        self.expect_err(&Kind::Import)?;
        
        let mut module_path = Vec::new();
        
        // Parse first identifier
        if let Some(Kind::Identifier(s)) = self.peek().cloned() {
            let symbol = self.string_interner.get_or_intern(s);
            module_path.push(symbol);
            self.next();
        } else {
            return Err(ParserError::generic_error(self.current_source_location(), "expected module name".to_string()));
        }
        
        // Parse additional parts separated by dots
        while matches!(self.peek(), Some(Kind::Dot)) {
            self.next(); // consume dot
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let symbol = self.string_interner.get_or_intern(s);
                module_path.push(symbol);
                self.next();
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected identifier after '.'".to_string()));
            }
        }
        
        // Parse optional alias: as alias_name
        let alias = if matches!(self.peek(), Some(Kind::As)) {
            self.next(); // consume 'as'
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let alias_symbol = self.string_interner.get_or_intern(s);
                self.next();
                Some(alias_symbol)
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected alias name after 'as'".to_string()));
            }
        } else {
            None
        };
        
        self.skip_newlines();
        Ok(ImportDecl { module_path, alias })
    }

    /// Parse program with multiple error collection
    pub fn parse_program_multiple_errors(&mut self) -> MultipleParserResult<Program> {
        self.errors.clear();
        
        match self.parse_program() {
            Ok(program) => {
                if self.errors.is_empty() {
                    MultipleParserResult::success(program)
                } else {
                    MultipleParserResult::with_errors(program, self.errors.clone())
                }
            }
            Err(_) => {
                MultipleParserResult::failure(self.errors.clone())
            }
        }
    }
}