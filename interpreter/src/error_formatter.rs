use frontend::parser::error::ParserError;
use frontend::diagnostic::Diagnostic;
use frontend::source_map::{FileId, SourceMap};
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
    /// Every file the program was built from (DEBUG-OBS D2).
    ///
    /// `None` for the callers that have one source string and nothing
    /// else — a parse error, a `--api` query — and those cannot
    /// produce a position outside it anyway. With a map, a location
    /// picks its own file, which is what stops a stdlib line number
    /// from being drawn against the user's source (実測 5).
    source_map: Option<&'a SourceMap>,
}

impl<'a> ErrorFormatter<'a> {
    pub fn new(source_code: &'a str, filename: &'a str) -> Self {
        Self {
            source_code,
            filename,
            source_map: None,
        }
    }

    /// A formatter that can draw an excerpt from any file the program
    /// was built from. The `source_code` / `filename` pair stays as
    /// the fallback for locations whose file the map does not know.
    pub fn with_source_map(
        source_code: &'a str,
        filename: &'a str,
        source_map: &'a SourceMap,
    ) -> Self {
        Self {
            source_code,
            filename,
            source_map: Some(source_map),
        }
    }

    /// The name and text a location should be rendered against.
    ///
    /// The constructor's pair *is* the entry file: a driver that hands
    /// the formatter a source and a name is naming the file the user
    /// ran, and it is the caller closest to knowing what to call it.
    /// The map answers for everything else — the imported files, which
    /// no caller could have passed in.
    fn file_for(&self, file: FileId) -> (&str, &str) {
        if file == FileId::ENTRY {
            return (self.filename, self.source_code);
        }
        match self.source_map.and_then(|map| map.get(file)) {
            Some(f) => (f.path.as_str(), f.source.as_str()),
            None => (self.filename, self.source_code),
        }
    }

    pub fn format_parse_error(&self, error: &ParserError) -> String {
        self.format_error_with_location(&error.to_string(), &error.location)
    }

    /// Render a structured diagnostic as the human-readable form.
    ///
    /// LLM-LOOP P3: the text output is a projection of `Diagnostic`, so
    /// what a reader sees and what a tool consumes cannot drift apart.
    pub fn format_diagnostic(&self, diagnostic: &Diagnostic) -> String {
        // COMPILE-TIME-EVAL C4: severity picks the leading word, so a
        // warning does not announce itself as an error.
        let label = match diagnostic.severity {
            frontend::diagnostic::Severity::Warning => "Warning",
            _ => "Error",
        };
        let mut out = if let Some(module) = &diagnostic.origin_module {
            let position = diagnostic
                .span
                .map(|s| format!(" (line {} of that module)", s.line))
                .unwrap_or_default();
            format!(
                "[{}] {label} in imported module `{module}`{position}: {}\n   \
                 = note: this comes from module `{module}`, not from the file being compiled",
                diagnostic.code, diagnostic.message
            )
        } else if let Some(span) = diagnostic.span {
            let location = SourceLocation::new_in(
                span.file,
                span.line,
                span.column,
                span.offset,
                span.end_offset,
            );
            let body = format!("[{}] {}", diagnostic.code, diagnostic.message);
            self.format_labelled_with_location(label, &body, &location)
        } else {
            format!("{label}: [{}] {}", diagnostic.code, diagnostic.message)
        };

        for suggestion in &diagnostic.suggestions {
            out.push_str(&format!(
                "\n   = help: {} — replace with `{}`",
                suggestion.message, suggestion.replacement
            ));
        }
        out
    }

    pub fn format_type_check_error(&self, error: &TypeCheckError) -> String {
        // LLM-LOOP P2: an error raised inside an imported module carries
        // a location into *that* module's source. Quoting the line at
        // that offset from the file being compiled produces a confident
        // pointer at unrelated code -- worse than no location at all,
        // because it sends the reader somewhere specific and wrong. Name
        // the module and drop the snippet.
        if let Some(module) = &error.origin_module {
            let position = error
                .location
                .map(|loc| format!(" (line {} of that module)", loc.line))
                .unwrap_or_default();
            return format!(
                "Error in imported module `{module}`{position}: {error}\n   \
                 = note: this comes from module `{module}`, not from the file being compiled"
            );
        }
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
        self.format_labelled_with_location("Error", error_msg, location)
    }

