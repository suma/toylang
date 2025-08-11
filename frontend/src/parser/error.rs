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

        let mut result = base_message;

        let location = &self.location;
        result = format!("{}:{}:{}: {}", location.line, location.column, location.offset, result);

        write!(f, "{}", result)
    }
}

impl std::error::Error for ParserError {}

// Type alias for parser results
pub type ParserResult<T> = std::result::Result<T, ParserError>;
