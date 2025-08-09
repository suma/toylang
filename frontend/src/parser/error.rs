use crate::parser::error::ParserErrorKind::UnexpectedToken;
use crate::type_checker::SourceLocation;

#[derive(Debug, Clone)]
pub enum ParserErrorKind {
    UnexpectedToken { expected: String },
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
            kind: UnexpectedToken { expected },
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
        };

        let mut result = base_message;

        let location = &self.location;
        result = format!("{}:{}:{}: {}", location.line, location.column, location.offset, result);

        write!(f, "{}", result)
    }
}
