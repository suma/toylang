use std::collections::HashMap;
use string_interner::DefaultSymbol;

use crate::ast::*;
use crate::token::Kind;
use crate::type_decl::TypeDecl;
use crate::type_checker::SourceLocation;
use super::token_source::{TokenProvider, LexerTokenSource, TokenNormalizationContext};

use string_interner::DefaultStringInterner;
use crate::parser::error::{ParserError, ParserResult, MultipleParserResult};

#[allow(clippy::slow_vector_initialization)]
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
        let result = {
            let parser = self.get_parser();
            let result = f(parser);
            // Lex failures are recorded by the token source as the
            // parse runs; surface them into `parser.errors` so every
            // entry point (`parse_program` / `parse_stmt` /
            // `parse_expr_impl` / ...) reports them. Draining makes
            // repeated calls idempotent.
            parser.merge_lex_errors();
            result
        };
        // Copy errors from the internal parser
        self.errors = self.get_parser().errors.clone();
        result
    }

    pub fn parse_program(&mut self) -> ParserResult<File> {
        self.call_parser_with_error_copy(|parser| parser.parse_program())
    }

    /// Forward to the inner parser's `set_source_file`. Powers the
    /// parser-level `__builtin_source_file()` substitution.
    pub fn set_source_file(&mut self, path: impl Into<String>) {
        let path = path.into();
        self.get_parser().set_source_file(path);
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

    pub fn parse_program_multiple_errors(&mut self) -> MultipleParserResult<File> {
        self.call_parser_with_error_copy(|parser| parser.parse_program_multiple_errors())
    }

    // Forward methods to internal parser
    pub fn peek(&mut self) -> Option<&Kind> {
        self.get_parser().peek()
    }

    pub fn peek_n(&mut self, pos: usize) -> Option<&Kind> {
        self.get_parser().peek_n(pos)
    }

    #[allow(clippy::should_implement_trait)]
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
    /// Index into `errors` marking where the current top-level
    /// declaration started. See [`Parser::report_error`].
    decl_error_floor: usize,
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
    /// ALLOC-CONTRACT: set while parsing an `ensures` predicate, so
    /// `old(...)` is recognised there and refused everywhere else.
    /// Cleared around the argument of an `old` so a nested
    /// `old(old(x))` — which would snapshot the same instant — is a
    /// parse error rather than a silently accepted no-op.
    pub(super) in_ensures_clause: bool,
    /// ALLOC-CONTRACT: the expressions collected from `old(...)` in
    /// the contract clauses of the function being parsed, in the
    /// order met. `parse_contract_clauses` takes this and hands it to
    /// the `Function` / `MethodFunction` it is building; each entry's
    /// index is the `N` in the `__old_N` identifier left behind in
    /// the clause.
    pub(super) old_exprs: Vec<ExprRef>,
    /// ALLOC-CONTRACT-SUGAR: the clause root the last budget sugar
    /// expanded to, and which counter it read. `parse_contract_clauses`
    /// compares the root against the finished clause to tell
    /// `ensures retains(0u64)` (a budget) from
    /// `ensures retains(0u64) && result > 0u64` (a plain predicate
    /// with a budget inside it).
    pub(super) last_alloc_budget: Option<(ExprRef, crate::ast::EnsuresKind)>,
    /// Counter feeding fresh synthetic identifiers (e.g.
    /// `__tuple_tmp_0`, `__tuple_tmp_1`) during desugaring.
    pub synthetic_counter: u32,
    /// Top-level `type Name = TargetType` aliases. Populated as the
    /// parser encounters each declaration; consulted in
    /// `parse_type_declaration` so that any subsequent occurrence of
    /// `Name` in a type position is replaced with `TargetType`. Forward
    /// references are NOT supported — the alias must be declared before
    /// its first use. Each entry stores `(generic_params, target)`:
    /// non-generic aliases have an empty params vector; generic aliases
    /// like `type Pair<T> = (T, T)` carry the parameter symbols and the
    /// target keeps `Generic(T)` placeholders that get substituted at
    /// the use site via `substitute_generics`.
    pub type_aliases: HashMap<DefaultSymbol, (Vec<DefaultSymbol>, TypeDecl)>,
    /// COMPILE-TIME-EVAL C5: the value of each top-level `const` whose
    /// initialiser is an integer literal, so an array length can name
    /// it: `const N: u64 = 3u64` then `val a: [i64; N]`.
    ///
    /// Populated as the parser meets each declaration, and consulted
    /// in `parse_type_declaration` — the same shape, and the same
    /// no-forward-references rule, as `type_aliases` above. It has to
    /// work this way round: a length is baked into `TypeDecl::Array`
    /// while parsing, and every later pass reads it as a number.
    ///
    /// Only literals and chains of them. Arithmetic would mean a
    /// fourth evaluator living in the parser, which is the thing
    /// `COMPILE_TIME_EVAL.md` exists to prevent; a length that needs
    /// computing is refused with a message saying so.
    pub const_lengths: HashMap<DefaultSymbol, u64>,
    /// Generic parameters declared on each `struct` / `enum`, by type
    /// name.
    ///
    /// Exists so `impl Container<T>` can mean what the language
    /// reference says it means — "the type parameter list on `impl` is
    /// implicit, re-using the parameter declared on `struct`". Without
    /// the declaration to compare against, the parser cannot tell that
    /// `T` from the `u8` in `impl Vec<u8>`: both are just a type
    /// argument at that position. Matching against the declaration is
    /// what distinguishes them, so the concrete-args form keeps
    /// working unchanged.
    ///
    /// Consequence: the type has to be declared before the `impl` that
    /// uses the implicit form. The explicit `impl<T> Container<T>`
    /// has no such ordering requirement.
    pub declared_type_generics: HashMap<DefaultSymbol, Vec<DefaultSymbol>>,
    /// Source file path for `__builtin_source_file()` substitution.
    /// `None` defaults to `"<source>"`. Set via `set_source_file` when
    /// the entry point knows the on-disk path (e.g. `interpreter` CLI,
    /// module loader); test / bench / inline-string parser sites
    /// typically leave it unset.
    pub source_file: Option<String>,
    /// Byte offset of the start of every line, built once in `new`.
    /// FRONTEND-PERF: `offset_to_line_col` used to walk the whole input
    /// from the top on every call (141 call sites), making the parse
    /// O(n²) on large files (20k lines ~100s). A binary search over
    /// this table is O(log lines) plus one line-length char count.
    line_starts: Vec<usize>,
}

