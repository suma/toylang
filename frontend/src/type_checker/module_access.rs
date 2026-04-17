use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, error};

impl<'a> TypeCheckerVisitor<'a> {
    /// Add error to collection without returning immediately
    pub fn collect_error(&mut self, error: TypeCheckError) {
        self.errors.push(error);
    }

    /// Type check program with multiple error collection
    pub fn check_program_multiple_errors(&mut self, program: &Program) -> error::MultipleTypeCheckResult<()> {
        self.errors.clear();

        // Collect errors during type checking instead of returning immediately
        for func in &program.function {
            if let Err(e) = self.type_check(func.clone()) {
                self.errors.push(e);
            }
        }

        for index in 0..program.statement.len() {
            let stmt_ref = StmtRef(index as u32);
            if let Err(e) = self.visit_stmt(&stmt_ref) {
                self.errors.push(e);
            }
        }

        if self.errors.is_empty() {
            error::MultipleTypeCheckResult::success(())
        } else {
            error::MultipleTypeCheckResult::with_errors((), self.errors.clone())
        }
    }

    /// Clear collected errors
    pub fn clear_errors(&mut self) {
        self.errors.clear();
    }

    // Module management methods (Phase 1: Basic namespace management)

    /// Set the current package context
    pub fn set_current_package(&mut self, package_path: Vec<DefaultSymbol>) {
        self.current_package = Some(package_path);
    }

    /// Get the current package path
    pub fn get_current_package(&self) -> Option<&Vec<DefaultSymbol>> {
        self.current_package.as_ref()
    }

    /// Register an imported module (simple alias -> full_path mapping)
    pub fn register_import(&mut self, module_path: Vec<DefaultSymbol>) {
        // Use the last component as alias (e.g., math.utils -> utils)
        let alias = if let Some(&last) = module_path.last() {
            vec![last]
        } else {
            module_path.clone()
        };
        self.imported_modules.insert(alias, module_path);
    }

    /// Check if a module path is valid for import (not self-referencing)
    pub fn is_valid_import(&self, module_path: &[DefaultSymbol]) -> bool {
        if let Some(current_pkg) = &self.current_package {
            current_pkg != module_path
        } else {
            true
        }
    }

    /// Try to resolve a module qualified name (e.g., math.add)
    /// Returns Some(TypeDecl) if it's a valid module qualified name, None if it's a regular field access
    pub fn try_resolve_module_qualified_name(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<Option<TypeDecl>, TypeCheckError> {
        // Check if obj is an identifier that matches an imported module
        if let Some(obj_expr) = self.core.expr_pool.get(obj) {
            if let Expr::Identifier(module_symbol) = obj_expr {
                let module_alias = vec![module_symbol];

                // Check if this identifier matches an imported module
                if let Some(full_module_path) = self.imported_modules.get(&module_alias) {
                    let module_path_clone = full_module_path.clone();
                    return self.resolve_module_member_type(&module_path_clone, field);
                }
            }
        }

        Ok(None)
    }

    /// Resolve the type of a member in a specific module
    fn resolve_module_member_type(&mut self, module_path: &[DefaultSymbol], member_name: &DefaultSymbol) -> Result<Option<TypeDecl>, TypeCheckError> {
        // Convert member name to string for lookup
        let member_str = self.core.string_interner.resolve(*member_name)
            .ok_or_else(|| TypeCheckError::generic_error("Member name not found in string interner"))?;

        // Simple heuristic: if it's a known function pattern, return a generic function type
        if self.is_likely_function_name(member_str) {
            Ok(Some(TypeDecl::Unknown))
        } else {
            Err(TypeCheckError::generic_error(&format!(
                "Member '{}' not found in module '{:?}'",
                member_str,
                self.resolve_module_path_names(module_path)
            )))
        }
    }

    /// Helper to check if a name looks like a function (simple heuristic)
    fn is_likely_function_name(&self, name: &str) -> bool {
        name.chars().all(|c| c.is_alphanumeric() || c == '_') &&
            !name.chars().next().unwrap_or('0').is_uppercase()
    }

    /// Helper to convert module path symbols to readable names
    fn resolve_module_path_names(&self, module_path: &[DefaultSymbol]) -> Vec<String> {
        module_path.iter()
            .map(|&symbol| self.resolve_symbol_name(symbol))
            .collect()
    }

    // =========================================================================
    // Phase 3: Access Control and Visibility Enforcement
    // =========================================================================

    /// Check if a function can be accessed based on visibility and module context
    pub(super) fn check_function_access(&self, function: &Function) -> Result<(), TypeCheckError> {
        // If function is public, it's accessible from anywhere
        if function.visibility == Visibility::Public {
            return Ok(());
        }

        // If function is private, check if we're in the same module
        if function.visibility == Visibility::Private {
            // For now, assume same-module access is allowed
            if self.is_same_module_access() {
                return Ok(());
            } else {
                let fn_name = self.resolve_symbol_name(function.name);
                return Err(TypeCheckError::access_denied(
                    &format!("Private function '{}' cannot be accessed from different module", fn_name)
                ));
            }
        }

        Ok(())
    }

    /// Check if current access is within the same module
    #[allow(dead_code)]
    fn is_same_module_access(&self) -> bool {
        // For Phase 3 initial implementation, assume same module access
        // TODO: Implement proper module context tracking
        true
    }
}

/// Check if a string is a reserved keyword
pub(super) fn is_reserved_keyword(name: &str) -> bool {
    matches!(name,
        "fn" | "val" | "var" | "if" | "else" | "for" | "in" | "to" |
        "while" | "break" | "continue" | "return" | "struct" | "impl" |
        "package" | "import" | "pub" | "true" | "false" | "u64" | "i64" |
        "bool" | "str" | "self" | "Self"
    )
}
