use string_interner::DefaultStringInterner;
use frontend::{ModuleResolver, Parser};
use frontend::ast::Program;
use frontend::parser::error::ParserResult;
use std::path::Path;

/// Compiler session that serves as the central context for compilation
/// 
/// This structure holds all shared compiler state and resources that need to be
/// accessible across different compilation phases (parsing, type checking, code generation).
/// It provides a unified interface for managing compiler-wide resources such as
/// string interning, module resolution, and other compilation context.
pub struct CompilerSession {
    string_interner: DefaultStringInterner,
    module_resolver: ModuleResolver,
}

impl CompilerSession {
    /// Create a new compiler session with default configuration
    pub fn new() -> Self {
        Self {
            string_interner: DefaultStringInterner::new(),
            module_resolver: ModuleResolver::new(),
        }
    }
    
    /// Create a new compiler session with custom search paths for module resolution
    pub fn with_search_paths(search_paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            string_interner: DefaultStringInterner::new(),
            module_resolver: ModuleResolver::with_search_paths(search_paths),
        }
    }
    
    /// Parse a program string within the compiler session context
    /// 
    /// Uses the session's shared resources (string interner, module resolver, etc.)
    /// to parse the input and produce an AST.
    pub fn parse_program(&mut self, input: &str) -> ParserResult<Program> {
        let mut parser = Parser::new(input, &mut self.string_interner);
        let program = parser.parse_program()?;
        
        Ok(program)
    }
    
    /// Merge symbols from another string interner into the session's interner
    /// 
    /// This is useful when integrating separately parsed modules or when
    /// combining ASTs from different parsing contexts into a single compilation unit.
    pub fn merge_string_interner(&mut self, other: &mut DefaultStringInterner) {
        // Merge all symbols from other interner into session's interner
        for (_symbol, string) in other.iter() {
            self.string_interner.get_or_intern(string);
        }
        
        // Replace the other interner with our session's interner to ensure consistency
        *other = self.string_interner.clone();
    }
    
    /// Parse a module file using the session's string interner
    pub fn parse_module_file<P: AsRef<Path>>(&mut self, file_path: P) -> ParserResult<Program> {
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| frontend::parser::error::ParserError::io_error(
                frontend::type_checker::SourceLocation { line: 0, column: 0, offset: 0 },
                format!("Failed to read file: {}", e)
            ))?;
        
        self.parse_program(&content)
    }
    
    /// Get an immutable reference to the string interner
    pub fn string_interner(&self) -> &DefaultStringInterner {
        &self.string_interner
    }
    
    /// Get a mutable reference to the string interner
    pub fn string_interner_mut(&mut self) -> &mut DefaultStringInterner {
        &mut self.string_interner
    }
    
    /// Get an immutable reference to the module resolver
    pub fn module_resolver(&self) -> &ModuleResolver {
        &self.module_resolver
    }
    
    /// Get a mutable reference to the module resolver
    pub fn module_resolver_mut(&mut self) -> &mut ModuleResolver {
        &mut self.module_resolver
    }
}

impl Default for CompilerSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use string_interner::Symbol;

    #[test]
    fn test_session_creation() {
        let session = CompilerSession::new();
        // Test that a non-existent symbol returns None
        let non_existent_symbol = string_interner::DefaultSymbol::try_from_usize(999).unwrap();
        assert!(session.string_interner().resolve(non_existent_symbol).is_none());
    }

    #[test]
    fn test_parse_simple_program() {
        let mut session = CompilerSession::new();
        let program = session.parse_program("fn main() -> u64 { 42u64 }").unwrap();
        assert_eq!(program.function.len(), 1);
    }
    
    #[test]
    fn test_string_interner_consistency() {
        let mut session = CompilerSession::new();
        
        // Parse a program that will intern some symbols
        let program = session.parse_program("fn test() -> u64 { 123u64 }").unwrap();
        
        // Check that the function name was interned
        assert_eq!(program.function.len(), 1);
        let function_name = program.function[0].name;
        
        // Debug: print what symbols we have
        println!("Function name symbol: {:?}", function_name);
        println!("String interner has {} symbols", session.string_interner().len());
        
        // The session should be able to resolve this symbol
        let resolved_name = session.string_interner().resolve(function_name);
        println!("Resolved name: {:?}", resolved_name);
        
        assert_eq!(resolved_name, Some("test"));
    }
}
