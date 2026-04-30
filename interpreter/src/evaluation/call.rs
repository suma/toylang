use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EvaluationContext, EvaluationResult};

impl EvaluationContext<'_> {
    pub(super) fn call_method(&mut self, method: Rc<MethodFunction>, self_obj: RcObject, args: Vec<RcObject>) -> Result<EvaluationResult, InterpreterError> {
        // Create new scope for method execution
        self.environment.enter_block();

        // Set up method parameters
        let mut param_index = 0;

        // Bind method parameters - first parameter should be self
        for (param_symbol, _param_type) in &method.parameter {
            if param_index == 0 {
                // First parameter is 'self' - bind the object
                self.environment.set_val(*param_symbol, (self_obj.clone().into()));
            } else if param_index - 1 < args.len() {
                // Subsequent parameters are regular args
                self.environment.set_val(*param_symbol, (args[param_index - 1].clone().into()));
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
                Ok(EvaluationResult::Return(v))
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

        // Set up method parameters - skip self parameter for associated functions
        let mut param_index = 0;
        let skip_self = method.has_self_param;

        // Bind method parameters
        for (param_symbol, _param_type) in &method.parameter {
            if skip_self && param_index == 0 {
                // Skip self parameter for associated functions
                param_index += 1;
                continue;
            }

            let arg_index = if skip_self { param_index - 1 } else { param_index };
            if arg_index < args.len() {
                self.environment.set_val(*param_symbol, (args[arg_index].clone().into()));
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
                Ok(EvaluationResult::Return(v))
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
        if let Some(func) = self.function.get::<DefaultSymbol>(name).cloned() {
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

                        // Skip type checking for generic functions since type checking was already done
                        if !is_generic_function && !actual_type.is_equivalent(expected_type) {
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
            Object::Struct { type_name, .. } => {
                let struct_name_symbol = *type_name;

                if let Some(method_func) = self.get_method(struct_name_symbol, *method) {
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

        let struct_obj = Object::Struct {
            type_name: *struct_name,
            fields: Box::new(field_values),
        };

        Ok(EvaluationResult::Value((struct_obj).into()))
    }

    /// Evaluates associated function calls (like Container::new)
    pub(super) fn evaluate_associated_function_call(&mut self, struct_name: &DefaultSymbol, function_name: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        // Enum tuple-variant construction: `Enum::Variant(args)` shares parse
        // structure with associated function calls. Intercept it here before
        // falling through to struct method dispatch.
        if let Some(variants) = self.enum_definitions.get(struct_name).cloned() {
            if variants.iter().any(|(name, _)| name == function_name) {
                let mut arg_values = Vec::new();
                for arg_expr in args {
                    let arg_value = self.evaluate(arg_expr)?;
                    let arg_obj = try_value!(Ok(arg_value));
                    arg_values.push(arg_obj);
                }
                let obj = Object::EnumVariant {
                    enum_name: *struct_name,
                    variant_name: *function_name,
                    values: arg_values,
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

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &[ExprRef]) -> Result<RcObject, InterpreterError> {
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

        // Look for struct method
        let struct_symbol = self.string_interner.get(struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unknown struct: {}", struct_name)))?;

        if let Some(struct_methods) = self.method_registry.get(&struct_symbol) {
            if let Some(method) = struct_methods.get(&method_name) {
                let method_args = args.to_vec();
                return self.call_method(method.clone(), object, method_args);
            }
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
        // Look for the associated function in the function map first (as a regular function)
        if let Some(func) = self.function.get(&function_name).cloned() {
            // This is a regular function, call it directly without self.
            // Bridge `RcObject` args to `Value` at the boundary.
            let value_args: Vec<crate::value::Value> = args.iter().cloned().map(Into::into).collect();
            let result = self.evaluate_function_with_values(func, &value_args)?;
            return Ok(EvaluationResult::Value(result.into()));
        }

        // Look for associated function in struct methods (but call without self)
        if let Some(struct_methods) = self.method_registry.get(&struct_name) {
            if let Some(method) = struct_methods.get(&function_name) {
                // For associated functions, we don't pass self, just the arguments
                return self.call_associated_method(method.clone(), args.to_vec());
            }
        }

        Err(InterpreterError::FunctionNotFound(
            format!("Associated function '{}' not found for struct '{}'",
                    function_name_str, struct_name_str)
        ))
    }
}
