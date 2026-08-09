use crate::type_checker::SourceLocation;

#[derive(Debug, Clone)]
pub enum ParserErrorKind {
    UnexpectedToken { expected: String },
    RecursionLimitExceeded,
    GenericError { message: String },
    IoError { message: String },
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
}

impl ParserError {
    pub fn unexpected_token(location: SourceLocation, expected: String) -> Self {
        Self {
            kind: ParserErrorKind::UnexpectedToken { expected },
            location,
        }
    }
    
    pub fn recursion_limit_exceeded(location: SourceLocation) -> Self {
        Self {
            kind: ParserErrorKind::RecursionLimitExceeded,
            location,
        }
    }
    
    pub fn generic_error(location: SourceLocation, message: String) -> Self {
        Self {
            kind: ParserErrorKind::GenericError { message },
            location,
        }
    }
    
    pub fn io_error(location: SourceLocation, message: String) -> Self {
        Self {
            kind: ParserErrorKind::IoError { message },
            location,
        }
    }
}
impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let base_message = match &self.kind {
            ParserErrorKind::UnexpectedToken { expected} => {
                format!("Expected {:?}", expected)
            }
            ParserErrorKind::RecursionLimitExceeded => {
                "Recursion limit exceeded".to_string()
            }
            ParserErrorKind::GenericError { message } => {
                message.clone()
            }
            ParserErrorKind::IoError { message } => {
                format!("IO error: {}", message)
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