    /// As [`Self::format_error_with_location`], with the leading word
    /// chosen by the caller. COMPILE-TIME-EVAL C4 added warnings, and
    /// a diagnostic that says "Error" while the program goes on to run
    /// tells the reader the opposite of what happened.
    fn format_labelled_with_location(
        &self,
        label: &str,
        error_msg: &str,
        location: &SourceLocation,
    ) -> String {
        let line_number = location.line;
        let column = location.column;
        let (filename, source_code) = self.file_for(location.file);

        // Get the source line
        let lines: Vec<&str> = source_code.lines().collect();
        let source_line = if (line_number as usize) <= lines.len() && line_number > 0 {
            lines[(line_number as usize) - 1]
        } else {
            "<line not available>"
        };

        // Create line number display
        let line_display = format!("{line_number:2}");

        // LLM-LOOP P2: the caret is derived from the location's span.
        // This used to guess -- it pulled the first single-quoted name
        // out of the message and searched the source line for it, so a
        // message that quoted nothing (most type mismatches) fell back
        // to a fixed two-column marker at the reported column, and a
        // message that quoted a name appearing twice underlined the
        // wrong one.
        let caret = Self::caret_for(source_line, column, location.width());

        format!(
            "{label} at {}:{}:{}:\n   |\n{} | {}\n   | {} {}\n   |",
            filename,
            line_number,
            column,
            line_display,
            source_line,
            caret,
            error_msg
        )
    }

    /// Build the `   ^^^^` marker: `column` (1-based) spaces of padding
    /// followed by `width` carets, clamped so the marker never runs past
    /// the end of the line it annotates.
    fn caret_for(source_line: &str, column: u32, width: usize) -> String {
        if column == 0 {
            return "^".to_string();
        }
        let line_len = source_line.chars().count();
        let start = (column as usize).saturating_sub(1).min(line_len);
        let width = width.min(line_len.saturating_sub(start)).max(1);
        format!("{:pad$}{}", "", "^".repeat(width), pad = start)
    }

    pub fn format_simple_error(&self, error_msg: &str) -> String {
        format!("Error: {error_msg}")
    }

    /// Display multiple parse errors with unified formatting
    pub fn display_parse_errors(&self, errors: &[ParserError]) {
        if errors.is_empty() {
            return;
        }

        // Through the sink-aware helpers so in-process tests can
        // capture parse diagnostics with `output::with_stderr_capture`
        // instead of spawning the binary.
        crate::output::eprintln_text(ErrorType::Parse.header());
        for error in errors {
            let formatted_error = self.format_parse_error(error);
            crate::output::eprintln_text(&format!("{}{}", ErrorType::Parse.prefix(), formatted_error));
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

    /// Display warnings — diagnostics that do not stop the program.
    ///
    /// COMPILE-TIME-EVAL C4 introduced the first ones, and with them
    /// the need for an output path that does not end in a non-zero
    /// exit. Kept separate from the error header so a reader can tell
    /// at a glance whether the program ran.
    pub fn display_warnings(&self, warnings: &[String]) {
        if warnings.is_empty() {
            return;
        }
        eprintln!("Warnings found:");
        for warning in warnings {
            eprintln!("  {warning}");
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
        // `"string"` — the caret should span the whole literal.
        error.location = Some(SourceLocation::new(2, 18, 35, 43));

        let formatted = formatter.format_type_check_error(&error);
        assert!(formatted.contains("Error at test.t:2:18:"));
        assert!(formatted.contains("val x: i64 = \"string\""));
        assert!(formatted.contains("^^^^^^^^"), "caret should match the span width: {formatted}");
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
        
        let location = SourceLocation::new(3, 5, 58, 65);
        
        let formatted = formatter.format_runtime_error("Index out of bounds", Some(&location));
        assert!(formatted.contains("Error at test.t:3:5:"));
        assert!(formatted.contains("a[5u64]"));
        assert!(formatted.contains("Index out of bounds"));
    }
}