use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EnumRegistryEntry, EnumRegistryVariant, EvaluationContext, EvaluationResult, StructRegistryEntry};
use std::collections::HashMap as HashMapStd;

/// Map a runtime value to the canonical-name `DefaultSymbol` the
/// extension-trait machinery uses as an impl target. Returns `None`
/// for non-primitive values (struct / enum / heap composite) — those
/// reach `evaluate_method_call`'s existing per-variant arms.
///
/// The interner lookup is `get` rather than `get_or_intern`: if a
/// program never wrote `impl Foo for i64 { ... }`, the canonical name
/// `"i64"` was never interned, so there is no symbol and no registry
/// entry to find. Falling back to `None` keeps the dispatch cost a
/// single hashmap probe per primitive method call.
fn primitive_target_symbol(
    obj: &Object,
    interner: &DefaultStringInterner,
) -> Option<DefaultSymbol> {
    let name = match obj {
        Object::Bool(_) => "bool",
        Object::Int64(_) => "i64",
        Object::UInt64(_) => "u64",
        // NUM-W: narrow primitive method dispatch.
        Object::Int8(_) => "i8",
        Object::Int16(_) => "i16",
        Object::Int32(_) => "i32",
        Object::UInt8(_) => "u8",
        Object::UInt16(_) => "u16",
        Object::UInt32(_) => "u32",
        Object::Float64(_) => "f64",
        Object::ConstString(_) | Object::String(_) => "str",
        Object::Pointer(_) => "ptr",
        _ => return None,
    };
    interner.get(name)
}

/// Walk the struct field type-decls looking for `Generic(P)`
/// occurrences and bind each generic parameter to the runtime
/// type of the matching field. Returns `type_args` in the order
/// declared by `entry.generic_params`. Falls back to an empty
/// vector when no generic parameter binding can be derived (e.g.
/// non-generic struct, or generic param appearing only in nested
/// positions we don't drill into).
fn derive_struct_type_args(
    entry: &StructRegistryEntry,
    field_values: &std::collections::HashMap<DefaultSymbol, RcObject>,
) -> Vec<TypeDecl> {
    if entry.generic_params.is_empty() {
        return Vec::new();
    }
    let mut bindings: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
    for (field_name, field_ty) in &entry.fields {
        if let Some(value) = field_values.get(field_name) {
            collect_generic_bindings(field_ty, &value.borrow(), &mut bindings);
        }
    }
    entry
        .generic_params
        .iter()
        .map(|p| bindings.get(p).cloned().unwrap_or(TypeDecl::Unknown))
        .collect()
}

/// Variant counterpart to `derive_struct_type_args`. Walks the
/// variant payload type list against the constructed argument
/// values to derive each generic parameter binding.
fn derive_enum_type_args(
    entry: &EnumRegistryEntry,
    variant: &EnumRegistryVariant,
    arg_values: &[RcObject],
) -> Vec<TypeDecl> {
    if entry.generic_params.is_empty() {
        return Vec::new();
    }
    let mut bindings: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
    for (declared, value) in variant.payload_types.iter().zip(arg_values.iter()) {
        collect_generic_bindings(declared, &value.borrow(), &mut bindings);
    }
    entry
        .generic_params
        .iter()
        .map(|p| bindings.get(p).cloned().unwrap_or(TypeDecl::Unknown))
        .collect()
}

/// Recursively match a declared `TypeDecl` against the actual
/// runtime value to populate `bindings` with `Generic(P) -> Type`
/// pairs. Handles the common cases (`T`, `Cell<T>`, `(T, U)`) so
/// generic struct / enum / tuple instantiations infer correctly
/// without re-running the type-checker.
fn collect_generic_bindings(
    declared: &TypeDecl,
    value: &Object,
    bindings: &mut HashMapStd<DefaultSymbol, TypeDecl>,
) {
    match declared {
        TypeDecl::Generic(sym) => {
            let runtime_ty = value.get_type();
            bindings.entry(*sym).or_insert(runtime_ty);
        }
        TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) => {
            // Pull the value's own type-arg vector if it has one.
            // Recurse element-wise so `Cell<T>` against a
            // `Cell<i64>` value resolves T = i64.
            let runtime_args = match value {
                Object::Struct { type_args, .. } => type_args.clone(),
                Object::EnumVariant { type_args, .. } => type_args.clone(),
                _ => Vec::new(),
            };
            for (decl_arg, runtime_arg) in args.iter().zip(runtime_args.iter()) {
                if let TypeDecl::Generic(sym) = decl_arg {
                    bindings.entry(*sym).or_insert(runtime_arg.clone());
                }
            }
        }
        TypeDecl::Tuple(decl_elems) => {
            if let Object::Tuple(value_elems) = value {
                for (decl, val) in decl_elems.iter().zip(value_elems.iter()) {
                    collect_generic_bindings(decl, &val.borrow(), bindings);
                }
            }
        }
        _ => {}
    }
}

/// REF-Stage-2 (i)+(iii): caller-side target of an explicit
/// `&mut <lvalue>` borrow argument. Captured at the call site so
/// the post-body parameter value can flow back into the caller's
/// storage with the right shape.
#[derive(Debug, Clone)]
pub(super) enum WritebackTarget {
    /// The argument was not `&mut <lvalue>`, or the lvalue shape
    /// isn't supported yet (e.g. `&mut arr[i]`).
    None,
    /// `&mut <name>` — write back into the local binding `name`.
    Name(DefaultSymbol),
    /// `&mut <expr>.<field>` — `obj` is the parent struct value
    /// (captured at call time so we keep a stable Rc to the
    /// underlying `Object::Struct`); `field` is the field symbol
    /// to overwrite via `borrow_mut`.
    StructField {
        obj: RcObject,
        field: DefaultSymbol,
    },
    /// `&mut <expr>.<index>` — `obj` is the parent tuple value
    /// (captured Rc to the `Object::Tuple` cell); `index` is
    /// the element position to overwrite via `borrow_mut` +
    /// indexed assignment.
    TupleElement {
        obj: RcObject,
        index: usize,
    },
    /// `&mut <name>[i]` — `obj` is the parent array value
    /// (captured Rc to the `Object::Array` cell); `index` is
    /// the position to overwrite via `borrow_mut`.
    ArrayElement {
        obj: RcObject,
        index: usize,
    },
}

