use crate::type_checker::SourceLocation;

#[derive(Debug, Clone)]
pub enum ParserErrorKind {
    UnexpectedToken { expected: String },
    RecursionLimitExceeded,
    GenericError { message: String },
    IoError { message: String },
    /// A lexical failure — a literal or character the lexer could not
    /// read. Carries its diagnostic code ([`crate::diagnostic::codes::LEXICAL`])
    /// so `--explain` can answer it like any other code.
    LexError { message: String },
    /// `1.5`: a float literal without its type suffix, which reads as
    /// a tuple index on `1`. `suffix` is the one to add — `f64` unless
    /// the declaration it initialises says `f32`.
    UnsuffixedFloat { written: String, suffix: &'static str },
    /// `else if`, which the language spells `elif`.
    ElseIf,
}

#[derive(Debug)]
pub struct MultipleParserResult<T> {
    pub result: Option<T>,
    pub errors: Vec<ParserError>,
}

impl<T> MultipleParserResult<T> {
    pub fn success(value: T) -> Self {
        Self {
            result: Some(value),
            errors: Vec::new(),
        }
    }

    pub fn failure(errors: Vec<ParserError>) -> Self {
        Self {
            result: None,
            errors,
        }
    }

    pub fn with_errors(value: T, errors: Vec<ParserError>) -> Self {
        Self {
            result: Some(value),
            errors,
        }
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}
#[derive(Debug, Clone)]
pub struct ParserError {
    pub kind: ParserErrorKind,
    pub location: SourceLocation,
    /// Fixes carried into the diagnostic (LLM-TOOLING #1). Empty for
    /// most parse errors: only a mistake whose correction is certain
    /// gets one.
    pub suggestions: Vec<crate::diagnostic::Suggestion>,
    /// Reported through [`crate::parser::core::Parser::report_recovered_error`]:
    /// not a consequence of anything else, so never folded into
    /// another error on the same line.
    pub recovered: bool,
}

impl ParserError {
    pub fn unexpected_token(location: SourceLocation, expected: String) -> Self {
        Self {
            kind: ParserErrorKind::UnexpectedToken { expected },
            location,
            suggestions: Vec::new(),
            recovered: false,
        }
    }
    
    pub fn recursion_limit_exceeded(location: SourceLocation) -> Self {
        Self {
            kind: ParserErrorKind::RecursionLimitExceeded,
            location,
            suggestions: Vec::new(),
            recovered: false,
        }
    }
    
    pub fn generic_error(location: SourceLocation, message: String) -> Self {
        Self {
            kind: ParserErrorKind::GenericError { message },
            location,
            suggestions: Vec::new(),
            recovered: false,
        }
    }
    
    pub fn io_error(location: SourceLocation, message: String) -> Self {
        Self {
            kind: ParserErrorKind::IoError { message },
            location,
            suggestions: Vec::new(),
            recovered: false,
        }
    }

    /// A lexical failure, reported at the offending literal. The
    /// message is rendered with the code (`[E0012]`) so it can be
    /// looked up with `--explain E0012`.
    pub fn lex_error(location: SourceLocation, message: String) -> Self {
        Self {
            kind: ParserErrorKind::LexError { message },
            location,
            suggestions: Vec::new(),
            recovered: false,
        }
    }
}
impl ParserError {
    /// A float literal written without its suffix, with the fix.
    pub fn unsuffixed_float(location: SourceLocation, written: String) -> Self {
        let fixed = format!("{written}f64");
        Self {
            kind: ParserErrorKind::UnsuffixedFloat { written, suffix: "f64" },
            location,
            suggestions: vec![crate::diagnostic::Suggestion::machine_applicable(
                "add the `f64` suffix",
                fixed,
                crate::diagnostic::Span::from(location),
            )],
            recovered: false,
        }
    }

    /// Make an [`ParserErrorKind::UnsuffixedFloat`] ask for `f32`.
    pub fn use_f32_suffix(&mut self) {
        if let ParserErrorKind::UnsuffixedFloat { written, suffix } = &mut self.kind {
            *suffix = "f32";
            let fixed = format!("{written}f32");
            self.suggestions = vec![crate::diagnostic::Suggestion::machine_applicable(
                "add the `f32` suffix",
                fixed,
                crate::diagnostic::Span::from(self.location),
            )];
        }
    }

    /// `else if` at `location`, with the fix.
    pub fn else_if(location: SourceLocation) -> Self {
        Self {
            kind: ParserErrorKind::ElseIf,
            location,
            suggestions: vec![crate::diagnostic::Suggestion::machine_applicable(
                "use `elif`",
                "elif".to_string(),
                crate::diagnostic::Span::from(location),
            )],
            recovered: false,
        }
    }

    /// The same error, carrying a fix.
    pub fn with_suggestion(mut self, suggestion: crate::diagnostic::Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let base_message = match &self.kind {
            // The payload is a message, not a token name — most call
            // sites pass a sentence ("unexpected token in primary
            // expression: ..."). `format!("Expected {:?}", ..)` wrapped
            // that in a second, contradictory sentence *and* quoted it,
            // so the reader got `Expected "unexpected token ..."`.
            ParserErrorKind::UnexpectedToken { expected } => expected.clone(),
            ParserErrorKind::RecursionLimitExceeded => {
                "Recursion limit exceeded".to_string()
            }
            ParserErrorKind::GenericError { message } => {
                message.clone()
            }
            ParserErrorKind::IoError { message } => {
                format!("IO error: {}", message)
            }
            ParserErrorKind::ElseIf => {
                "`else if` is not supported; write `elif` instead (`} elif cond {`)".to_string()
            }
            ParserErrorKind::UnsuffixedFloat { written, suffix } => format!(
                "a float literal needs a type suffix: write `{written}{suffix}` \
                 (a bare `{written}` would read as a tuple index)"
            ),
            ParserErrorKind::LexError { message } => {
                // The code travels in the message so every rendering —
                // the interpreter's formatter, `{:?}` in test helpers,
                // JSON `message` fields — carries the lookup key.
                format!("[{}] {}", crate::diagnostic::codes::LEXICAL, message)
            }
        };

        // LLM-LOOP P2: the message is the message. The formatter already
        // prints `Error at <file>:<line>:<col>` above it, so prefixing
        // `line:column:offset` here duplicated the position and leaked
        // `offset`, a byte index into the source that means nothing to a
        // reader. `TypeCheckError` was cleaned up in P2; this is its
        // parser-side twin. Callers that need a position read
        // `self.location`.
        write!(f, "{}", base_message)
    }
}

impl std::error::Error for ParserError {}

// Type alias for parser results
pub type ParserResult<T> = std::result::Result<T, ParserError>;
