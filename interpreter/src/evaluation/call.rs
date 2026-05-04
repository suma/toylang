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

impl EvaluationContext<'_> {
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

                    for (i, (arg_expr, (_param_name, expected_type))) in args.iter().zip(func.parameter.iter()).enumerate() {
                        let arg_result = self.evaluate(arg_expr);
                        let arg_value = try_value_v!(arg_result);
                        let actual_type = arg_value.get_type();

                        // Skip type checking for generic functions since type checking was already done.
                        // REF-Stage-2: use is_arg_compatible so `T` can be passed to a `&T` / `&mut T`
                        // parameter via auto-borrow at the call site (the runtime value type is the
                        // bare inner type — references are erased here).
                        if !is_generic_function && !TypeDecl::is_arg_compatible(&actual_type, expected_type) {
                            let func_name = self.string_interner.resolve(*name).unwrap_or("<unknown>");
                            return Err(InterpreterError::TypeError {
                                expected: expected_type.clone(),
                                found: actual_type,
                                message: format!("Function '{}' argument {} type mismatch", func_name, i + 1)
                            });
                        }

                        evaluated_args.push(arg_value);
                    }

                    // Call function with pre-evaluated arguments
                    Ok(EvaluationResult::Value(self.evaluate_function_with_values(func, &evaluated_args)?.into()))
                }
                _ => Err(InterpreterError::InternalError("evaluate_function: expected ExprList".to_string())),
            }
        } else {
            let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
            Err(InterpreterError::FunctionNotFound(name.to_string()))
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
        if function.is_extern {
            return self.dispatch_extern_fn(&function, args);
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
        for (i, value) in args.iter().enumerate() {
            let name = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?.0;
            self.environment.set_val(name, value.clone());
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

        self.environment.exit_block();
        Ok(return_value)
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
