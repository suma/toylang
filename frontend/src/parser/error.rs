use crate::parser::error::ParserErrorKind::UnexpectedToken;
use crate::type_checker::SourceLocation;

#[derive(Debug)]
pub enum ParserErrorKind {
    UnexpectedToken { expected: String },
}
#[derive(Debug)]
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
