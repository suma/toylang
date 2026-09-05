use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, error};

impl<'a> TypeCheckerVisitor<'a> {
    /// Add error to collection without returning immediately
    pub fn collect_error(&mut self, error: TypeCheckError) {
        self.errors.push(error);
    }

    /// Type check program with multiple error collection.
    ///
    /// LLM-LOOP P1: recovery is statement-level. Before, the loops below
    /// were the only recovery point, so a function that failed reported
    /// exactly one error no matter how many its body contained and the
    /// caller had to re-run after every single fix. With
    /// `recovery_enabled` set, a failing statement is recorded and
    /// checking continues with the next one, so one run reports every
    /// independent problem in the program.
    ///
    /// The loops still catch errors themselves: recovery covers
    /// statements inside a body, not failures raised around it (a
    /// reference-typed return position, a malformed body, a `requires`
    /// clause that isn't bool).
    pub fn check_program_multiple_errors(&mut self, program: &File) -> error::MultipleTypeCheckResult<()> {
        self.errors.clear();
        let prev_recovery = std::mem::replace(&mut self.recovery_enabled, true);

        // RECURSIVE-TYPES: a type containing itself by value has no
        // finite layout, and lowering one used to abort the process.
        // Reported before anything else so the cause comes ahead of the
        // cascade it produces at each use site.
        let recursive =
            crate::type_checker::check_recursive_types(program, self.core.string_interner);
        self.errors.extend(recursive);

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

        self.recovery_enabled = prev_recovery;

        // NEWTYPE: install the tuple-struct desugar's pool rewrites now
        // that every body has been checked.
        self.apply_tuple_struct_rewrites();

        // NULL-COALESCE: replace the `a ?? b` nodes that surfaced
        // through direct `accept_expr` dispatch (and were typed but not
        // rewritten) with their lazy `val` + `match` blocks.
        self.apply_null_coalesce_rewrites();
        // STDLIB-ORD: `a < b` on two `str`s becomes the `Ord` call that
        // implements it, now that every operand type is recorded.
        self.apply_str_ordering_rewrites();

        // COLLECTIONS C0(a): every body and every call site has been
        // seen, so the recorded `==`-on-a-type-parameter requirements
        // can finally be matched against the types they were
        // instantiated with.
        self.report_missing_equality_impls();

        // Report in source order. Functions are checked in declaration
        // order but a call site can pull a callee's body forward
        // (`type_check_forward_ref`), so collection order doesn't match
        // the file. Errors without a location keep their relative order
        // and sort last -- there is nothing to place them by.
        self.errors.sort_by_key(|e| {
            e.location
                .map(|loc| (0u8, loc.line, loc.column))
                .unwrap_or((1, 0, 0))
        });

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
        if let Some(obj_expr) = self.core.expr_pool.get(obj)
            && let Expr::Identifier(module_symbol) = obj_expr {
                let module_alias = vec![module_symbol];

                // Check if this identifier matches an imported module
                if let Some(full_module_path) = self.imported_modules.get(&module_alias) {
                    let module_path_clone = full_module_path.clone();
                    return self.resolve_module_member_type(&module_path_clone, field);
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
                "Member '{}' not found in module '{}'",
                member_str,
                self.resolve_module_path_names(module_path).join("::")
            )))
        }
    }

    /// Helper to check if a name looks like a function (simple heuristic)
    fn is_likely_function_name(&self, name: &str) -> bool {
        name.chars().all(|c| c.is_alphanumeric() || c == '_') &&
            !name.chars().next().unwrap_or('0').is_uppercase()
    }

    /// A module path as a program would write it (`std::math`).
    pub(super) fn resolve_module_path(&self, module_path: &[DefaultSymbol]) -> String {
        self.resolve_module_path_names(module_path).join("::")
    }

    /// MODULE-SYSTEM P2: the qualifier matched more than one module.
    /// Naming the competing paths is the whole point — the reader's
    /// fix is to write enough leading segments to tell them apart.
    pub(super) fn ambiguous_module_function_error(
        &self,
        qualifier: Option<&[DefaultSymbol]>,
        function_name: DefaultSymbol,
        candidates: &[std::rc::Rc<[DefaultSymbol]>],
    ) -> TypeCheckError {
        let name = self.resolve_symbol_name(function_name);
        let written = match qualifier {
            Some(segments) => format!("{}::{}", self.resolve_module_path(segments), name),
            None => name.clone(),
        };
        let mut paths: Vec<String> = candidates
            .iter()
            .map(|p| format!("{}::{}", self.resolve_module_path(p), name))
            .collect();
        paths.sort();
        // The advice depends on *why* it is ambiguous, and the two
        // reasons had one message between them. A bare call matching
        // several modules is not a file-name clash -- `std::base64`
        // and `std::hex` have different file names and both export
        // `encode` -- so telling the reader to rename a file sends
        // them somewhere there is nothing to fix.
        match qualifier {
            None => TypeCheckError::generic_error(&format!(
                "ambiguous call `{written}`: {} both define it. \
                 Qualify the call with the module you mean, or move \
                 your own definition into a module root that comes \
                 after them (a later `--core-modules` root wins)",
                paths.join(" and ")
            )),
            // A qualifier that still matches several modules *is* the
            // file-name clash: the parser keeps only the last segment
            // today, so two modules whose paths end the same way are
            // indistinguishable until MODULE-SYSTEM P3 keeps more.
            Some(_) => TypeCheckError::generic_error(&format!(
                "ambiguous module path `{written}`: it matches {}. Two modules \
                 cannot share a file name — rename one of them (writing more \
                 leading segments will be the other way out once multi-segment \
                 paths are checked)",
                paths.join(" and ")
            )),
        }
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
