use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::TypeCheckerVisitor;

/// Builtin functions and methods implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Create builtin method registry
    pub fn create_builtin_method_registry() -> HashMap<(TypeDecl, String), BuiltinMethod> {
        let mut registry = HashMap::new();
        
        // Universal methods (available for all types - we'll handle these specially)
        // is_null is handled separately in visit_method_call
        
        // String methods
        registry.insert((TypeDecl::String, "len".to_string()), BuiltinMethod::StrLen);
        registry.insert((TypeDecl::String, "concat".to_string()), BuiltinMethod::StrConcat);
        registry.insert((TypeDecl::String, "substring".to_string()), BuiltinMethod::StrSubstring);
        registry.insert((TypeDecl::String, "contains".to_string()), BuiltinMethod::StrContains);
        registry.insert((TypeDecl::String, "split".to_string()), BuiltinMethod::StrSplit);
        registry.insert((TypeDecl::String, "trim".to_string()), BuiltinMethod::StrTrim);
        registry.insert((TypeDecl::String, "to_upper".to_string()), BuiltinMethod::StrToUpper);
        registry.insert((TypeDecl::String, "to_lower".to_string()), BuiltinMethod::StrToLower);
        
        // Future: Array methods (when ArrayLen etc. are added)
        // registry.insert((TypeDecl::Array(vec![], 0), "len".to_string()), BuiltinMethod::ArrayLen);
        // Note: For arrays, we'll need special handling since TypeDecl::Array contains element types
        
        registry
    }


    // Builtin method and function processing is handled by method.rs and main type_checker.rs
    // This module provides registry data and is reserved for future builtin-specific functionality
}