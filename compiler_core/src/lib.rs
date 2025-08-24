use string_interner::DefaultStringInterner;
use frontend::{ModuleResolver, Parser};
use frontend::ast::Program;
use frontend::parser::error::ParserResult;
use frontend::type_checker::{TypeCheckerVisitor, TypeCheckError};
use std::path::Path;
use std::collections::HashMap;

/// Compiler session that serves as the central context for compilation
/// 
/// This structure holds all shared compiler state and resources that need to be
/// accessible across different compilation phases (parsing, type checking, code generation).
/// It provides a unified interface for managing compiler-wide resources such as
/// string interning, module resolution, and other compilation context.
pub struct CompilerSession {
    string_interner: DefaultStringInterner,
    module_resolver: ModuleResolver,
    // Type checking results - stored after type checking is performed
    type_check_results: Option<TypeCheckResults>,
}

/// Results from type checking that can be used by code generators
pub struct TypeCheckResults {
    pub expr_types: HashMap<frontend::ast::ExprRef, frontend::type_decl::TypeDecl>,
    pub struct_types: HashMap<string_interner::DefaultSymbol, String>, // variable -> struct type name
}

impl CompilerSession {
    /// Create a new compiler session with default configuration
    pub fn new() -> Self {
        Self {
            string_interner: DefaultStringInterner::new(),
            module_resolver: ModuleResolver::new(),
            type_check_results: None,
        }
    }
    
    /// Create a new compiler session with custom search paths for module resolution
    pub fn with_search_paths(search_paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            string_interner: DefaultStringInterner::new(),
            module_resolver: ModuleResolver::with_search_paths(search_paths),
            type_check_results: None,
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
    
    /// Type check a program and store the results in the session
    pub fn type_check_program(&mut self, program: &Program) -> Result<(), Vec<TypeCheckError>> {
        use frontend::visitor::ProgramVisitor;
        
        // Create a mutable copy of expression pool for type checking
        let mut expr_pool = program.expression.clone();
        let mut type_checker = TypeCheckerVisitor::new(
            &program.statement, 
            &mut expr_pool,
            &self.string_interner, 
            &program.location_pool
        );
        
        // Run type checking
        match type_checker.visit_program(program) {
            Ok(_) => {
                // Extract useful type information for code generation
                let expr_types = type_checker.get_expr_types();
                let struct_types = type_checker.get_struct_var_mappings(&self.string_interner);
                
                self.type_check_results = Some(TypeCheckResults {
                    expr_types,
                    struct_types,
                });
                
                Ok(())
            }
            Err(error) => Err(vec![error])
        }
    }
    
    /// Get type check results if available
    pub fn type_check_results(&self) -> Option<&TypeCheckResults> {
        self.type_check_results.as_ref()
    }
    
    /// Parse and type check a program in one step
    pub fn parse_and_type_check_program(&mut self, input: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let program = self.parse_program(input)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            
        self.type_check_program(&program)
            .map_err(|errors| {
                let error_msg = errors.into_iter()
                    .map(|e| format!("{}", e))
                    .collect::<Vec<_>>()
                    .join("; ");
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error_msg)) as Box<dyn std::error::Error>
            })?;
            
        Ok(program)
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
