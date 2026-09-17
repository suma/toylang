use string_interner::DefaultSymbol;
use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    AcceptableStmt, AcceptableDecl,
};

/// Statement type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Main entry point for statement type checking
    pub fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        let mut stmt_val = self.core.stmt_pool.get(stmt).unwrap_or(Stmt::Break(None)).clone();
        
        let result = stmt_val.accept_stmt(self);
        
        // If an error occurred, try to add location information if not already present
        let result = match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_stmt_location(stmt);
                Err(error)
            }
            other => other,
        };
        
        // Declaration statements also need to be dispatched through DeclVisitor
        // so their definitions are registered in the type-check context.
        if result.is_ok() {
            match &stmt_val {
                Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => {
                    stmt_val.accept_decl(self)?;
                }
                _ => {}
            }
        }
        
        result
    }

    /// Type check expression statements
    pub fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.check_expr_located(expr)
    }

    /// Type check variable declarations (var) - internal implementation
    pub fn visit_var_impl(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let type_decl = type_decl.clone();
        let expr = *expr;
        // REF-Stage-2 (f): `var` bindings are mutable, so subsequent
        // `&mut <name>` borrow expressions are accepted by the type
        // checker.
        self.process_val_type_with_mut(name, &type_decl, &expr, true)?;
        Ok(TypeDecl::Unit)
    }

    /// Type check value declarations (val) - internal implementation
    pub fn visit_val_impl(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_ref = *expr;
        // LLM-LOOP P7: `val x: _ = expr` is a hole. For the rest of this
        // routine it behaves exactly like an unannotated binding
        // (`Unknown` is how the parser records "no annotation"); the
        // answer is reported once the inferred type is known.
        let is_hole = matches!(type_decl, Some(TypeDecl::Hole));
        let type_decl = if is_hole { Some(TypeDecl::Unknown) } else { type_decl.clone() };

        // REF-Stage-2 (e): syntactic escape rule — a `val` binding
        // cannot annotate a reference type. The inferred-type form
        // is also rejected after evaluation below.
        if let Some(decl) = type_decl.as_ref()
            && decl.contains_ref() {
                let var_name = self.resolve_symbol_name(name);
                return Err(TypeCheckError::generic_error(&format!(
                    "binding `{}` annotates a reference type; references cannot be \
                     stored in val / var bindings (REF-Stage-2 (e))",
                    var_name
                )));
            }

        // Set type hint and evaluate expression
        let old_hint = self.setup_type_hint_for_val(&type_decl);
        let expr_ty = self.visit_expr(&expr_ref)?;

        // NUMBER-HINT: an explicit annotation is the most direct
        // statement of what an unsuffixed literal should be, so it
        // claims the literal here rather than leaving it to the
        // default pass. `val c: i64 = 10` used to land on `i64` only
        // by way of a function-wide hint that happened to be set —
        // nothing made the annotation itself decide.
        let expr_ty = match type_decl.as_ref() {
            Some(decl) => self.coerce_number_expr(&expr_ref, &expr_ty, decl)?,
            None => expr_ty,
        };

        // Manage variable-expression mapping
        self.update_variable_expr_mapping_internal(name, &expr_ref, &expr_ty);
        
        // Apply type transformations
        self.apply_type_transformations_for_expr(&type_decl, &expr_ty, &expr_ref)?;
        
        // Check type compatibility if explicit type is declared.
        // Normalize `Identifier(s)` to `Generic(s)` when `s` is in
        // the impl's generic-param scope — the parser emits Identifier
        // for any annotation in a method body (it doesn't thread
        // generic context that far), and Identifier vs UInt64 fails
        // even though Generic vs UInt64 succeeds.
        if let Some(declared_type) = &type_decl {
            let normalized = self.normalize_generic_identifier(declared_type);
            if !self.are_types_compatible(&normalized, &expr_ty) {
                let normalized_for_suggestion = normalized.clone();
                let declared_name = self.type_name_for_error(&normalized);
                let expr_name = self.type_name_for_error(&expr_ty);
                // LLM-LOOP P2: anchor at the initializer, not at the
                // statement. The offending value is the rhs; pointing
                // the caret at `val` tells the reader nothing about
                // which part of the line to change.
                let err = TypeCheckError::type_mismatch(
                    normalized,
                    expr_ty.clone()
                ).with_context(&format!("Cannot convert '{}' to '{}'", expr_name, declared_name));
                let err = self.error_with_location(err, &expr_ref);
                return Err(self.suggest_numeric_cast(err, &expr_ref, &expr_ty, &normalized_for_suggestion));
            }
        }
        
        // Determine final type and store variable
        let final_type = self.determine_final_type_for_expr(&type_decl, &expr_ty);

        // REF-Stage-2 (e): same escape rule for the inferred-type
        // case (no annotation, rhs evaluated to a reference type).
        if final_type.contains_ref() {
            let var_name = self.resolve_symbol_name(name);
            return Err(TypeCheckError::generic_error(&format!(
                "binding `{}` is inferred to a reference type; references cannot be \
                 stored in val / var bindings (REF-Stage-2 (e))",
                var_name
            )));
        }

        // Debug: Print variable type information
        let _var_name_str = self.resolve_symbol_name(name);

        // Extract type parameter mappings for generic struct instances
        if let TypeDecl::Struct(struct_name, type_params) = &final_type
            && !type_params.is_empty() {
                // Get the generic parameter names for this struct
                if let Some(generic_param_names) = self.context.get_struct_generic_params(*struct_name) {
                    let mut type_mappings = HashMap::new();

                    // Create mappings from parameter names to concrete types
                    for (param_name, concrete_type) in generic_param_names.iter().zip(type_params.iter()) {
                        type_mappings.insert(*param_name, concrete_type.clone());
                    }

                    // Store the type parameter mappings for this variable
                    self.context.set_var_type_mapping(name, type_mappings);
                }
            }
        
        self.context.set_var(name, final_type.clone());

        // Restore previous type hint
        self.type_inference.type_hint = old_hint;

        if is_hole {
            // NUMBER-HINT: an unresolved literal has no answer yet —
            // wait until the function's literals are settled.
            if final_type == TypeDecl::Number {
                self.pending_number_holes.push((name, expr_ref));
            } else if let Some(err) = self.report_type_hole(name, &final_type, &expr_ref) {
                return Err(err);
            }
        }

        Ok(TypeDecl::Unit)
    }

    /// Type check return statements
    pub fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if expr.is_none() {
            Ok(TypeDecl::Unit)
        } else {
            let e = expr.as_ref()
                .ok_or_else(|| TypeCheckError::generic_error("Expected expression in return"))?;
            let return_type = self.check_expr_located(e)?;
            // NUMBER-HINT: an explicit `return 0` names the same
            // position as the tail expression, so the enclosing
            // function's declared return type claims the literal.
            let coerced = match self.current_fn_return_type.clone() {
                Some(fn_ret) => self.coerce_number_expr(e, &return_type, &fn_ret)?,
                None => return_type,
            };
            self.validate_return_type(&coerced)?;
            Ok(coerced)
        }
    }

    /// TRY-ERR-RETYPE: the value a `return` carries must be compatible
    /// with the enclosing function's declared return type. Before this
    /// check the checker only coerced numbers, so `return` of the
    /// inner `Result<T1, E>` from a fn declared `-> Result<T2, E>` (the
    /// shape a `?` whose success type differs produces) type-checked
    /// and then died in the compiled lanes with "not an enum binding of
    /// the expected return type" — a TYPECHECK-LIES.
    ///
    /// `is_equivalent` is the right looseness: it accepts the
    /// Identifier/Struct/Enum spelling drift the parser produces and
    /// lets Unknown / generic parameters through. Closure bodies are
    /// skipped — they are lifted into synthetic functions with their
    /// own return types, and this check still sees the enclosing
    /// function's context. (The tail expression is validated
    /// separately by the function-level body-type check.)
    pub(super) fn validate_return_type(&mut self, ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if self.context.closure_scope_floors.is_empty()
            && let Some(fn_ret) = self.current_fn_return_type.clone()
            && !fn_ret.is_equivalent(ty)
        {
            return Err(TypeCheckError::type_mismatch(ty.clone(), fn_ret)
                .with_context("return statement"));
        }
        Ok(())
    }

    /// Type check for loops - internal implementation
    pub fn visit_for_impl(&mut self, label: Option<DefaultSymbol>, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        self.context.loop_label_stack.push(label);

        let range_ty = self.check_expr_located(range)?;
        let ty = Some(range_ty);

        self.process_val_type(init, &ty, &Some(*range))?;

        let res = self.check_expr_located(body);

        self.context.loop_label_stack.pop();
        self.pop_context();
        res
    }

    /// RANGE-FOR: `for i in r` over a range **value**.
    ///
    /// The parser cannot tell a range value from an iterator (it sees
    /// only a name), so it desugars `for x in EXPR { body }` into the
    /// iterator protocol:
    ///
    /// ```text
    /// while true {
    ///     match RECV.next() {
    ///         Option::Some(x) => { body; continue },
    ///         Option::None => { break },
    ///     }
    /// }
    /// ```
    ///
    /// where `RECV` is `EXPR` itself when it is a name, and otherwise a
    /// `var __iter_for_N = EXPR` bound just before. A range has no
    /// `next`, so this used to stop at "method not found". By the time
    /// the loop is reached the receiver's type is known, and when it is
    /// `Range<T>` the loop is replaced in place by the integer fast
    /// path:
    ///
    /// ```text
    /// for x in RECV.start..RECV.end { body }
    /// ```
    ///
    /// Rewriting rather than giving ranges a `next` keeps the range
    /// **unconsumed**: a stateful `next` would leave a second
    /// `for i in r` over the same binding with nothing to visit. The
    /// bounds are read once, when the loop starts, as `Stmt::For`
    /// always does. Backends only ever see the rewritten loop.
    pub(super) fn rewrite_range_for_in(&mut self, s: &StmtRef) {
        let Some(Stmt::While(label, cond, while_body)) = self.core.stmt_pool.get(s) else {
            return;
        };
        if !matches!(self.core.expr_pool.get(&cond), Some(Expr::True)) {
            return;
        }
        let Some(Expr::Block(inner)) = self.core.expr_pool.get(&while_body) else {
            return;
        };
        let [only] = inner.as_slice() else { return };
        let Some(Stmt::Expression(match_ref)) = self.core.stmt_pool.get(only) else {
            return;
        };
        let Some(Expr::Match(scrutinee, arms)) = self.core.expr_pool.get(&match_ref) else {
            return;
        };
        let Some(Expr::MethodCall(recv_ref, method, args)) = self.core.expr_pool.get(&scrutinee)
        else {
            return;
        };
        if !args.is_empty() || self.core.string_interner.resolve(method) != Some("next") {
            return;
        }
        let Some(Expr::Identifier(recv)) = self.core.expr_pool.get(&recv_ref) else {
            return;
        };
        if !matches!(self.context.get_var(recv), Some(TypeDecl::Range(_))) {
            return;
        }
        // The two arms the parser writes: `Some(x) => { body; continue }`
        // and `None => { break }`.
        let [some_arm, _none_arm] = arms.as_slice() else { return };
        let Pattern::EnumVariant(_, _, subs) = &some_arm.pattern else { return };
        let [Pattern::Name(loop_var)] = subs.as_slice() else { return };
        let Some(Expr::Block(arm_stmts)) = self.core.expr_pool.get(&some_arm.body) else {
            return;
        };
        let [body_stmt, _continue] = arm_stmts.as_slice() else { return };
        let Some(Stmt::Expression(user_body)) = self.core.stmt_pool.get(body_stmt) else {
            return;
        };

        // Seeded by `BuiltinFunctionSymbols::new`.
        let (Some(start_sym), Some(end_sym)) = (
            self.core.string_interner.get("start"),
            self.core.string_interner.get("end"),
        ) else {
            return;
        };
        let start_recv = self.core.expr_pool.add(Expr::Identifier(recv));
        let end_recv = self.core.expr_pool.add(Expr::Identifier(recv));
        let start = self.core.expr_pool.add(Expr::FieldAccess(start_recv, start_sym));
        let end = self.core.expr_pool.add(Expr::FieldAccess(end_recv, end_sym));
        self.core
            .stmt_pool
            .update(s, Stmt::For(label, *loop_var, start, end, user_body));
    }

    /// Type check while loops - internal implementation
    pub fn visit_while_impl(&mut self, label: Option<DefaultSymbol>, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Evaluate condition type first
        let cond_type = self.check_expr_located(cond)?;

        // Verify condition is boolean
        if cond_type != TypeDecl::Bool {
            return Err(TypeCheckError::type_mismatch(TypeDecl::Bool, cond_type));
        }

        // Create new scope for while body
        self.push_context();
        self.context.loop_label_stack.push(label);
        let res = self.check_expr_located(body);
        self.context.loop_label_stack.pop();
        self.pop_context();
        res
    }

    /// Type check break statements. LABEL: validates that bare `break` is
    /// inside a loop, and that `break @label` references an active label.
    pub fn visit_break_impl(&mut self, label: Option<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        self.validate_loop_label("break", label)?;
        Ok(TypeDecl::Unit)
    }

    /// Type check continue statements. Same validation as `break`.
    pub fn visit_continue_impl(&mut self, label: Option<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        self.validate_loop_label("continue", label)?;
        Ok(TypeDecl::Unit)
    }

    fn validate_loop_label(&self, kw: &str, label: Option<DefaultSymbol>) -> Result<(), TypeCheckError> {
        match label {
            None => {
                if self.context.loop_label_stack.is_empty() {
                    Err(TypeCheckError::generic_error(&format!("`{kw}` outside of a loop")))
                } else {
                    Ok(())
                }
            }
            Some(sym) => {
                if self.context.loop_label_stack.iter().rev().any(|l| *l == Some(sym)) {
                    Ok(())
                } else {
                    let name = self.core.string_interner.resolve(sym).unwrap_or("?");
                    Err(TypeCheckError::generic_error(&format!(
                        "`{kw}` references undefined loop label `@{name}`"
                    )))
                }
            }
        }
    }
}