use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::environment::VariableSetType;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use crate::value::Value;
use super::{convert_object, EvaluationContext, EvaluationResult};

/// Apply the val/var annotation to fill in `type_args` on a generic
/// struct / enum value when the construction itself couldn't infer
/// them (e.g. unit enum variant `Option::None`). The runtime never
/// changes the underlying object's identity — it just patches the
/// `type_args` slot via a single `RefMut` borrow.
pub(super) fn apply_annotation_type_args(value: Value, annotation: Option<&TypeDecl>) -> Value {
    let Some(anno) = annotation else { return value; };
    let args: Vec<TypeDecl> = match anno {
        TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) => args.clone(),
        _ => return value,
    };
    if args.is_empty() {
        return value;
    }
    if let Value::Heap(rc) = &value {
        let mut borrow = rc.borrow_mut();
        match &mut *borrow {
            Object::Struct { type_args, .. } | Object::EnumVariant { type_args, .. } => {
                // Patch when the existing args are empty OR when
                // every existing arg is `Unknown` — `derive_struct_type_args`
                // emits `[Unknown; N]` for a generic struct whose
                // fields don't reference `T` (e.g. `struct Container<T> { value: u64 }`),
                // which prevents CONCRETE-IMPL Phase 2 dispatch from
                // matching `[u8]` against `[Unknown]`. The annotation
                // is the source of truth for `T` in that case.
                //
                // The merge is element-wise rather than all-or-nothing.
                // `derive_enum_type_args` infers only from the argument
                // values, so `Result::Ok(v)` binds `T` and leaves `E`
                // `Unknown` — a *mixed* vector that an all-or-nothing
                // rule refuses to touch. The compiled lanes print the
                // declared `E` there, so the lanes disagreed on the
                // rendering of any `Result` whose error type only the
                // annotation knows (UNIT-TYPE-ARG surfaced it, but
                // `Result<u64, E>` has the same shape). Concrete args
                // already inferred from the values win; the annotation
                // fills the holes.
                if type_args.is_empty() {
                    *type_args = args.clone();
                } else {
                    for (slot, from_anno) in type_args.iter_mut().zip(args.iter().cloned()) {
                        if matches!(slot, TypeDecl::Unknown) {
                            *slot = from_anno;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // TREE-WALKER-SELF-TYPE-ARG: reach the payload too. An
    // `Option<Win<u64>>` says what its `Win` is, and a payload built
    // from a bare struct literal (`Option::Some(Win { addr: p })`)
    // has no other evidence — the three compiled lanes read the
    // annotation there, and without this the tree-walker was the only
    // one that could not answer `__builtin_sizeof::<T>()` on the
    // value that came out.
    stamp_payload_type_args(&value, &args);
    value
}

/// Fill in the type arguments of an enum value's payloads from the
/// annotation's own arguments, matching by the type's base name.
///
/// `Option<Win<u64>>` carries one argument, `Win<u64>`; a payload
/// object named `Win` takes its `[u64]` from it. Matching by name
/// rather than by position is what makes this safe for the enums that
/// carry more than one (`Result<Win<u64>, MyErr>`), where a payload's
/// position in the variant says nothing about which parameter it fills.
fn stamp_payload_type_args(value: &Value, args: &[TypeDecl]) {
    if args.is_empty() {
        return;
    }
    let Value::Heap(rc) = value else { return };
    let payloads: Vec<RcObject> = match &*rc.borrow() {
        Object::EnumVariant { values, .. } => values.clone(),
        _ => return,
    };
    for payload in payloads {
        // Only a payload that has nothing to say for itself. A
        // `derive_*_type_args` that could not see `T` in the value
        // leaves `[Unknown; N]` rather than an empty vector, and that
        // is the same "nothing" — concrete arguments already inferred
        // from the values win, exactly as they do at the top level.
        let unknown = |args: &Vec<TypeDecl>| {
            args.is_empty() || args.iter().all(|a| matches!(a, TypeDecl::Unknown))
        };
        let name = match &*payload.borrow() {
            Object::Struct { type_name, type_args, .. } if unknown(type_args) => *type_name,
            Object::EnumVariant { enum_name, type_args, .. } if unknown(type_args) => *enum_name,
            _ => continue,
        };
        let Some(matching) = args.iter().find(|a| match a {
            TypeDecl::Struct(n, inner) | TypeDecl::Enum(n, inner) => {
                *n == name && !inner.is_empty()
            }
            _ => false,
        }) else {
            continue;
        };
        let inner_args = match matching {
            TypeDecl::Struct(_, inner) | TypeDecl::Enum(_, inner) => inner.clone(),
            _ => continue,
        };
        match &mut *payload.borrow_mut() {
            Object::Struct { type_args, .. } | Object::EnumVariant { type_args, .. } => {
                *type_args = inner_args;
            }
            _ => {}
        }
    }
}

impl EvaluationContext<'_> {
    /// A `val` / `var` annotation with the running body's type
    /// parameters replaced by what this call bound them to.
    ///
    /// Inside `EnumerateIter<T>::collect`, `Vec<(u64, T)>` is written
    /// with the *iterator's* `T`. Stamped onto the value as written, the
    /// `Vec`'s own `T` became `(u64, T)` once `push` was entered, and
    /// the scopes, merged by name, then read that `T` as itself:
    /// `__builtin_sizeof::<T>()` recursed until the stack ran out. The
    /// annotation's parameters are the enclosing body's, so they are
    /// resolved here, before the callee's scope can shadow them.
    fn resolve_annotation(&self, annotation: Option<&TypeDecl>) -> Option<TypeDecl> {
        let anno = annotation?;
        if self.generic_type_scopes.is_empty() {
            return Some(anno.clone());
        }
        Some(super::call::substitute_params(anno, &self.merged_generic_scope()))
    }

    /// `one` is the step, passed rather than derived: `T::from(1u8)`
    /// looked like the obvious bound until `i8` turned out not to
    /// implement `From<u8>`, and an `i8` range is one of the widths
    /// NUM-W-FOR-RANGE had to cover.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_for_loop<T>(
        &mut self,
        loop_label: Option<DefaultSymbol>,
        identifier: DefaultSymbol,
        start: T,
        end: T,
        one: T,
        statements: &Vec<StmtRef>,
        create_object: fn(T) -> Object,
    ) -> Result<EvaluationResult, InterpreterError>
    where
        T: Copy + std::cmp::PartialOrd + std::ops::Add<Output = T>,
    {
        let mut current = start;

        while current < end {
            // CHECK-NONTERMINATION: the other back-edge. A range this
            // wide is unreachable from source (`0u64 to u64::MAX` is
            // legal but nobody waits for it); it is reachable from a
            // property trial, whose bounds are sampled.
            self.charge_loop_step()?;
            self.environment.enter_block();
            // Phase 5: bypass the `Object → Value` conversion by lifting
            // the primitive directly into a `Value` variant.
            let iter_value: crate::value::Value = create_object(current).into();
            self.environment.set_var(
                identifier,
                iter_value,
                VariableSetType::Insert,
                self.string_interner,
            )?;

            let res_block = self.evaluate_block(statements);
            self.environment.exit_block();

            match res_block {
                Ok(EvaluationResult::Value(_)) => (),
                Ok(EvaluationResult::Return(v)) => return Ok(EvaluationResult::Return(v)),
                // LABEL: bare `break` / `break @self_label` consume here,
                // foreign labels propagate to the enclosing loop.
                Ok(EvaluationResult::Break(target)) => {
                    if target.is_none() || target == loop_label {
                        break;
                    } else {
                        return Ok(EvaluationResult::Break(target));
                    }
                }
                Ok(EvaluationResult::Continue(target)) => {
                    if target.is_none() || target == loop_label {
                        current = current + one;
                        continue;
                    } else {
                        return Ok(EvaluationResult::Continue(target));
                    }
                }
                Ok(EvaluationResult::None) => (),
                Err(e) => return Err(e),
            }

            current = current + one;
        }

        Ok(EvaluationResult::Value((Object::null_unknown()).into()))
    }

    pub fn evaluate_block(&mut self, statements: &[StmtRef] ) -> Result<EvaluationResult, InterpreterError> {
        // Phase 5 (汎用 RAII): every block opens a fresh auto-drop
        // scope. `Drop`-impling bindings registered inside the
        // body get drained at exit (linear / Return / Break /
        // Continue) in reverse declaration order. Errors from
        // the body skip the drop calls — the process is heading
        // toward exit / panic so running drops would risk
        // double-faulting. The inner method holds the body so
        // the scope mgmt sits cleanly around it.
        self.enter_drop_scope();
        // LEND-FREEING-CALLEE: a function body's own scope holds the
        // parameters it was handed.
        if !self.pending_param_drops.is_empty() {
            for (name, decl) in std::mem::take(&mut self.pending_param_drops) {
                if let Some(value) = self.environment.get_val(name) {
                    self.register_drop_if_needed(decl, name, &value);
                }
            }
        }
        let result = self.evaluate_block_body(statements);
        match result {
            Ok(v) => {
                // DROP-GLUE: the block's own value escapes, so it is
                // not this scope's to free.
                let escaping = match &v {
                    EvaluationResult::Value(value) => Some(value.clone()),
                    EvaluationResult::Return(Some(value)) => Some(value.clone()),
                    _ => None,
                };
                self.run_and_pop_drop_scope_except(escaping.as_ref())?;
                Ok(v)
            }
            Err(e) => {
                self.discard_drop_scope();
                Err(e)
            }
        }
    }

    fn evaluate_block_body(&mut self, statements: &[StmtRef]) -> Result<EvaluationResult, InterpreterError> {
        let to_stmt = |s: &StmtRef| -> Result<Stmt, InterpreterError> {
            self.stmt_pool.get(s)
                .ok_or_else(|| InterpreterError::InternalError("Invalid statement reference".to_string()))
        };
        // The refs are kept alongside the statements: a `val` / `var`
        // that transferred its value away must not register a drop, and
        // `File::transferred_bindings` is keyed by the statement
        // (BOX-T).
        let statements = statements
            .iter()
            .map(|s| to_stmt(s).map(|stmt| (*s, stmt)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut last: Option<EvaluationResult> = None;

        for (stmt_ref, stmt) in statements {
            if !self.drop_flags.is_empty() {
                let flags = self.drop_flags.clone();
                if let Some(decls) = flags.clear_before_stmt.get(&stmt_ref) {
                    self.disarm_drops(decls);
                }
            }
            match stmt {
                Stmt::Val(name, annotation, e) => {
                    // val/var declarations don't themselves produce a value, but
                    // the rhs may propagate control flow (e.g. `val x = return ...`)
                    // which we must surface to the enclosing function/loop.
                    match self.handle_val_declaration(stmt_ref, name, annotation.as_ref(), &e)? {
                        flow @ (EvaluationResult::Return(_)
                                | EvaluationResult::Break(_)
                                | EvaluationResult::Continue(_)) => return Ok(flow),
                        _ => last = None,
                    }
                }
                Stmt::Var(name, annotation, e) => {
                    match self.handle_var_declaration(stmt_ref, name, annotation.as_ref(), &e)? {
                        flow @ (EvaluationResult::Return(_)
                                | EvaluationResult::Break(_)
                                | EvaluationResult::Continue(_)) => return Ok(flow),
                        _ => last = None,
                    }
                }
                Stmt::Return(e) => {
                    return self.handle_return_statement(&e);
                }
                Stmt::Break(label) => {
                    return Ok(EvaluationResult::Break(label));
                }
                Stmt::Continue(label) => {
                    return Ok(EvaluationResult::Continue(label));
                }
                Stmt::StructDecl { .. } => {
                    // Struct declarations are handled at compile time
                    last = None;
                }
                Stmt::ImplBlock { .. } => {
                    // Impl blocks are handled at compile time
                    last = None;
                }
                Stmt::EnumDecl { .. } => {
                    // Enum declarations are handled at compile time; nothing to do at runtime.
                    last = None;
                }
                Stmt::TraitDecl { .. } => {
                    // Trait declarations are handled at compile time; their
                    // method signatures live in the type checker context and
                    // do not produce a runtime value.
                    last = None;
                }
                Stmt::TypeAlias { .. } => {
                    // Type aliases are resolved by the parser; they have no
                    // runtime effect.
                    last = None;
                }
                Stmt::While(label, cond, body) => {
                    // DICT-RETURN-WHILE fix: the while-loop body
                    // may produce a `Return` (an explicit
                    // `return v` inside the loop). Storing the
                    // result in `last` without propagating
                    // swallows the signal — the function then
                    // falls through to whatever expression
                    // follows the loop. Mirror the `For` arm
                    // immediately below: surface Return / Break
                    // / Continue to the enclosing block instead
                    // of treating them as a value.
                    let result = self.handle_while_loop(label, &cond, &body)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break(t) => return Ok(EvaluationResult::Break(t)),
                        EvaluationResult::Continue(t) => return Ok(EvaluationResult::Continue(t)),
                        _ => last = Some(EvaluationResult::Value((Object::Unit).into())),
                    }
                }
                Stmt::For(label, identifier, start, end, block) => {
                    let result = self.handle_for_loop(label, identifier, &start, &end, &block)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break(t) => return Ok(EvaluationResult::Break(t)),
                        EvaluationResult::Continue(t) => return Ok(EvaluationResult::Continue(t)),
                        _ => last = Some(EvaluationResult::Value((Object::Unit).into())),
                    }
                }
                Stmt::Expression(expr) => {
                    let result = self.handle_expression_statement(&expr)?;
                    match result {
                        EvaluationResult::Return(v) => return Ok(EvaluationResult::Return(v)),
                        EvaluationResult::Break(t) => return Ok(EvaluationResult::Break(t)),
                        EvaluationResult::Continue(t) => return Ok(EvaluationResult::Continue(t)),
                        other => last = Some(other),
                    }
                }
            }
        }

        if last.is_some() {
            last.ok_or_else(|| InterpreterError::InternalError("Empty block evaluation".to_string()))
        } else {
            Ok(EvaluationResult::None)
        }
    }

    /// Handles val (immutable variable) declarations.
    ///
    /// Returns `EvaluationResult::None` on success (a `val` is not itself a
    /// value-producing statement). Control flow inside the rhs (e.g.
    /// `val x = if cond { return 100 } else { 5 }`) is propagated as
    /// `Ok(Return(...))` so the enclosing function returns correctly —
    /// previously this would surface as a stray "Propagate flow:" error.
    fn handle_val_declaration(
        &mut self,
        stmt_ref: StmtRef,
        name: DefaultSymbol,
        annotation: Option<&frontend::type_decl::TypeDecl>,
        expr: &ExprRef,
    ) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        // POINTER P1: make the annotation visible while the rhs
        // evaluates — `val h: Holder<u64> = Holder::make(n)` derives
        // the callee body's `T` from it. Restored before the flow
        // check so an early `return` in the rhs cannot leak it.
        let annotation = self.resolve_annotation(annotation);
        let annotation = annotation.as_ref();
        let prev_annotation = self.pending_annotation.take();
        self.pending_annotation = annotation.cloned();
        let value = self.evaluate(expr);
        self.pending_annotation = prev_annotation;
        let value = try_value_v!(value);
        let value = apply_annotation_type_args(value, annotation);
        // DROP-GLUE: `val v: T = __builtin_ptr_read::<T>(...)` copies the
        // slot's value out. The copy is an *alias* of the slot (the
        // slot's owner frees it when it dies), so the binding must not
        // register a drop — otherwise `Box::get()` on a `Box<Box<i64>>`
        // frees the inner slot while the outer box still owns it.
        // DATA-ORIENTED Phase 2: `__builtin_soa_read` copies out of a
        // column-split buffer the same way, and is an alias for the
        // same reason.
        let from_ptr_read = matches!(
            self.expr_pool.get(expr),
            Some(Expr::BuiltinCall(
                frontend::ast::BuiltinFunction::PtrReadTyped(_)
                    | frontend::ast::BuiltinFunction::PtrRefTyped(_)
                    | frontend::ast::BuiltinFunction::SoaRead,
                _
            ))
        );
        if !from_ptr_read {
            // Phase 5 (汎用 RAII): record the binding for auto-drop
            // before consuming `value` into the environment.
            self.register_drop_if_needed(stmt_ref, name, &value);
        }
        self.environment.set_val(name, value);
        Ok(EvaluationResult::None)
    }

    /// Handles var (mutable variable) declarations. Same flow-propagation
    /// convention as `handle_val_declaration`.
    fn handle_var_declaration(
        &mut self,
        stmt_ref: StmtRef,
        name: DefaultSymbol,
        annotation: Option<&frontend::type_decl::TypeDecl>,
        expr: &Option<ExprRef>,
    ) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        // POINTER P1: same pending-annotation window as `val`.
        let annotation = self.resolve_annotation(annotation);
        let annotation = annotation.as_ref();
        let prev_annotation = self.pending_annotation.take();
        self.pending_annotation = annotation.cloned();
        let evaluated: Result<crate::evaluation::EvaluationResult, InterpreterError> =
            if let Some(e) = expr {
                self.evaluate(e)
            } else {
                Ok(crate::evaluation::EvaluationResult::Value(self.null_object.clone().into()))
            };
        self.pending_annotation = prev_annotation;
        let value: crate::value::Value = try_value_v!(evaluated);
        let value = apply_annotation_type_args(value, annotation);
        // DROP-GLUE: same ptr_read alias rule as `val` — see
        // `handle_val_declaration`.
        let from_ptr_read = expr.is_some_and(|e| {
            matches!(
                self.expr_pool.get(&e),
                Some(Expr::BuiltinCall(
                    frontend::ast::BuiltinFunction::PtrReadTyped(_)
                        | frontend::ast::BuiltinFunction::PtrRefTyped(_)
                        | frontend::ast::BuiltinFunction::SoaRead,
                    _
                ))
            )
        });
        if !from_ptr_read {
            // Phase 5 (汎用 RAII): same as val — `var` bindings are
            // also auto-dropped at scope exit when the type impls
            // Drop. (Reassignment via `var x = ...` later in scope
            // doesn't re-trigger registration; the original Rc is
            // shared so the drop record stays valid.)
            self.register_drop_if_needed(stmt_ref, name, &value);
        }
        self.environment.set_var(name, value, VariableSetType::Insert, self.string_interner)?;
        Ok(EvaluationResult::None)
    }

    /// Handles return statements
    fn handle_return_statement(&mut self, expr: &Option<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        if expr.is_none() {
            return Ok(EvaluationResult::Return(None));
        }
        match self.evaluate(expr.as_ref().ok_or_else(|| InterpreterError::InternalError("Missing expression in return".to_string()))?)? {
            EvaluationResult::Value(v) => Ok(EvaluationResult::Return(Some(v))),
            EvaluationResult::Return(v) => Ok(EvaluationResult::Return(v)),
            EvaluationResult::Break(_) => Err(InterpreterError::InternalError("break cannot be used in here".to_string())),
            EvaluationResult::Continue(_) => Err(InterpreterError::InternalError("continue cannot be used in here".to_string())),
            EvaluationResult::None => Err(InterpreterError::InternalError("unexpected None".to_string())),
        }
    }

    /// Handles while loop execution. LABEL: `loop_label` (`Some(sym)` for
    /// `@sym: while ...`) decides whether `Break(target)` / `Continue(target)`
    /// is consumed locally or propagated to an enclosing loop.
    fn handle_while_loop(&mut self, loop_label: Option<DefaultSymbol>, cond: &ExprRef, body: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        loop {
            // CHECK-NONTERMINATION: one of the two back-edges a
            // toylang run has. Charged before the condition so a
            // condition that itself loops forever is covered too.
            self.charge_loop_step()?;
            let cond_result = self.evaluate(cond);
            let cond_value = try_value_v!(cond_result);
            let cond_bool = cond_value.try_unwrap_bool().map_err(InterpreterError::ObjectError)?;

            if !cond_bool {
                break;
            }

            let body_expr = self.expr_pool.get(body)
                .ok_or_else(|| InterpreterError::InternalError("Invalid body expression reference".to_string()))?;
            if let Expr::Block(statements) = body_expr {
                self.environment.enter_block();
                let res = self.evaluate_block(&statements);
                self.environment.exit_block();

                match res {
                    Ok(EvaluationResult::Value(_)) => (),
                    Ok(EvaluationResult::Return(v)) => return Ok(EvaluationResult::Return(v)),
                    Ok(EvaluationResult::Break(target)) => {
                        if target.is_none() || target == loop_label {
                            break;
                        } else {
                            return Ok(EvaluationResult::Break(target));
                        }
                    }
                    Ok(EvaluationResult::Continue(target)) => {
                        if target.is_none() || target == loop_label {
                            continue;
                        } else {
                            return Ok(EvaluationResult::Continue(target));
                        }
                    }
                    Ok(EvaluationResult::None) => (),
                    Err(e) => return Err(e),
                }
            } else {
                return Err(InterpreterError::InternalError("While body is not a block".to_string()));
            }
        }
        Ok(EvaluationResult::Value((Object::Unit).into()))
    }

    /// Handles for loop execution
    fn handle_for_loop(&mut self, loop_label: Option<DefaultSymbol>, identifier: DefaultSymbol, start: &ExprRef, end: &ExprRef, block: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        let start = self.evaluate(start);
        let start_v = try_value_v!(start);
        let end = self.evaluate(end);
        let end_v = try_value_v!(end);
        let start_ty = start_v.get_type();
        let end_ty = end_v.get_type();

        if start_ty != end_ty {
            return Err(InterpreterError::TypeError {
                expected: start_ty,
                found: end_ty,
                message: "evaluate_block: Bad types for 'for' loop due to different type".to_string()
            });
        }

        let block = self.expr_pool.get(block)
            .ok_or_else(|| InterpreterError::InternalError("Invalid block expression reference".to_string()))?;
        if let Expr::Block(statements) = block {
            // NUM-W-FOR-RANGE: every integer width drives a range, not
            // just the two wide ones. `for i in -3i32..2i32` used to be
            // "For loop range must be UInt64 or Int64" here while the
            // IR VM ran it — the type checker accepts a narrow range,
            // so the three lanes disagreed on a program that looks
            // ordinary.
            //
            // Matching on the `Value` pair rather than on the declared
            // type keeps one arm per width and no unwrap per width; the
            // equal-types check above has already run, so a mismatched
            // pair here can only be a non-integer range.
            macro_rules! range_over {
                ($start:expr, $end:expr, $one:expr, $ctor:path) => {
                    self.execute_for_loop(
                        loop_label, identifier, $start, $end, $one, &statements, $ctor,
                    )
                };
            }
            match (start_v, end_v) {
                (Value::UInt64(a), Value::UInt64(b)) => range_over!(a, b, 1u64, Object::UInt64),
                (Value::Int64(a), Value::Int64(b)) => range_over!(a, b, 1i64, Object::Int64),
                (Value::UInt32(a), Value::UInt32(b)) => range_over!(a, b, 1u32, Object::UInt32),
                (Value::Int32(a), Value::Int32(b)) => range_over!(a, b, 1i32, Object::Int32),
                (Value::UInt16(a), Value::UInt16(b)) => range_over!(a, b, 1u16, Object::UInt16),
                (Value::Int16(a), Value::Int16(b)) => range_over!(a, b, 1i16, Object::Int16),
                (Value::UInt8(a), Value::UInt8(b)) => range_over!(a, b, 1u8, Object::UInt8),
                (Value::Int8(a), Value::Int8(b)) => range_over!(a, b, 1i8, Object::Int8),
                _ => Err(InterpreterError::TypeError {
                    expected: TypeDecl::UInt64,
                    found: start_ty,
                    message: "For loop range must be an integer type".to_string(),
                }),
            }
        } else {
            Err(InterpreterError::InternalError("For loop body is not a block".to_string()))
        }
    }

    /// Handles expression statements
    fn handle_expression_statement(&mut self, expr: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let e = self.expr_pool.get(expr)
            .ok_or_else(|| InterpreterError::InternalError("Invalid expression reference".to_string()))?;
        match e {
            Expr::Assign(lhs, rhs) => {
                self.handle_assignment(&lhs, &rhs)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                let obj = convert_object(&e)?;
                Ok(EvaluationResult::Value((obj).into()))
            }
            Expr::Identifier(s) => {
                self.handle_identifier_expression(s)
            }
            Expr::Block(blk_expr) => {
                self.handle_nested_block(&blk_expr)
            }
            _ => {
                // Take care to handle loop control flow correctly when break/continue is executed
                // in nested loops. These statements affect only their immediate enclosing loop.
                self.evaluate(expr)
            }
        }
    }

    /// Handles assignment expressions (variable, field, and array element assignment)
    fn handle_assignment(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        if let Some(lhs_expr) = self.expr_pool.get(lhs) {
            match lhs_expr {
                Expr::Identifier(name) => self.handle_variable_assignment(name, lhs, rhs),
                Expr::FieldAccess(obj, field) => self.handle_field_assignment(&obj, field, rhs),
                Expr::TupleAccess(obj, index) => self.handle_tuple_element_assignment(&obj, index, rhs),
                _ => {
                    Err(InterpreterError::InternalError("bad assignment due to lhs is not identifier or array access".to_string()))
                }
            }
        } else {
            Err(InterpreterError::InternalError("bad assignment due to invalid lhs reference".to_string()))
        }
    }

    /// Handles field assignment: `obj.field = rhs`
    fn handle_field_assignment(&mut self, obj: &ExprRef, field: DefaultSymbol, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Evaluate the receiver first so we hold an Rc to the underlying struct.
        // Mutating through the Rc updates every alias (which is the whole point —
        // `self.field = x` inside a method has to be observable on the caller's copy).
        let obj_val = self.evaluate(obj);
        let obj_val = try_value!(obj_val);

        // Evaluate the right-hand side, mirroring handle_variable_assignment's
        // Null-shortcut so `obj.field = null` keeps working.
        let rhs_expr = self.expr_pool.get(rhs)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", rhs)))?;
        let new_value = match rhs_expr {
            Expr::Null => self.null_object.clone(),
            _ => {
                let v = self.evaluate(rhs);
                try_value!(v)
            }
        };

        {
            let mut obj_borrowed = obj_val.borrow_mut();
            match &mut *obj_borrowed {
                Object::Struct { fields, .. } => {
                    if !fields.contains_key(&field) {
                        let field_name = self
                            .string_interner
                            .resolve(field)
                            .unwrap_or("<unknown>");
                        return Err(InterpreterError::InternalError(format!(
                            "Cannot assign to unknown field '{}'", field_name
                        )));
                    }
                    fields.insert(field, new_value.clone());
                }
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "Cannot assign field on non-struct object: {:?}", other
                    )));
                }
            }
        }

        let cloned_value = new_value.borrow().clone();
        Ok(EvaluationResult::Value((cloned_value).into()))
    }

    /// `t.0 = v`: the tuple counterpart of `handle_field_assignment`.
    /// Writing through the evaluated receiver's `Rc` reaches every
    /// alias -- the caller's tuple, for a `&mut (A, B)` parameter.
    /// Missing before, so `t.0 = t.1` stopped the tree-walker with an
    /// internal error while the compiled lanes ran it.
    fn handle_tuple_element_assignment(
        &mut self,
        obj: &ExprRef,
        index: usize,
        rhs: &ExprRef,
    ) -> Result<EvaluationResult, InterpreterError> {
        let obj_val = self.evaluate(obj);
        let obj_val = try_value!(obj_val);
        let new_value = self.evaluate(rhs);
        let new_value = try_value!(new_value);
        {
            let mut obj_borrowed = obj_val.borrow_mut();
            match &mut *obj_borrowed {
                Object::Tuple(elements) => {
                    let len = elements.len();
                    let slot = elements.get_mut(index).ok_or(InterpreterError::IndexOutOfBounds {
                        index: index as isize,
                        size: len,
                    })?;
                    *slot = new_value.clone();
                }
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "Cannot assign a tuple element on a non-tuple object: {:?}", other
                    )));
                }
            }
        }
        let cloned_value = new_value.borrow().clone();
        Ok(EvaluationResult::Value((cloned_value).into()))
    }

    /// Handles variable assignment
    fn handle_variable_assignment(&mut self, name: DefaultSymbol, lhs: &ExprRef, rhs: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        use crate::try_value_v;
        // Handle null expressions specially in variable assignments
        let expr = self.expr_pool.get(rhs)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", rhs)))?;

        let rhs_v: crate::value::Value = match expr {
            Expr::Null => {
                // Pre-created null object for variable assignments. The
                // shared cell is wrapped via `From<RcObject>` so primitives
                // get lifted out, but here it carries `Object::Null(_)` so
                // it stays as `Value::Heap` — same semantics as before.
                self.null_object.clone().into()
            }
            _ => {
                let rhs = self.evaluate(rhs);
                try_value_v!(rhs)
            }
        };

        // type check
        let existing_val = self.environment.get_val(name);
        if existing_val.is_none() {
            return Err(InterpreterError::UndefinedVariable("bad assignment due to variable was not set".to_string()));
        }
        let existing_val = existing_val.unwrap();
        let val_ty = existing_val.get_type();
        let rhs_ty = rhs_v.get_type();

        if val_ty != rhs_ty {
            // Allow null assignment to any type
            if !matches!(rhs_ty, TypeDecl::Unknown) {
                return Err(InterpreterError::TypeError {
                    expected: val_ty,
                    found: rhs_ty,
                    message: "Bad types for assignment due to different type".to_string()
                });
            }
        }

        if !self.drop_flags.is_empty() {
            if let Some(decl) = self.drop_flags.reinit.get(lhs).copied() {
                self.reinit_drop(decl, &rhs_v)?;
            }
        }
        self.environment.set_var(name, rhs_v.clone(), VariableSetType::Overwrite, self.string_interner)?;
        Ok(EvaluationResult::Value(rhs_v))
    }


    /// Handles identifier expressions
    fn handle_identifier_expression(&mut self, symbol: DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        let obj = self.environment.get_val(symbol);
        let obj_ref = obj.clone();
        if obj.is_none() || obj.unwrap().is_null() {
            let s = self.string_interner.resolve(symbol).unwrap_or("<NOT_FOUND>");
            return Err(InterpreterError::UndefinedVariable(format!("Identifier {s} is null")));
        }
        Ok(EvaluationResult::Value(obj_ref.unwrap()))
    }

    /// Handles nested block expressions
    fn handle_nested_block(&mut self, statements: &[StmtRef]) -> Result<EvaluationResult, InterpreterError> {
        self.environment.enter_block();
        let result = self.evaluate_block(statements)?;
        self.environment.exit_block();
        Ok(result)
    }
}
