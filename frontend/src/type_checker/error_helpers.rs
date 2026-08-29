use crate::ast::{Expr, ExprRef, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::{AcceptableExpr, TypeCheckerVisitor, TypeCheckError};

/// Whether a cast suggestion has to parenthesise what it quotes.
#[derive(Clone, Copy)]
enum CastForm {
    Bare,
    Parenthesised,
}

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

    /// Answer a `val x: _ = expr` type hole (LLM-LOOP P7).
    ///
    /// The caller registers the binding with the inferred type *before*
    /// calling this. A hole is a question, and failing the statement
    /// would poison every later use of `x` — the answer would then
    /// arrive buried under a cascade of "unknown type" noise it caused
    /// itself, which is the opposite of the point.
    ///
    /// Returns `Some(err)` only when recovery is off (the fail-fast
    /// `type_check` entry point). The CLI path runs with recovery on and
    /// collects, so every hole in a file is answered in one run.
    pub fn report_type_hole(
        &mut self,
        name: string_interner::DefaultSymbol,
        inferred: &TypeDecl,
        at: &ExprRef,
    ) -> Option<TypeCheckError> {
        let var = self.resolve_symbol_name(name).to_string();
        // An initializer whose type has no surface syntax (an
        // unresolved numeric literal, a range) cannot be spelled back.
        // Say so rather than printing an internal name that does not
        // parse -- the reader's next move is to paste this.
        let spelled = inferred
            .source_name(self.core.string_interner)
            .unwrap_or_else(|| format!("<{}: no source syntax>", self.type_name_for_error(inferred)));
        let error = TypeCheckError::type_hole(var, spelled);
        let error = self.error_with_location(error, at);
        if self.recovery_enabled {
            self.collect_error(error);
            None
        } else {
            Some(error)
        }
    }

    /// NUMBER-HINT: answer the type holes whose initializer was an
    /// unresolved integer literal when the binding was registered.
    ///
    /// Run at the end of the function body, with its scope still
    /// open, so the binding's type is the one finalization settled on
    /// rather than the `Number` placeholder. A hole is a question the
    /// reader will paste the answer to; `<Number: no source syntax>`
    /// answered it with a name that does not parse.
    pub fn answer_pending_number_holes(&mut self) -> Result<(), TypeCheckError> {
        for (name, at) in std::mem::take(&mut self.pending_number_holes) {
            let ty = self
                .resolved_initializer_type(&at)
                // A literal that reached no position naming a type
                // falls back to the same default
                // `finalize_number_types` applies.
                .unwrap_or(TypeDecl::UInt64);
            if let Some(err) = self.report_type_hole(name, &ty, &at) {
                return Err(err);
            }
        }
        Ok(())
    }

    /// NUMBER-HINT: read back the type a hole's initializer settled
    /// on, once the function's literals have been resolved.
    ///
    /// The hole's own binding is no help — it was registered as
    /// `Number` and nothing revisits it — so this reads the
    /// initializer instead: a literal node, which finalization has
    /// rewritten to its concrete width by now, or a name, whose
    /// binding was updated when some later position claimed it.
    /// `None` means nothing ever decided.
    fn resolved_initializer_type(&self, at: &ExprRef) -> Option<TypeDecl> {
        let ty = match self.core.expr_pool.get(at)? {
            Expr::UInt64(_) => TypeDecl::UInt64,
            Expr::Int64(_) => TypeDecl::Int64,
            Expr::UInt8(_) => TypeDecl::UInt8,
            Expr::UInt16(_) => TypeDecl::UInt16,
            Expr::UInt32(_) => TypeDecl::UInt32,
            Expr::Int8(_) => TypeDecl::Int8,
            Expr::Int16(_) => TypeDecl::Int16,
            Expr::Int32(_) => TypeDecl::Int32,
            Expr::Identifier(n) => self.context.get_var(n)?,
            _ => return None,
        };
        (ty != TypeDecl::Number).then_some(ty)
    }

    /// Attach an `as <T>` cast suggestion when the only thing wrong is
    /// the numeric type of `expr`.
    ///
    /// LLM-LOOP P3: this is the single most common fix in toylang --
    /// there is no implicit widening, so a `u64` reaching an `i64` slot
    /// is a mechanical `as i64` away. The suggestion is only produced
    /// when both sides are types an `as` cast accepts, which is exactly
    /// the condition under which applying it is guaranteed to compile.
    pub fn suggest_numeric_cast(
        &self,
        mut error: TypeCheckError,
        expr: &ExprRef,
        actual: &TypeDecl,
        expected: &TypeDecl,
    ) -> TypeCheckError {
        let (Some(_), Some(target)) = (
            crate::diagnostic::castable_type_name(actual),
            crate::diagnostic::castable_type_name(expected),
        ) else {
            return error;
        };
        let Some(location) = self.get_expr_location(expr) else {
            return error;
        };
        let Some(text) = self.source_text(&location) else {
            return error;
        };
        let Some(form) = self.cast_suggestion_form(expr) else {
            return error;
        };
        let text = text.trim();
        let replacement = match form {
            // `as` binds tighter than every binary operator, so casting
            // a compound expression without parentheses casts only its
            // right operand: `a + b as i64` leaves the mismatch in
            // place while looking like a fix.
            CastForm::Parenthesised => format!("({text}) as {target}"),
            CastForm::Bare => format!("{text} as {target}"),
        };
        error.suggestions.push(crate::diagnostic::Suggestion::machine_applicable(
            &format!("cast the value to `{target}`"),
            replacement,
            location.into(),
        ));
        error
    }

    /// How to spell a cast of `expr`, or `None` when no suggestion can
    /// be made for it.
    ///
    /// The gate is **whether the expression's recorded location covers
    /// the expression**. `Expr::Call` is located at its callee, on
    /// purpose, so "function not found" points at the name; quoting
    /// that span and appending ` as i64` gives `g as i64()`, an edit
    /// that does not compile, offered as machine-applicable. Control
    /// forms (`if` / `match` / `with`) are anchored at their keyword
    /// for the same reason a caret cannot usefully span three lines.
    ///
    /// Those get no suggestion. The message already names both types; a
    /// missing suggestion costs a reader nothing, and a wrong one costs
    /// them a round trip plus their trust in the next.
    fn cast_suggestion_form(&self, expr: &ExprRef) -> Option<CastForm> {
        use crate::ast::Expr;
        match self.core.expr_pool.get(expr)? {
            // Single-token values: the span is the token.
            Expr::True
            | Expr::False
            | Expr::Null
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::Float64(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::Identifier(_)
            | Expr::QualifiedIdentifier(_)
            // Postfix forms: the parser spans these from the receiver
            // through the suffix, and postfix binds tighter than `as`,
            // so no parentheses are needed.
            | Expr::FieldAccess(..)
            | Expr::TupleAccess(..)
            | Expr::SliceAccess(..)
            | Expr::MethodCall(..)
            | Expr::Cast(..) => Some(CastForm::Bare),
            // Spanned over the whole operation, but bound more loosely
            // than `as`: `a + b as i64` casts `b`, and `-a as i64`
            // negates the cast rather than casting the negation.
            Expr::Binary(..) | Expr::Unary(..) => Some(CastForm::Parenthesised),
            _ => None,
        }
    }

    /// The source text a location spans, when the checker was given the
    /// source. A replacement has to quote what it replaces, so without
    /// this no suggestion can be built.
    pub fn source_text(&self, location: &crate::type_checker::SourceLocation) -> Option<&'a str> {
        let source = self.source_code?;
        let (start, end) = (location.offset as usize, location.end_offset as usize);
        if end <= start || end > source.len() {
            return None;
        }
        source.get(start..end)
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
        // STRUCT-UPDATE has to be intercepted on this route too: a
        // function's tail expression arrives here, and that is exactly
        // where `P { x: n, ..base }` is most often written.
        if let Some(ty) = self.intercept_struct_update(expr_ref)? {
            return Ok(ty);
        }
        let expr_obj = self.core.expr_pool.get(expr_ref)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        match expr_obj.clone().accept_expr(self) {
            // Record the type here as well as in `visit_expr`: this
            // route is how a *tail* expression is checked (a statement
            // in value position, a condition), and without this the
            // passes that read `expr_types` — the move check, the
            // effect walk's receiver types, the region check — saw a
            // hole exactly where a block's value is decided.
            Ok(ty) => {
                self.type_inference.set_expr_type(*expr_ref, ty.clone());
                Ok(ty)
            }
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

    /// Get human-readable type name for error messages. Backed by
    /// `TypeDecl::spell_with` so user types resolve through the
    /// interner — the old catch-all fell back to Debug, leaking
    /// interned symbol ids (`identifier(symbolu32 { value: 40 })`)
    /// into messages.
    pub fn type_name_for_error(&self, type_decl: &TypeDecl) -> String {
        type_decl.spell_with(Some(self.core.string_interner))
    }
}
