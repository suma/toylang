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
        
        // `str` methods.
        //
        // STDLIB-TEXT §3: only the ones that answer without a new
        // buffer. `str` is a borrowed handle -- it owns nothing to
        // write into -- so `substring` / `trim` / `to_ascii_upper` /
        // `to_ascii_lower` / `split` belong to `String`, and were
        // removed from here. They had never worked anywhere but the
        // tree-walker: the IR has `StrLen` and `StrConcat` and nothing
        // else, so the compiled lanes refused them at lowering time
        // while the type checker said yes.
        //
        // `contains` left too, in the other direction: it is now an
        // extension impl in `core/std/str.t` on top of one `find`
        // extern, which is how it reaches every backend.
        registry.insert((TypeDecl::String, "len".to_string()), BuiltinMethod::StrLen);
        registry.insert((TypeDecl::String, "concat".to_string()), BuiltinMethod::StrConcat);

        // NOTE: numeric value-method registrations (`i64.abs()` /
        // `f64.abs()` / `f64.sqrt()`) lived here as
        // `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}` entries. Step E
        // moved them onto extension-trait impls in the always-loaded
        // prelude (`impl Abs for i64 { fn abs(self) -> i64 { ... } }`
        // / `impl Abs for f64` / `impl Sqrt for f64`); call sites
        // resolve through `context.struct_methods` keyed by the
        // canonical primitive name (`"i64"` / `"f64"`) instead of
        // through this builtin-method registry.
        
        // Future: Array methods (when ArrayLen etc. are added)
        // registry.insert((TypeDecl::Array(vec![], 0), "len".to_string()), BuiltinMethod::ArrayLen);
        // Note: For arrays, we'll need special handling since TypeDecl::Array contains element types
        
        registry
    }


    // Builtin method and function processing is handled by method.rs and main type_checker.rs
    // This module provides registry data and is reserved for future builtin-specific functionality
}