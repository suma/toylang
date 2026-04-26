use crate::ast::*;
use crate::token::Kind;
use crate::type_checker::SourceLocation;
use super::token_source::{TokenProvider, LexerTokenSource, TokenNormalizationContext};

use string_interner::DefaultStringInterner;
use crate::parser::error::{ParserError, ParserResult, MultipleParserResult};

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
                    static EMPTY_EXPR_POOL: ExprPool = ExprPool::new();
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
                    static EMPTY_STMT_POOL: StmtPool = StmtPool::new();
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
    /// Extra statements produced as a side effect of parsing a single
    /// `val`/`var` form — used by the tuple-destructuring desugaring
    /// (`val (a, b) = expr` expands to a temporary plus per-name
    /// bindings). `parse_block_impl` drains this buffer immediately
    /// before the return value of `parse_stmt`, preserving source
    /// order.
    pub pending_prelude_stmts: Vec<StmtRef>,
    /// Counter feeding fresh synthetic identifiers (e.g.
    /// `__tuple_tmp_0`, `__tuple_tmp_1`) during desugaring.
    pub synthetic_counter: u32,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str, string_interner: &'a mut DefaultStringInterner) -> Self {
        let source = LexerTokenSource::new(input);
        let builtin_symbols = BuiltinFunctionSymbols::new(string_interner);
        Parser {
            token_provider: TokenProvider::with_format_normalization(source, 128, 64),
            // Increased initial capacity for large codebases (1000-10000 lines)
            // Expression density: ~3000-5000 nodes per 1000 lines
            // Statement density: ~1500-2500 nodes per 1000 lines
            ast_builder: AstBuilder::with_capacity(16384, 8192),
            string_interner,
            builtin_symbols,
            errors: Vec::with_capacity(4),
            input,
            recursion_depth: 0,
            max_recursion_depth: 500,
            normalization_context: TokenNormalizationContext::new(),
            context_stack: vec![ParseContext::Expression],
            pending_prelude_stmts: Vec::new(),
            synthetic_counter: 0,
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

    /// Push a synthetic token to the front of the token stream.
    /// Used for rewriting `>>` into two `>` tokens in nested generic contexts.
    pub(super) fn insert_token(&mut self, token: Kind) {
        self.token_provider.insert_token(token);
    }

    pub fn expect(&mut self, accept: &Kind) -> ParserResult<()> {
        let tk = self.peek();
        if tk.is_some() && *tk.unwrap() == *accept {
            self.next();
            Ok(())
        } else {
            let current = self.peek().cloned().unwrap_or(Kind::EOF);
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

    pub fn skip_newlines(&mut self) {
        while let Some(Kind::NewLine) = self.peek() {
            self.next();
        }
    }

    /// Check if there is a newline in the original source text before the current token.
    /// This is useful for disambiguating postfix operators (like `[`) from new expressions
    /// when format normalization removes newline tokens.
    pub fn has_newline_before_current_token(&mut self) -> bool {
        if let Some(position) = self.current_position() {
            let current_offset = position.start;
            if current_offset > 0 {
                // Scan backwards from current token to find the previous non-whitespace character
                let bytes = self.input.as_bytes();
                let mut i = current_offset;
                while i > 0 {
                    i -= 1;
                    let ch = bytes[i];
                    if ch == b'\n' {
                        return true;
                    }
                    if !ch.is_ascii_whitespace() {
                        return false;
                    }
                }
            }
        }
        false
    }
}