/// Offsets where each line begins (line 1 starts at 0, every later
/// line right after a `\n`).
fn build_line_starts(input: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(input.len() / 32 + 1);
    starts.push(0);
    for (i, b) in input.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
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
            decl_error_floor: 0,
            input,
            recursion_depth: 0,
            max_recursion_depth: 500,
            normalization_context: TokenNormalizationContext::new(),
            context_stack: vec![ParseContext::Expression],
            pending_prelude_stmts: Vec::new(),
            synthetic_counter: 0,
            in_ensures_clause: false,
            old_exprs: Vec::new(),
            last_alloc_budget: None,
            type_aliases: HashMap::new(),
            const_lengths: HashMap::new(),
            declared_type_generics: HashMap::new(),
            source_file: None,
            line_starts: build_line_starts(input),
        }
    }

    /// Set the source file path used by `__builtin_source_file()`
    /// substitution. Call this immediately after `Parser::new` from
    /// any entry point that knows the on-disk path (e.g. the
    /// interpreter binary's main loop, or the module loader).
    pub fn set_source_file(&mut self, path: impl Into<String>) {
        self.source_file = Some(path.into());
    }

    /// Borrow a substring of the original source. Used by
    /// `__builtin_dbg(expr)` to recover the textual form of `expr`
    /// from the byte range of its tokens.
    pub fn source_substring(&self, range: std::ops::Range<usize>) -> &str {
        &self.input[range]
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

    /// A [`SourceLocation`] for an arbitrary byte span, rather than
    /// for the token at the cursor. INTERP-DIAG-SPAN: the
    /// string-interpolation desugaring reports against spans it
    /// recorded before consuming the literal, so it cannot use
    /// [`current_source_location`].
    pub(super) fn location_from_span(
        &self,
        span: &std::ops::Range<usize>,
    ) -> SourceLocation {
        let (line, column) = self.offset_to_line_col(span.start);
        SourceLocation::new(line, column, span.start as u32, span.end as u32)
    }

    /// A span running from `start` to wherever the cursor now sits.
    ///
    /// A node's own location is the token that *names* it — a binary
    /// expression is located at its operator, a qualified path at its
    /// first segment. That is enough to point a caret at, but not
    /// enough to quote: `suggest_numeric_cast` builds its replacement
    /// out of the span's source text, so a span narrower than the
    /// expression yields an edit that does not mean what it says
    /// (`a + b` produced `+ as i64`). Callers that have just finished
    /// consuming a construct widen its location with this.
    ///
    /// The end is derived from the *start* of the following token and
    /// then walked back over whitespace, since the token source does
    /// not expose the previous token's end. Without the walk-back a
    /// span reaches to the next token — across the newline and the
    /// indentation when the construct ends a line — and the caret runs
    /// to the end of its line.
    pub fn span_to_cursor(&mut self, start: SourceLocation) -> SourceLocation {
        let cursor = self
            .current_position()
            .map(|p| p.start)
            .unwrap_or(self.input.len());
        let end = self.input[..cursor.min(self.input.len())]
            .trim_end()
            .len() as u32;
        SourceLocation::new(
            start.line,
            start.column,
            start.offset,
            end.max(start.end_offset),
        )
    }

    /// Get current source location with line and column information
    pub fn current_source_location(&mut self) -> SourceLocation {
        if let Some(position) = self.current_position() {
            let offset = position.start;
            // The current token's extent, so a diagnostic anchored here
            // gets a caret the width of the token rather than a guess.
            let end = position.end;
            let (line, column) = self.offset_to_line_col(offset);
            SourceLocation {
                line,
                column,
                offset: offset as u32,
                end_offset: end as u32,
            }
        } else {
            // Default location when no position is available (e.g., at EOF)
            let input_len = self.input.len();
            let (line, column) = self.offset_to_line_col(input_len);
            SourceLocation {
                line,
                column,
                offset: input_len as u32,
                // Nothing left to underline at EOF.
                end_offset: input_len as u32,
            }
        }
    }

    /// Calculate line and column from absolute offset.
    ///
    /// FRONTEND-PERF: binary search over the precomputed line-start
    /// table. Line is 1-based; column is the 1-based count of chars
    /// from the start of the line to `offset` (not bytes — the old
    /// linear walk counted `char_indices`, so diagnostics report
    /// columns in characters).
    fn offset_to_line_col(&self, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.input.len());
        // `partition_point` counts the line starts <= offset; the first
        // entry is 0 so the count is at least 1.
        let line_index = self.line_starts.partition_point(|&s| s <= offset) - 1;
        let line_start = self.line_starts[line_index];
        let column = self.input[line_start..offset].chars().count() as u32 + 1;
        (line_index as u32 + 1, column)
    }

    pub fn next(&mut self) {
        self.token_provider.advance();
    }

    pub fn line_count(&mut self) -> usize {
        self.token_provider.line_count()
    }

    /// Collect lexical failures recorded by the token source into
    /// `self.errors` as ordinary parse errors, so they surface through
    /// the existing parse-error reporting and fail the parse.
    ///
    /// The lexer *skips* a failing token (or a single character when
    /// no rule matched at all) and keeps going, so the parser runs to
    /// the end; the error is only recorded here. That is deliberate:
    /// the token source has no way to reach `self.errors`, and
    /// reporting the token stream's damage instead — "expected
    /// expression" at the next statement — is exactly the misleading
    /// diagnostic this replaces.
    ///
    /// Lex errors are inserted at the *front* of the error list: a
    /// lexical failure usually desynchronises the parse around it, so
    /// in the single-error path (`parse_program`) the root cause is
    /// what gets reported. The multi-error path sorts by position
    /// anyway, so the insertion order is invisible there.
    pub fn merge_lex_errors(&mut self) {
        let lex_errors = self.token_provider.drain_lex_errors();
        if lex_errors.is_empty() {
            return;
        }
        for lex in lex_errors {
            let start = lex.span.start.min(self.input.len());
            let mut end = lex.span.end.min(self.input.len());
            // A failed token can span several lines (an unterminated
            // string matches to end of input); quoting everything to
            // EOF would drown the message in the rest of the file.
            // Clamp to the end of the first line.
            if let Some(nl) = self.input[start..end].find('\n') {
                end = start + nl;
            }
            let (line, column) = self.offset_to_line_col(start);
            let text = &self.input[start..end];
            let error = ParserError::lex_error(
                SourceLocation::new(line, column, start as u32, end as u32),
                lex.kind.describe(text),
            );
            self.errors.insert(0, error);
        }
    }

    /// Push a synthetic token to the front of the token stream.
    /// Used for rewriting `>>` into two `>` tokens in nested generic contexts.
    pub(super) fn insert_token(&mut self, token: Kind) {
        self.token_provider.insert_token(token);
    }

    /// Insert a synthesized token with the span it should report
    /// (INTERP-DIAG-SPAN). See
    /// [`TokenProvider::insert_token_at`][super::token_source].
    pub(super) fn insert_token_at(
        &mut self,
        token: Kind,
        position: std::ops::Range<usize>,
    ) {
        self.token_provider.insert_token_at(token, position);
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
            let error = ParserError::unexpected_token(location, format!("{:?}", accept));
            self.report_error(error);
            self.next();
            Ok(())
        }
    }

    /// Collect error without stopping parse, used for multiple error collection
    /// Mark the start of a top-level declaration.
    ///
    /// LLM-LOOP P1: the parser resynchronises reliably at declaration
    /// boundaries but not inside one — after a bad token it keeps
    /// stumbling through the rest of the body, collecting consequences
    /// of the same mistake ("unexpected token: x", "unexpected token:
    /// }"). Those read as separate problems and send a reader chasing
    /// code that is fine. One report per declaration keeps the errors
    /// that are genuinely independent (one per broken declaration) and
    /// drops the derivative ones.
    pub fn begin_declaration(&mut self) {
        self.decl_error_floor = self.errors.len();
    }

    /// Record a parse error, subject to the one-per-declaration rule.
    pub fn report_error(&mut self, error: ParserError) {
        if self.errors.len() > self.decl_error_floor {
            return;
        }
        self.errors.push(error);
    }

    pub fn collect_error(&mut self, error_msg: &str) {
        let location = self.current_source_location();
        let error = ParserError::unexpected_token(location, error_msg.to_string());
        self.report_error(error);
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
        self.string_interner
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

#[cfg(test)]
mod offset_to_line_col_tests {
    use super::*;

    /// FRONTEND-PERF (a): pin the binary-search `offset_to_line_col`
    /// against the linear walk it replaced. Columns are counted in
    /// characters, not bytes — a multi-byte char before the offset
    /// advances the column by one (offset 6 in the input below is
    /// `c`, which follows the 3-byte `雪`), and lines are 1-based with
    /// the first line starting at offset 0.
    #[test]
    fn offset_to_line_col_counts_characters_and_lines() {
        let mut interner = DefaultStringInterner::new();
        // a b \n 雪(3 bytes) c \n
        let input = "ab\n\u{96ea}c\n";
        let parser = Parser::new(input, &mut interner);
        assert_eq!(parser.offset_to_line_col(0), (1, 1));
        assert_eq!(parser.offset_to_line_col(1), (1, 2));
        assert_eq!(parser.offset_to_line_col(2), (1, 3));
        assert_eq!(parser.offset_to_line_col(3), (2, 1)); // `雪` first byte
        assert_eq!(parser.offset_to_line_col(6), (2, 2)); // `c` — one char after `雪`
        assert_eq!(parser.offset_to_line_col(7), (2, 3)); // `\n` on line 2
        assert_eq!(parser.offset_to_line_col(8), (3, 1)); // EOF after trailing `\n`
    }

    #[test]
    fn offset_past_end_clamps_to_input_len() {
        let mut interner = DefaultStringInterner::new();
        let input = "hi";
        let parser = Parser::new(input, &mut interner);
        assert_eq!(parser.offset_to_line_col(99), (1, 3)); // clamps to len 2 -> column 3
    }
}
