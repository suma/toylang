use crate::ast::{ExprRef, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::{AcceptableExpr, TypeCheckerVisitor, TypeCheckError};

/// Error-reporting helpers for `TypeCheckerVisitor`.
impl<'a> TypeCheckerVisitor<'a> {
    /// Add location information to an error if available.
    pub fn error_with_location(
        &self,
        mut error: TypeCheckError,
        expr: &ExprRef,
    ) -> TypeCheckError {
        if error.location.is_none()
            && let Some(location) = self.get_expr_location(expr) {
                error = error.with_location(location);
            }
        error
    }

    /// Best available position for a method-level diagnostic.
    ///
    /// LLM-LOOP P2: `MethodFunction::node` is not filled in with the
    /// method's own span (it reports the enclosing declaration's start),
    /// so anchoring on it underlines the wrong construct. The body
    /// statement does have a recorded location, and for a return-type
    /// complaint the body is the right thing to point at anyway.
    pub fn method_body_location(
        &self,
        method: &std::rc::Rc<crate::ast::MethodFunction>,
    ) -> crate::type_checker::SourceLocation {
        self.get_stmt_location(&method.code)
            .unwrap_or_else(|| self.node_to_source_location(&method.node))
    }

    /// Type check `expr_ref`, stamping the expression's own location
    /// onto any error that doesn't already carry one.
    ///
    /// LLM-LOOP P2: `visit_expr` does this stamping, but plenty of
    /// call sites reach an expression through `accept_expr` directly
    /// (statement bodies, loop conditions, contract clauses, impl-block
    /// method bodies). Errors raised under those escaped with no
    /// location at all and were reported without so much as a line
    /// number. This is the wrapper those sites use instead.
    ///
    /// Deliberately *not* routed through `visit_expr`: that one also
    /// consults the type cache and rewrites `Expr::Try`, neither of
    /// which every caller wants.
    pub fn check_expr_located(&mut self, expr_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_obj = self.core.expr_pool.get(expr_ref)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        match expr_obj.clone().accept_expr(self) {
            Ok(ty) => Ok(ty),
            Err(e) => Err(self.error_with_location(e, expr_ref)),
        }
    }

    /// LLM-LOOP P1: absorb a statement-level error so the enclosing
    /// statement loop can carry on with the next statement instead of
    /// unwinding out of the function. Only reached when
    /// `recovery_enabled` is set.
    ///
    /// Besides recording the error this repairs just enough context for
    /// the rest of the block to be checked usefully: a `val` / `var`
    /// that failed never registered its binding, so every later mention
    /// of that name would otherwise report "variable not found" — a
    /// cascade about a name the user did in fact declare. Binding it to
    /// `Unknown` keeps those uses quiet (`Unknown` is already the
    /// checker's "don't report against this" type) while still letting
    /// genuinely unrelated errors surface.
    pub fn recover_stmt_error(&mut self, stmt: &StmtRef, mut error: TypeCheckError) {
        if error.location.is_none()
            && let Some(location) = self.get_stmt_location(stmt) {
                error = error.with_location(location);
            }

        match self.core.stmt_pool.get(stmt) {
            Some(Stmt::Val(name, _, _)) if self.context.get_var(name).is_none() => {
                self.context.set_var(name, TypeDecl::Unknown);
            }
            Some(Stmt::Var(name, _, _)) if self.context.get_var(name).is_none() => {
                // `var` is mutable; registering it as such keeps a later
                // assignment from also reporting an immutable-binding error.
                self.context.set_mutable_var(name, TypeDecl::Unknown);
            }
            _ => {}
        }

        self.errors.push(error);
    }

    /// Get human-readable type name for error messages.
    pub fn type_name_for_error(&self, type_decl: &TypeDecl) -> String {
        match type_decl {
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::String => "string".to_string(),
            TypeDecl::Number => "number".to_string(),
            TypeDecl::Unit => "unit".to_string(),
            TypeDecl::Unknown => "unknown".to_string(),
            TypeDecl::Array(element_types, size) => {
                if element_types.len() == 1 {
                    format!(
                        "[{}; {}]",
                        self.type_name_for_error(&element_types[0]),
                        size
                    )
                } else {
                    format!("[{:?}; {}]", element_types, size)
                }
            }
            TypeDecl::Struct(name, _) => self
                .core
                .string_interner
                .resolve(*name)
                .unwrap_or("struct")
                .to_string(),
            TypeDecl::Dict(key_type, value_type) => {
                format!(
                    "dict<{}, {}>",
                    self.type_name_for_error(key_type),
                    self.type_name_for_error(value_type)
                )
            }
            _ => format!("{:?}", type_decl).to_lowercase(),
        }
    }
}
