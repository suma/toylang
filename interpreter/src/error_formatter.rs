use frontend::parser::error::ParserError;
use frontend::type_checker::{SourceLocation, TypeCheckError};

/// Enum for different types of errors that can occur
#[derive(Debug)]
pub enum ErrorType {
    Parse,
    TypeCheck,
    Runtime,
}

impl ErrorType {
    /// Get the header message for this error type
    pub fn header(&self) -> &'static str {
        match self {
            ErrorType::Parse => "Parse errors found:",
            ErrorType::TypeCheck => "Type check errors found:",
            ErrorType::Runtime => "Runtime error occurred:",
        }
    }

    /// Get the prefix for individual error messages
    pub fn prefix(&self) -> &'static str {
        match self {
            ErrorType::Parse => "  ",
            ErrorType::TypeCheck => "  ", 
            ErrorType::Runtime => "",
        }
    }
}

pub struct ErrorFormatter<'a> {
    source_code: &'a str,
    filename: &'a str,
}

impl<'a> ErrorFormatter<'a> {
    pub fn new(source_code: &'a str, filename: &'a str) -> Self {
        Self {
            source_code,
            filename,
        }
    }

    pub fn format_parse_error(&self, error: &ParserError) -> String {
        self.format_error_with_location(&error.to_string(), &error.location)
    }

    pub fn format_type_check_error(&self, error: &TypeCheckError) -> String {
        if let Some(location) = &error.location {
            self.format_error_with_location(&error.to_string(), location)
        } else {
            format!("Error: {error}")
        }
    }

    pub fn format_runtime_error(&self, error_msg: &str, location: Option<&SourceLocation>) -> String {
        if let Some(loc) = location {
            self.format_error_with_location(error_msg, loc)
        } else {
            error_msg.to_string()  // Don't add "Runtime Error:" prefix here since it's handled by display method
        }
    }

    fn format_error_with_location(&self, error_msg: &str, location: &SourceLocation) -> String {
        let line_number = location.line;
        let column = location.column;
        
        // Get the source line
        let lines: Vec<&str> = self.source_code.lines().collect();
        let source_line = if (line_number as usize) <= lines.len() && line_number > 0 {
            lines[(line_number as usize) - 1]
        } else {
            "<line not available>"
        };
        
        // Create line number display
        let line_display = format!("{line_number:2}");
        
        // Create the caret indicator
        let caret = if column > 0 {
            // Try to extract identifier from error message and find its position
            let actual_position = self.find_error_position_in_line(error_msg, source_line)
                .unwrap_or_else(|| {
                    // Fallback to the reported column, adjusted
                    if (column as usize) > source_line.len() {
                        source_line.len().saturating_sub(1)
                    } else {
                        (column as usize).saturating_sub(1)
                    }
                });
            format!("{:width$}^^", "", width = actual_position)
        } else {
            "^".to_string()
        };
        
        format!(
            "Error at {}:{}:{}:\n   |\n{} | {}\n   | {} {}\n   |",
            self.filename,
            line_number,
            column,
            line_display,
            source_line,
            caret,
            error_msg
        )
    }

    pub fn format_simple_error(&self, error_msg: &str) -> String {
        format!("Error: {error_msg}")
    }

    /// Display multiple parse errors with unified formatting
    pub fn display_parse_errors(&self, errors: &[ParserError]) {
        if errors.is_empty() {
            return;
        }

        eprintln!("{}", ErrorType::Parse.header());
        for error in errors {
            let formatted_error = self.format_parse_error(error);
            eprintln!("{}{}", ErrorType::Parse.prefix(), formatted_error);
        }
    }

    /// Display multiple type check errors with unified formatting
    pub fn display_type_check_errors(&self, errors: &[String]) {
        if errors.is_empty() {
            return;
        }

        eprintln!("{}", ErrorType::TypeCheck.header());
        for error in errors {
            eprintln!("{}{}", ErrorType::TypeCheck.prefix(), error);
        }
    }

    /// Display a single runtime error with unified formatting
    pub fn display_runtime_error(&self, error: &str) {
        eprintln!("{}", ErrorType::Runtime.header());
        
        // Remove "Runtime Error:" prefix if it already exists to avoid duplication
        let clean_error = error.strip_prefix("Runtime Error: ").unwrap_or(error);
        let formatted_error = self.format_runtime_error(clean_error, None);
        eprintln!("{}{}", ErrorType::Runtime.prefix(), formatted_error);
    }

    /// Generic method to display any collection of errors with unified formatting
    pub fn display_errors<T: std::fmt::Display>(&self, error_type: ErrorType, errors: &[T]) {
        if errors.is_empty() {
            return;
        }

        eprintln!("{}", error_type.header());
        for error in errors {
            eprintln!("{}{}", error_type.prefix(), error);
        }
    }

    fn find_error_position_in_line(&self, error_msg: &str, source_line: &str) -> Option<usize> {
        // Extract identifier from error messages like "Identifier 'undefined_variable' not found"
        if let Some(start) = error_msg.find("'") {
            if let Some(end) = error_msg[start + 1..].find("'") {
                let identifier = &error_msg[start + 1..start + 1 + end];
                return source_line.find(identifier);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontend::type_checker::{SourceLocation, TypeCheckError};

    #[test]
    fn test_error_formatter_with_location() {
        let source = "fn main() -> i64 {\n    val x: i64 = \"string\"\n    x\n}";
        let formatter = ErrorFormatter::new(source, "test.t");
        
        let mut error = TypeCheckError::type_mismatch(
            frontend::type_decl::TypeDecl::Int64,
            frontend::type_decl::TypeDecl::String
        );
        error.location = Some(SourceLocation {
            line: 2,
            column: 18,
            offset: 35,
        });
        
        let formatted = formatter.format_type_check_error(&error);
        assert!(formatted.contains("Error at test.t:2:18:"));
        assert!(formatted.contains("val x: i64 = \"string\""));
        assert!(formatted.contains("^^"));
    }

    #[test] 
    fn test_error_formatter_without_location() {
        let source = "fn main() -> i64 { 42i64 }";
        let formatter = ErrorFormatter::new(source, "test.t");
        
        let error = TypeCheckError::generic_error("Generic error message");
        let formatted = formatter.format_type_check_error(&error);
        assert_eq!(formatted, "Error: Generic error message");
    }

    #[test]
    fn test_runtime_error_formatting() {
        let source = "fn main() -> u64 {\n    val a: [u64; 2] = [1u64, 2u64]\n    a[5u64]\n}";
        let formatter = ErrorFormatter::new(source, "test.t");
        
        let location = SourceLocation {
            line: 3,
            column: 5,
            offset: 58,
        };
        
        let formatted = formatter.format_runtime_error("Index out of bounds", Some(&location));
        assert!(formatted.contains("Error at test.t:3:5:"));
        assert!(formatted.contains("a[5u64]"));
        assert!(formatted.contains("Index out of bounds"));
    }
}