impl EvaluationContext<'_> {
    /// Classify a `&mut <lvalue>` operand into a `WritebackTarget`.
    /// Walks `Expr::Identifier` and `Expr::FieldAccess` (one level
    /// from the root) — anything else (deeper chains, tuple
    /// access, index access) currently falls back to `None` so
    /// the call still runs but no writeback fires. Future phases
    /// can broaden the supported lvalue shapes.
    pub(super) fn classify_writeback_target(
        &mut self,
        operand: &ExprRef,
    ) -> Result<WritebackTarget, InterpreterError> {
        let expr = self
            .expr_pool
            .get(operand)
            .ok_or_else(|| InterpreterError::InternalError("classify_writeback_target: unbound operand".to_string()))?;
        match expr {
            Expr::Identifier(sym) => Ok(WritebackTarget::Name(sym)),
            Expr::FieldAccess(obj, field) => {
                let obj_value = self.evaluate(&obj);
                let obj_value = match obj_value {
                    Ok(EvaluationResult::Value(v)) => v,
                    Ok(_) => return Ok(WritebackTarget::None),
                    Err(e) => return Err(e),
                };
                // Coerce the Value to a `RcObject` (no-op when it
                // was already `Heap(_)`; primitives wrap into a
                // fresh cell, but field-target writeback only
                // makes sense for struct values, which always
                // ride `Heap`).
                Ok(WritebackTarget::StructField {
                    obj: obj_value.clone_to_rc(),
                    field,
                })
            }
            Expr::TupleAccess(obj, index) => {
                let obj_value = self.evaluate(&obj);
                let obj_value = match obj_value {
                    Ok(EvaluationResult::Value(v)) => v,
                    Ok(_) => return Ok(WritebackTarget::None),
                    Err(e) => return Err(e),
                };
                Ok(WritebackTarget::TupleElement {
                    obj: obj_value.clone_to_rc(),
                    index,
                })
            }
            Expr::SliceAccess(obj, info) => {
                if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                    return Ok(WritebackTarget::None);
                }
                let idx_expr = match info.start {
                    Some(e) => e,
                    None => return Ok(WritebackTarget::None),
                };
                // Evaluate the array (parent) and the index in
                // user order so any side effects in the index
                // expression run exactly once at the call site.
                let obj_value = match self.evaluate(&obj)? {
                    EvaluationResult::Value(v) => v,
                    _ => return Ok(WritebackTarget::None),
                };
                let idx_value = match self.evaluate(&idx_expr)? {
                    EvaluationResult::Value(v) => v,
                    _ => return Ok(WritebackTarget::None),
                };
                let index = match idx_value {
                    crate::value::Value::UInt64(n) => n as usize,
                    crate::value::Value::Int64(n) => n as usize,
                    crate::value::Value::UInt8(n) => n as usize,
                    crate::value::Value::UInt16(n) => n as usize,
                    crate::value::Value::UInt32(n) => n as usize,
                    crate::value::Value::Int8(n) => n as usize,
                    crate::value::Value::Int16(n) => n as usize,
                    crate::value::Value::Int32(n) => n as usize,
                    _ => return Ok(WritebackTarget::None),
                };
                Ok(WritebackTarget::ArrayElement {
                    obj: obj_value.clone_to_rc(),
                    index,
                })
            }
            _ => Ok(WritebackTarget::None),
        }
    }

    /// Apply a captured `WritebackTarget` with the post-body
    /// `value`. Identifier targets go through `set_var` /
    /// `Overwrite` (mirroring `var` reassignment); struct field
    /// targets borrow the captured `Rc` and overwrite the field
    /// in place (mirroring `obj.field = value` user code).
    pub(super) fn apply_writeback(
        &mut self,
        target: &WritebackTarget,
        value: crate::value::Value,
    ) -> Result<(), InterpreterError> {
        match target {
            WritebackTarget::None => Ok(()),
            WritebackTarget::Name(sym) => {
                let _ = self.environment.set_var(
                    *sym,
                    value,
                    crate::environment::VariableSetType::Overwrite,
                    self.string_interner,
                );
                Ok(())
            }
            WritebackTarget::StructField { obj, field } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Struct { fields, .. } => {
                        if !fields.contains_key(field) {
                            let field_name = self
                                .string_interner
                                .resolve(*field)
                                .unwrap_or("<unknown>");
                            return Err(InterpreterError::InternalError(format!(
                                "writeback: unknown field '{}'", field_name
                            )));
                        }
                        fields.insert(*field, new_value);
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not a struct: {:?}", other
                    ))),
                }
            }
            WritebackTarget::TupleElement { obj, index } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Tuple(elements) => {
                        if *index >= elements.len() {
                            return Err(InterpreterError::IndexOutOfBounds {
                                index: *index as isize,
                                size: elements.len(),
                            });
                        }
                        elements[*index] = new_value;
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not a tuple: {:?}", other
                    ))),
                }
            }
            WritebackTarget::ArrayElement { obj, index } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Array(elements) => {
                        if *index >= elements.len() {
                            return Err(InterpreterError::IndexOutOfBounds {
                                index: *index as isize,
                                size: elements.len(),
                            });
                        }
                        elements[*index] = new_value;
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not an array: {:?}", other
                    ))),
                }
            }
        }
    }

    pub(super) fn call_method(&mut self, method: Rc<MethodFunction>, self_obj: RcObject, args: Vec<RcObject>) -> Result<EvaluationResult, InterpreterError> {
        // Create new scope for method execution
        self.environment.enter_block();

        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter` (the parser
        // only flips `has_self_param=true`). Bind the `self`
        // identifier explicitly to the same Rc the caller passed
        // — RefCell semantics give reference behaviour for free,
        // so `&self` / `&mut self` / `self: Self` are runtime-
        // equivalent here. The frontend type checker enforces
        // mutability at compile time.
        let first_param_is_self = method
            .parameter
            .first()
            .and_then(|(sym, _)| self.string_interner.resolve(*sym))
            .map(|name| name == "self")
            .unwrap_or(false);
        let bind_implicit_self = method.has_self_param && !first_param_is_self;
        if bind_implicit_self {
            if let Some(self_sym) = self.string_interner.get("self") {
                self.environment.set_val(self_sym, self_obj.clone().into());
            }
        }

        // Set up method parameters
        let mut param_index = 0;

        // Bind method parameters - first parameter should be self
        // (when the source uses the `self: Self` form)
        for (param_symbol, _param_type) in &method.parameter {
            if param_index == 0 && first_param_is_self {
                // First parameter is `self: Self` - bind the object
                self.environment.set_val(*param_symbol, self_obj.clone().into());
            } else {
                // Subsequent parameters are regular args. With
                // an implicit self receiver the first method.parameter
                // entry IS the first user arg, so use param_index
                // directly when self isn't in the list.
                let arg_idx = if first_param_is_self {
                    if param_index == 0 { continue; } else { param_index - 1 }
                } else {
                    param_index
                };
                if arg_idx < args.len() {
                    self.environment.set_val(*param_symbol, args[arg_idx].clone().into());
                }
            }
            param_index += 1;
        }

        // Pre-body `requires` checks. `self` and named args are visible above.
        if let Err(e) = self.evaluate_requires_clauses(method.name, &method.requires) {
            self.environment.exit_block();
            return Err(e);
        }

        // Execute method body
        let result = self.evaluate_method(&method);

        // Post-body `ensures` checks with `result` bound to the method's
        // produced value. Skip if the body already errored or propagated a
        // non-value flow (e.g. break/continue would be a bug at this layer
        // anyway, but we don't want to mask the original error).
        let result = match result {
            Ok(EvaluationResult::Value(v)) => {
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, v.clone_to_rc()) {
                    self.environment.exit_block();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().map(|val| val.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, ret) {
                    self.environment.exit_block();
                    return Err(e);
                }
                // DICT-RETURN-WHILE follow-up: an explicit `return v`
                // inside the method body bubbles up here as
                // EvaluationResult::Return. Convert to Value at the
                // method boundary — Return is a control-flow signal
                // for the *callee's* enclosing control structure
                // (loop, block) and shouldn't leak into the
                // caller's scope. Without this, a `return` inside
                // a callee's `while` loop (now propagating
                // correctly per the `evaluate_block::While` arm
                // fix) would also unwind the *caller's* function.
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            other => other,
        };

        // Clean up scope
        self.environment.exit_block();

        result
    }

    /// Evaluate every `requires` clause for the given callable against the
    /// current environment (parameters and, for methods, `self` already
    /// bound). Returns the first violation as a ContractViolation error.
    /// No-op when the active `INTERPRETER_CONTRACTS` mode disables
    /// pre-checks. Shared by `evaluate_function_with_values`,
    /// `call_method`, and `call_associated_method`.
    fn evaluate_requires_clauses(
        &mut self,
        fn_name: DefaultSymbol,
        clauses: &[ExprRef],
    ) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_pre || clauses.is_empty() {
            return Ok(());
        }
        for (idx, cond) in clauses.iter().enumerate() {
            // Contract predicates are bool expressions; control flow
            // (Return / Break / Continue) inside them is meaningless and is
            // rejected as an internal error rather than propagated.
            let cond_res = self.evaluate(cond)?;
            let cond_obj = self.unwrap_value(cond_res)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                return Err(InterpreterError::ContractViolation {
                    kind: "requires",
                    function: self.string_interner.resolve(fn_name).unwrap_or("<unknown>").to_string(),
                    clause_index: idx,
                });
            }
        }
        Ok(())
    }

    /// Bind `result` to the callable's produced value and evaluate every
    /// `ensures` clause. The caller is responsible for cleaning up the
    /// environment block; we don't enter/exit a new scope here so the
    /// `result` binding lives in the same scope as the parameters.
    fn evaluate_ensures_clauses(
        &mut self,
        fn_name: DefaultSymbol,
        clauses: &[ExprRef],
        return_value: RcObject,
    ) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_post || clauses.is_empty() {
            return Ok(());
        }
        self.environment.set_val(self.result_symbol, (return_value).into());
        for (idx, cond) in clauses.iter().enumerate() {
            let cond_res = self.evaluate(cond)?;
            let cond_obj = self.unwrap_value(cond_res)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                return Err(InterpreterError::ContractViolation {
                    kind: "ensures",
                    function: self.string_interner.resolve(fn_name).unwrap_or("<unknown>").to_string(),
                    clause_index: idx,
                });
            }
        }
        Ok(())
    }

    /// Call an associated method (without self parameter)
    pub(super) fn call_associated_method(&mut self, method: Rc<MethodFunction>, args: Vec<RcObject>) -> Result<EvaluationResult, InterpreterError> {
        // Create new scope for method execution
        self.environment.enter_block();

        // Set up method parameters - skip self parameter for associated functions.
        // Stage 1 of `&` references: with `&self` / `&mut self` (the
        // implicit form), `self` is not in `method.parameter`, so
        // there is nothing to skip — every entry is a user arg.
        let first_param_is_self = method
            .parameter
            .first()
            .and_then(|(sym, _)| self.string_interner.resolve(*sym))
            .map(|name| name == "self")
            .unwrap_or(false);
        let skip_self = method.has_self_param && first_param_is_self;
        let mut param_index = 0;

        // Bind method parameters
        for (param_symbol, _param_type) in &method.parameter {
            if skip_self && param_index == 0 {
                // Skip self parameter for associated functions
                param_index += 1;
                continue;
            }

            let arg_index = if skip_self { param_index - 1 } else { param_index };
            if arg_index < args.len() {
                self.environment.set_val(*param_symbol, args[arg_index].clone().into());
            }
            param_index += 1;
        }

        // Same contract evaluation flow as `call_method`. Associated functions
        // have no `self`, but `requires` / `ensures` predicates may still
        // reference the named parameters and `result`.
        if let Err(e) = self.evaluate_requires_clauses(method.name, &method.requires) {
            self.environment.exit_block();
            return Err(e);
        }

        let result = self.evaluate_method(&method);

        let result = match result {
            Ok(EvaluationResult::Value(v)) => {
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, v.clone_to_rc()) {
                    self.environment.exit_block();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().map(|val| val.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, ret) {
                    self.environment.exit_block();
                    return Err(e);
                }
                // Same Return → Value boundary conversion as
                // call_method above — see that comment for the
                // full rationale (DICT-RETURN-WHILE follow-up).
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            other => other,
        };

        // Clean up scope
        self.environment.exit_block();

        result
    }

    fn evaluate_method(&mut self, method: &MethodFunction) -> Result<EvaluationResult, InterpreterError> {
        // Get the method body from the statement pool
        let stmt = self.stmt_pool.get(&method.code)
            .ok_or_else(|| InterpreterError::InternalError("Invalid method code reference".to_string()))?;

        // Execute the method body
        match stmt {
            frontend::ast::Stmt::Expression(expr_ref) => {
                if let Some(Expr::Block(statements)) = self.expr_pool.get(&expr_ref) {
                    self.evaluate_block(&statements)
                } else {
                    // Single expression method body
                    self.evaluate(&expr_ref)
                }
            }
            _ => Err(InterpreterError::InternalError(format!("evaluate_method: unexpected method body type: {stmt:?}")))
        }
    }

    /// Evaluates function calls
    pub(super) fn evaluate_function_call(&mut self, name: &DefaultSymbol, args: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Bare-name resolution: prefer the user-authored
        // `(None, name)` slot so a user `fn add(Point, Point)`
        // wins over an auto-loaded stdlib `pub fn add(u64, u64)`
        // (#193b). Falls back to the legacy flat `function` map
        // when the qualified table isn't populated (e.g. tests
        // that built the context with the older constructor).
        let resolved = self
            .lookup_function_qualified(None, *name)
            .or_else(|| self.function.get::<DefaultSymbol>(name).cloned());
        if let Some(func) = resolved {
            let args = self.expr_pool.get(&args)
                .ok_or_else(|| InterpreterError::InternalError("Invalid arguments reference".to_string()))?;
            match args {
                Expr::ExprList(args) => {
                    if args.len() != func.parameter.len() {
                        return Err(
                            InterpreterError::FunctionParameterMismatch {
                                message: format!("evaluate_function: bad function parameter length: {:?}", args.len()),
                                expected: func.parameter.len(),
                                found: args.len()
                            }
                        );
                    }

                    // Evaluate arguments once and perform type checking. Phase 5:
                    // collect as `Value` rather than `RcObject` so primitive
                    // arguments stay inline through the call boundary.
                    use crate::try_value_v;
                    let mut evaluated_args: Vec<crate::value::Value> = Vec::new();
                    let is_generic_function = !func.generic_params.is_empty();
                    // REF-Stage-2 (i): track which positional args are
                    // an explicit `&mut <name>` borrow expression so
                    // we can write the post-body parameter value back
                    // to the caller's binding once the call returns.
                    // Other arg shapes contribute `None` (no
                    // writeback target).
                    let mut writeback_targets: Vec<WritebackTarget> = Vec::with_capacity(args.len());

                    for (i, (arg_expr, (_param_name, expected_type))) in args.iter().zip(func.parameter.iter()).enumerate() {
                        // Detect `&mut <lvalue>` arg shape before
                        // evaluating, so the caller-side target is
                        // captured even when the argument expression
                        // itself mutates other state during evaluation.
                        let mut target = WritebackTarget::None;
                        if let Some(Expr::Unary(op, inner)) = self.expr_pool.get(arg_expr) {
                            if matches!(op, UnaryOp::BorrowMut) {
                                target = self.classify_writeback_target(&inner)?;
                            }
                        }
                        writeback_targets.push(target);

                        let arg_result = self.evaluate(arg_expr);
                        let arg_value = try_value_v!(arg_result);
                        let actual_type = arg_value.get_type();

                        // Skip type checking for generic functions since type checking was already done.
                        // REF-Stage-2: references are erased at runtime, so peel the
                        // expected `&T` / `&mut T` to `T` before comparing — the static
                        // type checker already enforced the call-site mut/borrow rules
                        // (REF-Stage-2 (f) requires explicit `&mut <var>` for `&mut T`
                        // parameters), so this runtime check is purely defence-in-depth
                        // against the inner value type.
                        let expected_runtime = expected_type.deref_ref();
                        if !is_generic_function && !actual_type.is_equivalent(expected_runtime) {
                            let func_name = self.string_interner.resolve(*name).unwrap_or("<unknown>");
                            return Err(InterpreterError::TypeError {
                                expected: expected_type.clone(),
                                found: actual_type,
                                message: format!("Function '{}' argument {} type mismatch", func_name, i + 1)
                            });
                        }

                        evaluated_args.push(arg_value);
                    }

                    // Call function with pre-evaluated arguments and collect
                    // post-body `&mut T` parameter values.
                    let (ret_val, writebacks) = self
                        .evaluate_function_with_values_writeback(func, &evaluated_args)?;

                    // REF-Stage-2 (i)+(iii): apply writebacks. Each
                    // entry pairs the caller-side target (identifier
                    // or struct field) with the post-body value of
                    // the corresponding `&mut T` parameter. The type
                    // checker has already enforced that the root
                    // binding is `var` (so `set_var` w/ Overwrite
                    // succeeds, and field assignment via `borrow_mut`
                    // mirrors the semantics of `obj.field = x` in
                    // user code).
                    for (target, modified) in writeback_targets.iter().zip(writebacks.iter()) {
                        if let Some(val) = modified {
                            self.apply_writeback(target, val.clone())?;
                        }
                    }

                    Ok(EvaluationResult::Value(ret_val.into()))
                }
                _ => Err(InterpreterError::InternalError("evaluate_function: expected ExprList".to_string())),
            }
        } else {
            // Closures Phase 3: indirect call. When the bare name
            // doesn't resolve to a fn decl but does resolve to a
            // local variable holding `Object::Closure`, dispatch
            // through the closure body. Mirrors the type-checker
            // fallback in `visit_call`.
            if let Some(callee_val) = self.environment.get_val(*name) {
                let callee_rc = callee_val.into_rc();
                let is_closure = matches!(&*callee_rc.borrow(), Object::Closure { .. });
                if is_closure {
                    return self.evaluate_indirect_call(callee_rc, name, args);
                }
            }
            let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
            Err(InterpreterError::FunctionNotFound(name.to_string()))
        }
    }

    /// Closures Phase 3: dispatch through an `Object::Closure` value
    /// at the given binding. Evaluates each argument expression
    /// against the caller's environment, then opens a fresh block
    /// scope, binds captures + params, and evaluates the body.
    /// Mirrors the values-based call path used by `evaluate_function`
    /// without the writeback / contract / generic-monomorphisation
    /// machinery — closures don't carry contract clauses or generic
    /// params (Phase 2 reject), so the simpler shape suffices.
    /// Closures Phase 8: dispatch a `obj.field(args)` call when
    /// the field's value is a closure. Same shape as
    /// `evaluate_indirect_call` but takes the args as a slice
    /// (method-call ABI) instead of an `ExprList` ref.
    fn evaluate_field_closure_call(
        &mut self,
        callee: RcObject,
        field_name: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        let (params, body, captures) = {
            let borrowed = callee.borrow();
            match &*borrowed {
                Object::Closure { params, body, captures, .. } => (
                    params.clone(),
                    *body,
                    captures.clone(),
                ),
                _ => return Err(InterpreterError::InternalError(
                    "evaluate_field_closure_call: callee was not a closure".to_string(),
                )),
            }
        };
        if args.len() != params.len() {
            let name_str = self
                .string_interner
                .resolve(field_name)
                .unwrap_or("<closure-field>");
            return Err(InterpreterError::FunctionParameterMismatch {
                message: format!(
                    "closure field '{}' arg count mismatch: expected {}, found {}",
                    name_str,
                    params.len(),
                    args.len()
                ),
                expected: params.len(),
                found: args.len(),
            });
        }
        let mut evaluated: Vec<crate::value::Value> = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.evaluate(arg)?;
            let v = match v {
                EvaluationResult::Value(v) => v,
                _ => {
                    return Err(InterpreterError::InternalError(
                        "field closure argument produced control-flow value".to_string(),
                    ));
                }
            };
            evaluated.push(v);
        }
        self.environment.enter_block();
        for (name, val) in &captures {
            self.environment
                .set_val(*name, crate::value::Value::from_rc(val));
        }
        for ((param_sym, _), arg_val) in params.iter().zip(evaluated.into_iter()) {
            self.environment.set_val(*param_sym, arg_val);
        }
        let body_expr = self.expr_pool.get(&body).ok_or_else(|| {
            InterpreterError::InternalError("closure body ExprRef not in pool".to_string())
        })?;
        let result = match body_expr {
            Expr::Block(stmts) => self.evaluate_block(&stmts),
            _ => self.evaluate(&body),
        };
        self.environment.exit_block();
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(EvaluationResult::Value(v)),
            Ok(EvaluationResult::Return(v)) => {
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            Ok(other) => Ok(other),
            Err(e) => Err(e),
        }
    }

    fn evaluate_indirect_call(
        &mut self,
        callee: RcObject,
        callee_name: &DefaultSymbol,
        args: &ExprRef,
    ) -> Result<EvaluationResult, InterpreterError> {
        let args_list = match self.expr_pool.get(args) {
            Some(Expr::ExprList(args)) => args,
            _ => return Err(InterpreterError::InternalError(
                "evaluate_indirect_call: expected ExprList".to_string(),
            )),
        };
        // Snapshot closure parts under a borrow + drop pattern so we
        // can mutate the environment afterwards without reborrowing.
        let (params, body, captures) = {
            let borrowed = callee.borrow();
            match &*borrowed {
                Object::Closure { params, body, captures, .. } => (
                    params.clone(),
                    *body,
                    captures.clone(),
                ),
                _ => return Err(InterpreterError::InternalError(
                    "evaluate_indirect_call: callee was not a closure".to_string(),
                )),
            }
        };
        if args_list.len() != params.len() {
            let name_str = self
                .string_interner
                .resolve(*callee_name)
                .unwrap_or("<closure>");
            return Err(InterpreterError::FunctionParameterMismatch {
                message: format!(
                    "closure '{}' arg count mismatch: expected {}, found {}",
                    name_str,
                    params.len(),
                    args_list.len()
                ),
                expected: params.len(),
                found: args_list.len(),
            });
        }
        // Evaluate args in caller scope FIRST — they may reference
        // bindings that aren't in the closure's capture set.
        let mut evaluated: Vec<crate::value::Value> = Vec::with_capacity(args_list.len());
        for arg in &args_list {
            let v = self.evaluate(arg)?;
            let v = match v {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(_)
                | EvaluationResult::Break
                | EvaluationResult::Continue
                | EvaluationResult::None => {
                    return Err(InterpreterError::InternalError(
                        "closure argument produced control-flow value".to_string(),
                    ));
                }
            };
            evaluated.push(v);
        }
        // Open a fresh scope and bind captures + params. Args take
        // precedence (a param shadowing a captured name is fine) —
        // params are inserted last so they win on lookup.
        self.environment.enter_block();
        for (name, val) in &captures {
            self.environment
                .set_val(*name, crate::value::Value::from_rc(val));
        }
        for ((param_sym, _), arg_val) in params.iter().zip(evaluated.into_iter()) {
            self.environment.set_val(*param_sym, arg_val);
        }
        // Evaluate the body. Body is a block ExprRef per the parser
        // (`parse_closure_expr` always uses `parse_block`), so this
        // exercises the standard block evaluator.
        let body_expr = self.expr_pool.get(&body).ok_or_else(|| {
            InterpreterError::InternalError("closure body ExprRef not in pool".to_string())
        })?;
        let result = match body_expr {
            Expr::Block(stmts) => self.evaluate_block(&stmts),
            _ => self.evaluate(&body),
        };
        self.environment.exit_block();
        // Convert a Return result back into a plain Value at the
        // closure boundary — the body's `return` shouldn't leak
        // into the caller's control flow.
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(EvaluationResult::Value(v)),
            Ok(EvaluationResult::Return(v)) => {
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            Ok(other) => Ok(other),
            Err(e) => Err(e),
        }
    }

    /// Evaluates field access expressions
    pub(super) fn evaluate_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        // First check if this is a module qualified name (e.g., math.add)
        if let Some(Expr::Identifier(module_name)) = self.expr_pool.get(&obj) {
            if let Some(module_value) = self.resolve_module_qualified_name(module_name, *field) {
                return Ok(EvaluationResult::Value(module_value.into()));
            }
        }

        // If not a module qualified name, evaluate as struct field access
        let obj_val = self.evaluate(obj)?;
        let obj_val = try_value!(Ok(obj_val));
        let obj_borrowed = obj_val.borrow();

        match &*obj_borrowed {
            Object::Struct { fields, .. } => {
                fields.get(field)
                    .cloned()
                    .map(|rc| EvaluationResult::Value(rc.into()))
                    .ok_or_else(|| {
                        let field_name = self
                            .string_interner
                            .resolve(*field)
                            .unwrap_or("<unknown>");
                        InterpreterError::InternalError(format!("Field '{field_name}' not found"))
                    })
            }
            _ => Err(InterpreterError::InternalError(format!("Cannot access field on non-struct object: {obj_borrowed:?}")))
        }
    }

    /// Evaluates method call expressions
    pub(super) fn evaluate_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let obj_val = self.evaluate(obj)?;
        let obj_val = try_value!(Ok(obj_val));
        let obj_borrowed = obj_val.borrow();
        let method_name = self.string_interner.resolve(*method).unwrap_or("<unknown>");

        // Handle universal is_null() method first
        if method_name == "is_null" {
            if !args.is_empty() {
                return Err(InterpreterError::InternalError(format!(
                    "is_null() method takes no arguments, but {} provided",
                    args.len()
                )));
            }
            let is_null = obj_borrowed.is_null();
            return Ok(EvaluationResult::Value((Object::Bool(is_null)).into()));
        }

        // Step B of extension-trait support: dispatch through the
        // user-registered method registry first when the receiver is
        // a primitive. Mirrors what `Object::Struct { type_name, .. }`
        // already does — looks `(target_symbol, method_name)` up in
        // `method_registry` and, on hit, evaluates args and calls the
        // method body with `self` as the first parameter.
        //
        // This runs *before* the hardcoded `Object::Int64`/`Float64`
        // arms below, so a user `impl Foo for i64 { fn abs(self) -> i64 { ... } }`
        // takes precedence over the legacy `BuiltinMethod::I64Abs`
        // path. Steps E + F migrate the legacy methods onto extension
        // traits and remove the hardcoded arms entirely.
        if let Some(target_sym) = primitive_target_symbol(&obj_borrowed, self.string_interner) {
            // Primitive receivers have no type args; pass empty
            // slice. CONCRETE-IMPL Phase 2: any future
            // `impl Foo for u8` etc. always registers with empty
            // target_type_args, so the empty-args lookup is
            // exhaustive for the primitive path.
            if let Some(method_func) = self.get_method(target_sym, *method, &[]) {
                drop(obj_borrowed);
                let mut arg_values = Vec::new();
                for arg in args {
                    let arg_val = self.evaluate(arg)?;
                    let arg_val = try_value!(Ok(arg_val));
                    arg_values.push(arg_val);
                }
                return self.call_method(method_func, obj_val, arg_values);
            }
        }

        match &*obj_borrowed {
            Object::ConstString(_) | Object::String(_) => {
                // Handle built-in String methods
                match method_name {
                    "len" => {
                        // String.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.len() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        // Get the actual string value regardless of internal representation
                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let len = string_value.len() as u64;

                        Ok(EvaluationResult::Value((Object::UInt64(len)).into()))
                    }
                    "contains" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.contains() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(&self.string_interner);

                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = try_value!(Ok(arg_value));
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);

                        let contains = string_value.contains(&arg_string);
                        Ok(EvaluationResult::Value((Object::Bool(contains)).into()))
                    }
                    "concat" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.concat() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(&self.string_interner);

                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = try_value!(Ok(arg_value));
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);

                        let concatenated = format!("{}{}", string_value, arg_string);
                        // Return as dynamic String, not interned - this is the key improvement
                        Ok(EvaluationResult::Value((Object::String(concatenated)).into()))
                    }
                    "trim" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.trim() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let trimmed = string_value.trim().to_string();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(trimmed)).into()))
                    }
                    "to_upper" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_upper() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let upper = string_value.to_uppercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(upper)).into()))
                    }
                    "to_lower" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_lower() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(&self.string_interner);
                        let lower = string_value.to_lowercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(lower)).into()))
                    }
                    _ => {
                        Err(InterpreterError::InternalError(format!(
                            "Method '{method_name}' not found for String type"
                        )))
                    }
                }
            }
            Object::Array(elements) => {
                // Handle built-in Array methods
                match method_name {
                    "len" => {
                        // Array.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "Array.len() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        let len = elements.len() as u64;
                        Ok(EvaluationResult::Value((Object::UInt64(len)).into()))
                    }
                    _ => {
                        Err(InterpreterError::InternalError(format!(
                            "Method '{method_name}' not found for Array type"
                        )))
                    }
                }
            }
            // NOTE: hardcoded `Object::Int64.abs()` /
            // `Object::Float64.{abs,sqrt}` arms lived here before
            // Step F. The Step B primitive-receiver dispatch path
            // earlier in this function intercepts these calls and
            // routes through the prelude's extension-trait impls
            // (`impl Abs for i64 { fn abs(self) -> i64 { ... } }`
            // / `impl Sqrt for f64 { ... }`). The arms below are
            // unreachable for `abs` / `sqrt` now; they only fire
            // when a user calls some unknown method on a
            // primitive, which produces the same "method not
            // found" diagnostic as before.
            Object::Int64(_) => Err(InterpreterError::InternalError(format!(
                "Method '{method_name}' not found for i64"
            ))),
            Object::Float64(_) => Err(InterpreterError::InternalError(format!(
                "Method '{method_name}' not found for f64"
            ))),
            Object::Struct { type_name, type_args, .. } => {
                let struct_name_symbol = *type_name;
                // CONCRETE-IMPL Phase 2: dispatch picks the impl
                // matching the receiver's concrete type args
                // (e.g. `impl FromStr for Vec<u8>` for a `Vec<u8>`),
                // falling back to a generic-parameterised impl with
                // empty target_type_args.
                let receiver_type_args = type_args.clone();

                if let Some(method_func) = self.get_method(struct_name_symbol, *method, &receiver_type_args) {
                    drop(obj_borrowed); // Release borrow before method call

                    // Evaluate method arguments
                    let mut arg_values = Vec::new();
                    for arg in args {
                        let arg_val = self.evaluate(arg)?;
                        let arg_val = try_value!(Ok(arg_val));
                        arg_values.push(arg_val);
                    }

                    // Call method with self as first argument
                    self.call_method(method_func, obj_val, arg_values)
                } else {
                    // Closures Phase 8: when no method matches,
                    // try the field-call fallback. If the struct
                    // has a field whose name matches and whose
                    // value is a closure (`Object::Closure`),
                    // dispatch through the indirect-call path —
                    // the same one a `val f = fn(...); f(x)`
                    // call would take.
                    if let Object::Struct { fields, .. } = &*obj_borrowed {
                        if let Some(field_rc) = fields.get(method).cloned() {
                            let is_closure = matches!(
                                &*field_rc.borrow(),
                                Object::Closure { .. }
                            );
                            if is_closure {
                                drop(obj_borrowed);
                                return self.evaluate_field_closure_call(
                                    field_rc, *method, args,
                                );
                            }
                        }
                    }
                    Err(InterpreterError::InternalError(format!("Method '{method_name}' not found for struct '{type_name:?}'")))
                }
            }
            Object::EnumVariant { enum_name, type_args, .. } => {
                // Enum receivers reuse the same `(target_symbol,
                // method_name)` `method_registry` lookup the struct
                // path uses; `impl<T> Option<T> { fn unwrap_or(...) }`
                // registers under the enum's name symbol. Mirrors how
                // primitive extension-trait dispatch piggy-backs on
                // the same registry above. CONCRETE-IMPL Phase 2:
                // pass enum's runtime type_args so any future
                // `impl Foo for Option<u8>` would dispatch correctly;
                // generic `impl<T> Option<T>` falls through with
                // empty target_type_args.
                let enum_name_symbol = *enum_name;
                let receiver_type_args = type_args.clone();
                if let Some(method_func) = self.get_method(enum_name_symbol, *method, &receiver_type_args) {
                    drop(obj_borrowed);
                    let mut arg_values = Vec::new();
                    for arg in args {
                        let arg_val = self.evaluate(arg)?;
                        let arg_val = try_value!(Ok(arg_val));
                        arg_values.push(arg_val);
                    }
                    self.call_method(method_func, obj_val, arg_values)
                } else {
                    Err(InterpreterError::InternalError(format!(
                        "Method '{method_name}' not found for enum '{enum_name:?}'"
                    )))
                }
            }
            _ => {
                Err(InterpreterError::InternalError(format!("Cannot call method '{method_name}' on non-struct object: {obj_borrowed:?}")))
            }
        }
    }

    /// Evaluates struct literal expressions
    pub(super) fn evaluate_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &[(DefaultSymbol, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        // Create a struct instance. Field keys flow through unchanged as
        // interned `DefaultSymbol`s — there is no need to resolve to a
        // textual name during construction.
        let mut field_values: HashMap<DefaultSymbol, RcObject> = HashMap::new();

        for (field_name, field_expr) in fields {
            // Handle null expressions specially in struct literals
            let expr = self.expr_pool.get(&field_expr)
                .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", field_expr)))?;

            let field_value = match expr {
                Expr::Null => {
                    // Use pre-created null object for struct fields
                    self.null_object.clone()
                }
                _ => {
                    let field_value = self.evaluate(field_expr)?;
                    try_value!(Ok(field_value))
                }
            };

            field_values.insert(*field_name, field_value);
        }

        let type_args = self
            .struct_definitions
            .get(struct_name)
            .map(|entry| derive_struct_type_args(entry, &field_values))
            .unwrap_or_default();
        let struct_obj = Object::Struct {
            type_name: *struct_name,
            fields: Box::new(field_values),
            type_args,
        };

        Ok(EvaluationResult::Value((struct_obj).into()))
    }

    /// Evaluates associated function calls (like Container::new)
    pub(super) fn evaluate_associated_function_call(&mut self, struct_name: &DefaultSymbol, function_name: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        // Enum tuple-variant construction: `Enum::Variant(args)` shares parse
        // structure with associated function calls. Intercept it here before
        // falling through to struct method dispatch.
        if let Some(entry) = self.enum_definitions.get(struct_name).cloned() {
            if let Some(variant) = entry.variants.iter().find(|v| v.name == *function_name) {
                let mut arg_values = Vec::new();
                for arg_expr in args {
                    let arg_value = self.evaluate(arg_expr)?;
                    let arg_obj = try_value!(Ok(arg_value));
                    arg_values.push(arg_obj);
                }
                let type_args = derive_enum_type_args(
                    &entry,
                    variant,
                    &arg_values,
                );
                let obj = Object::EnumVariant {
                    enum_name: *struct_name,
                    variant_name: *function_name,
                    values: arg_values,
                    type_args,
                };
                return Ok(EvaluationResult::Value((obj).into()));
            }
        }

        // Convert struct_name and function_name to strings for lookup and clone them to avoid borrow issues
        let struct_name_str = self.string_interner.resolve(*struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Struct name {:?} not found in string interner", struct_name)))?
            .to_string();

        let function_name_str = self.string_interner.resolve(*function_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Function name {:?} not found in string interner", function_name)))?
            .to_string();

        // Evaluate arguments first
        let mut arg_values = Vec::new();
        for arg_expr in args {
            let arg_value = self.evaluate(arg_expr)?;
            let arg_obj = try_value!(Ok(arg_value));
            arg_values.push(arg_obj);
        }

        // Call the associated function as if it's a static method
        // This is similar to call_struct_method but without self
        self.call_associated_function(*struct_name, *function_name, &arg_values, &struct_name_str, &function_name_str)
    }

    /// Look up an `extern fn` in the registry and invoke it. Surfaces
    /// a targeted "not yet implemented" error when no Rust impl is
    /// registered for the declared name. Shared by both the
    /// `evaluate_function` (RcObject-result) and
    /// `evaluate_function_with_values` (Value-result) call paths.
    fn dispatch_extern_fn(
        &mut self,
        function: &Rc<Function>,
        args: &[crate::value::Value],
    ) -> Result<crate::value::Value, InterpreterError> {
        let name = self
            .string_interner
            .resolve(function.name)
            .ok_or_else(|| InterpreterError::InternalError(
                "extern fn name failed to resolve in interner".to_string(),
            ))?;
        match self.extern_registry.get(name) {
            Some(impl_fn) => impl_fn(args),
            None => Err(InterpreterError::FunctionNotFound(format!(
                "extern fn `{name}` is not yet implemented in the interpreter"
            ))),
        }
    }

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &[ExprRef]) -> Result<RcObject, InterpreterError> {
        if function.is_extern {
            // Evaluate args eagerly, then route to the extern dispatch
            // shared with the values-based call path. Keeps the
            // implementation lookup in one place.
            let mut arg_values: Vec<crate::value::Value> = Vec::with_capacity(args.len());
            for arg_expr in args {
                let result = self.evaluate(arg_expr)?;
                let v = match result {
                    EvaluationResult::Value(v) => v,
                    EvaluationResult::Return(_)
                    | EvaluationResult::Break
                    | EvaluationResult::Continue
                    | EvaluationResult::None => {
                        return Err(InterpreterError::InternalError(
                            "extern fn argument produced control-flow value".to_string(),
                        ));
                    }
                };
                arg_values.push(v);
            }
            return self.dispatch_extern_fn(&function, &arg_values).map(|v| v.into_rc());
        }
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
        };

        self.environment.enter_block();
        for (i, arg) in args.iter().enumerate() {
            let name = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?.0;
            let value: RcObject = match self.evaluate(arg) {
                Ok(EvaluationResult::Value(v)) => v.into_rc(),
                Ok(EvaluationResult::Return(v)) => {
                    self.environment.exit_block();
                    return Ok(v.map(|x| x.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))));
                },
                Ok(EvaluationResult::Break) | Ok(EvaluationResult::Continue) => {
                    self.environment.exit_block();
                    return Ok(Rc::new(RefCell::new(Object::Unit)));
                },
                Ok(EvaluationResult::None) => Rc::new(RefCell::new(Object::Unit)),
                Err(e) => {
                    self.environment.exit_block();
                    return Err(e);
                },
            };
            self.environment.set_val(name, (value).into());
        }

        let res = self.evaluate_block(&block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v.into_rc(),
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.map(|x| x.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }

    /// Evaluates function with pre-evaluated argument values (used when type checking has already been done).
    /// Phase 5: takes `&[Value]` and returns `Value` so primitive arguments
    /// and return values stay inline through the call boundary.
    pub fn evaluate_function_with_values(&mut self, function: Rc<Function>, args: &[crate::value::Value]) -> Result<crate::value::Value, InterpreterError> {
        // Forwarding form for callers that don't care about the
        // post-body value of `&mut T` parameters. The full form
        // (used by `evaluate_function_call`) collects writebacks
        // so the caller can propagate mutations back to the
        // borrowed locals (REF-Stage-2 (i)).
        let (val, _writebacks) = self.evaluate_function_with_values_writeback(function, args)?;
        Ok(val)
    }

    /// Like `evaluate_function_with_values` but also returns the
    /// post-body value of every `&mut T` parameter, indexed by
    /// parameter position. The returned Vec has the same length
    /// as `function.parameter`; entries for non-mut-ref params are
    /// `None`. The caller (`evaluate_function_call`) pairs these
    /// with the original `&mut <name>` arg expression to write
    /// the modified value back to the caller's binding —
    /// REF-Stage-2 (i) interpreter mutation propagation.
    pub fn evaluate_function_with_values_writeback(
        &mut self,
        function: Rc<Function>,
        args: &[crate::value::Value],
    ) -> Result<(crate::value::Value, Vec<Option<crate::value::Value>>), InterpreterError> {
        if function.is_extern {
            // Extern fns can't take `&mut T` parameters that need
            // writeback (the runtime registry signature doesn't
            // expose them), so an empty writeback list is correct.
            let v = self.dispatch_extern_fn(&function, args)?;
            return Ok((v, vec![None; function.parameter.len()]));
        }
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
        };

        self.environment.enter_block();
        // Track which params are `&mut T` so we can snapshot their
        // post-body values just before `exit_block` clears the
        // function's scope.
        let mut mut_ref_params: Vec<Option<DefaultSymbol>> = Vec::with_capacity(args.len());
        for (i, value) in args.iter().enumerate() {
            let param = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?;
            let is_mut_ref = matches!(
                &param.1,
                frontend::type_decl::TypeDecl::Ref { is_mut: true, .. }
            );
            if is_mut_ref {
                self.environment.set_val_mutable(param.0, value.clone());
                mut_ref_params.push(Some(param.0));
            } else {
                self.environment.set_val(param.0, value.clone());
                mut_ref_params.push(None);
            }
        }

        // Pre-body `requires` checks. Shares the same helper as the method
        // path, so contract evaluation behaves identically across function
        // and method calls.
        if let Err(e) = self.evaluate_requires_clauses(function.name, &function.requires) {
            self.environment.exit_block();
            return Err(e);
        }

        let res = self.evaluate_block(&block)?;

        let return_value: crate::value::Value = if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            crate::value::Value::Unit
        } else {
            match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => crate::value::Value::Unit,
                EvaluationResult::Return(v) => v.unwrap_or_else(crate::value::Value::null_unknown),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => crate::value::Value::Unit,
            }
        };

        // Post-body `ensures` checks with `result` bound to the return value.
        // The contract helper still takes `RcObject`; bridge the value once.
        if let Err(e) = self.evaluate_ensures_clauses(function.name, &function.ensures, return_value.clone_to_rc()) {
            self.environment.exit_block();
            return Err(e);
        }

        // REF-Stage-2 (i): snapshot the post-body value of each
        // `&mut T` parameter BEFORE `exit_block` discards the
        // function scope. The caller (`evaluate_function_call`)
        // applies these to the corresponding caller-side bindings.
        let writebacks: Vec<Option<crate::value::Value>> = mut_ref_params
            .iter()
            .map(|maybe_sym| maybe_sym.and_then(|s| self.environment.get_val(s)))
            .collect();

        self.environment.exit_block();
        Ok((return_value, writebacks))
    }

    /// Call a struct method by name
    pub fn call_struct_method(
        &mut self,
        object: RcObject,
        method_name: DefaultSymbol,
        args: &[RcObject],
        struct_name: &str
    ) -> Result<EvaluationResult, InterpreterError> {
        // Look for the method in the function map first
        if let Some(method_func) = self.function.get(&method_name).cloned() {
            // This is a regular function, call it directly. Convert
            // legacy `RcObject` arguments to `Value` at the boundary.
            let mut method_args: Vec<crate::value::Value> = vec![object.into()];
            method_args.extend(args.iter().cloned().map(Into::into));
            let result = self.evaluate_function_with_values(method_func, &method_args)?;
            return Ok(EvaluationResult::Value(result.into()));
        }

        // Look for struct method. CONCRETE-IMPL Phase 2:
        // `call_struct_method` is hit by `call.rs` paths that have
        // an `RcObject` receiver but no compile-time type args
        // hint; extract type_args from the receiver itself when
        // it's a struct/enum so concrete-impls dispatch correctly.
        let struct_symbol = self.string_interner.get(struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unknown struct: {}", struct_name)))?;
        let receiver_type_args: Vec<TypeDecl> = match &*object.borrow() {
            Object::Struct { type_args, .. } => type_args.clone(),
            Object::EnumVariant { type_args, .. } => type_args.clone(),
            _ => Vec::new(),
        };

        if let Some(method) = self.get_method(struct_symbol, method_name, &receiver_type_args) {
            let method_args = args.to_vec();
            return self.call_method(method, object, method_args);
        }

        Err(InterpreterError::FunctionNotFound(
            format!("Method '{}' not found for struct '{}'",
                    self.string_interner.resolve(method_name).unwrap_or("<unknown>"),
                    struct_name)
        ))
    }

    /// Call an associated function (static method) by name
    pub fn call_associated_function(
        &mut self,
        struct_name: DefaultSymbol,
        function_name: DefaultSymbol,
        args: &[RcObject],
        struct_name_str: &str,
        function_name_str: &str
    ) -> Result<EvaluationResult, InterpreterError> {
        // Look for the associated function in the function map first
        // (as a regular function). #193b: try the module-qualified
        // slot `(Some(struct_name), function_name)` first so
        // `math::add(...)` resolves to the stdlib's u64 version
        // even when a user `fn add(Point, Point)` exists. Falls
        // back to the bare-name lookup, then to the legacy flat
        // map for back-compat.
        let resolved = self
            .lookup_function_qualified(Some(struct_name), function_name)
            .or_else(|| self.lookup_function_qualified(None, function_name))
            .or_else(|| self.function.get(&function_name).cloned());
        if let Some(func) = resolved {
            // This is a regular function, call it directly without self.
            // Bridge `RcObject` args to `Value` at the boundary.
            let value_args: Vec<crate::value::Value> = args.iter().cloned().map(Into::into).collect();
            let result = self.evaluate_function_with_values(func, &value_args)?;
            return Ok(EvaluationResult::Value(result.into()));
        }

        // Look for associated function in struct methods (but call
        // without self). CONCRETE-IMPL Phase 2: associated function
        // calls (`Vec::from_str(...)`) have no receiver to read
        // type_args off of; pass empty so the generic-parameterised
        // impl is preferred. The caller-side annotation hint
        // (`var v: Vec<u8> = ...`) isn't threaded into this layer
        // yet — that's a Phase 2b refinement.
        if let Some(method) = self.get_method(struct_name, function_name, &[]) {
            return self.call_associated_method(method, args.to_vec());
        }

        Err(InterpreterError::FunctionNotFound(
            format!("Associated function '{}' not found for struct '{}'",
                    function_name_str, struct_name_str)
        ))
    }
}
