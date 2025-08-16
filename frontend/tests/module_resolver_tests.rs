use frontend::{ModuleResolver, ParserWithInterner};
use frontend::ast::{ImportDecl, Program};
use string_interner::DefaultStringInterner;
use std::path::PathBuf;
use std::fs;
use tempfile::TempDir;

#[cfg(test)]
mod module_resolver_tests {
    use super::*;

    fn create_test_module(dir: &TempDir, path: &str, content: &str) -> PathBuf {
        let file_path = dir.path().join(path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&file_path, content).unwrap();
        file_path
    }

    fn sync_import_with_parser(import: &ImportDecl, string_interner: &DefaultStringInterner) -> ImportDecl {
        // Since we're now using the same string_interner for both program and parser,
        // we can just return the import as-is
        import.clone()
    }

    #[test]
    fn test_simple_file_module_resolution() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create math.t module
        create_test_module(&temp_dir, "math.t", r#"
package math

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}
"#);
        
        // Create main program that imports math
        let main_content = r#"
import math

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        // Sync ImportDecl symbols with parser's string_interner
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        
        // Test import resolution
        let resolved = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner).unwrap();
        
        assert_eq!(resolved.package_name.len(), 1);
        assert!(resolved.file_path.ends_with("math.t"));
        assert!(resolved.program.package_decl.is_some());
    }

    #[test]
    fn test_nested_module_resolution() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create math/basic.t module
        create_test_module(&temp_dir, "math/basic.t", r#"
package math.basic

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b
}
"#);
        
        let main_content = r#"
import math.basic

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        let resolved = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner).unwrap();
        
        assert_eq!(resolved.package_name.len(), 2);
        assert!(resolved.file_path.ends_with("math/basic.t"));
    }

    #[test]
    fn test_directory_module_with_mod_t() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create math/mod.t
        create_test_module(&temp_dir, "math/mod.t", r#"
package math

pub fn version() -> str {
    "1.0.0"
}
"#);
        
        let main_content = r#"
import math

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        let resolved = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner).unwrap();
        
        assert_eq!(resolved.package_name.len(), 1);
        assert!(resolved.file_path.ends_with("math/mod.t"));
    }

    #[test]
    fn test_module_not_found() {
        let temp_dir = TempDir::new().unwrap();
        
        let main_content = r#"
import nonexistent

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        let result = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner);
        
        assert!(result.is_err());
        let error_msg = format!("{}", result.unwrap_err());
        assert!(error_msg.contains("Module 'nonexistent' not found"));
    }

    #[test]
    fn test_package_declaration_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create module with wrong package declaration
        create_test_module(&temp_dir, "math.t", r#"
package wrongname

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}
"#);
        
        let main_content = r#"
import math

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        let result = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner);
        
        assert!(result.is_err());
        let error_msg = format!("{}", result.unwrap_err());
        assert!(error_msg.contains("Package declaration mismatch"));
    }

    #[test]
    fn test_module_caching() {
        let temp_dir = TempDir::new().unwrap();
        
        create_test_module(&temp_dir, "math.t", r#"
package math

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}
"#);
        
        let main_content = r#"
import math

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        let mut resolver = ModuleResolver::with_search_paths(vec![temp_dir.path().to_path_buf()]);
        
        let import = &program.imports[0];
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(import, string_interner);
        
        // First resolution
        let resolved1 = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner).unwrap();
        
        // Second resolution should use cache
        let resolved2 = resolver.resolve_import(&synced_import, Some(temp_dir.path()), string_interner).unwrap();
        
        // Should be the same (cached)
        assert_eq!(resolved1.file_path, resolved2.file_path);
        assert_eq!(resolver.get_loaded_modules().len(), 1);
    }

    #[test]
    fn test_search_path_priority() {
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();
        
        // Create math.t in both directories
        create_test_module(&temp_dir1, "math.t", r#"
package math

pub fn add(a: u64, b: u64) -> u64 {
    a + b  # First directory
}
"#);
        
        create_test_module(&temp_dir2, "math.t", r#"
package math

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b  # Second directory
}
"#);
        
        let main_content = r#"
import math

fn main() -> u64 {
    42u64
}
"#;
        
        let mut parser = ParserWithInterner::new(main_content);
        let program = parser.parse_program().unwrap();
        
        // Set search paths with temp_dir1 first
        let mut resolver = ModuleResolver::with_search_paths(vec![
            temp_dir1.path().to_path_buf(),
            temp_dir2.path().to_path_buf()
        ]);
        
        let string_interner = parser.get_string_interner();
        let synced_import = sync_import_with_parser(&program.imports[0], string_interner);
        let resolved = resolver.resolve_import(&synced_import, None, string_interner).unwrap();
        
        // Should resolve to first directory
        assert!(resolved.file_path.starts_with(temp_dir1.path()));
    }
}