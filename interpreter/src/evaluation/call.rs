use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
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
                self.environment.set_val(*param_symbol, self_obj.clone());
            } else if param_index - 1 < args.len() {
                // Subsequent parameters are regular args
                self.environment.set_val(*param_symbol, args[param_index - 1].clone());
            }
            param_index += 1;
        }

        // Pre-body `requires` checks. `self` and named args are visible above.
        if let Err(e) = self.evaluate_method_requires(&method) {
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
                if let Err(e) = self.evaluate_method_ensures(&method, v.clone()) {
                    self.environment.exit_block();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_method_ensures(&method, ret) {
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

    /// Evaluate every `requires` clause on the given method against the
    /// current environment (parameters and `self` already bound). Returns
    /// the first violation as a ContractViolation error. No-op when the
    /// active `INTERPRETER_CONTRACTS` mode disables pre-checks.
    fn evaluate_method_requires(&mut self, method: &MethodFunction) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_pre {
            return Ok(());
        }
        for (idx, cond) in method.requires.iter().enumerate() {
            let cond_val = self.evaluate(cond);
            let cond_obj = self.extract_value(cond_val)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                let fname = self.string_interner.resolve(method.name).unwrap_or("<unknown>").to_string();
                return Err(InterpreterError::ContractViolation {
                    kind: "requires",
                    function: fname,
                    clause_index: idx,
                });
            }
        }
        Ok(())
    }

    /// Bind `result` to the method's produced value and evaluate every
    /// `ensures` clause. The caller is responsible for cleaning up the
    /// environment block; we don't enter/exit a new scope here so the
    /// `result` binding lives in the same scope as the parameters.
    fn evaluate_method_ensures(&mut self, method: &MethodFunction, return_value: RcObject) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_post || method.ensures.is_empty() {
            return Ok(());
        }
        let result_sym = self.string_interner.get_or_intern("result");
        self.environment.set_val(result_sym, return_value);
        for (idx, cond) in method.ensures.iter().enumerate() {
            let cond_val = self.evaluate(cond);
            let cond_obj = self.extract_value(cond_val)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                let fname = self.string_interner.resolve(method.name).unwrap_or("<unknown>").to_string();
                return Err(InterpreterError::ContractViolation {
                    kind: "ensures",
                    function: fname,
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
                self.environment.set_val(*param_symbol, args[arg_index].clone());
            }
            param_index += 1;
        }

        // Same contract evaluation flow as `call_method`. Associated functions
        // have no `self`, but `requires` / `ensures` predicates may still
        // reference the named parameters and `result`.
        if let Err(e) = self.evaluate_method_requires(&method) {
            self.environment.exit_block();
            return Err(e);
        }

        let result = self.evaluate_method(&method);

        let result = match result {
            Ok(EvaluationResult::Value(v)) => {
                if let Err(e) = self.evaluate_method_ensures(&method, v.clone()) {
                    self.environment.exit_block();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_method_ensures(&method, ret) {
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

                    // Evaluate arguments once and perform type checking
                    let mut evaluated_args = Vec::new();
                    let is_generic_function = !func.generic_params.is_empty();

                    for (i, (arg_expr, (_param_name, expected_type))) in args.iter().zip(func.parameter.iter()).enumerate() {
                        let arg_result = self.evaluate(arg_expr)?;
                        let arg_value = self.extract_value(Ok(arg_result))?;
                        let actual_type = arg_value.borrow().get_type();

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
                    Ok(EvaluationResult::Value(self.evaluate_function_with_values(func, &evaluated_args)?))
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
                return Ok(EvaluationResult::Value(module_value));
            }
        }

        // If not a module qualified name, evaluate as struct field access
        let obj_val = self.evaluate(obj)?;
        let obj_val = self.extract_value(Ok(obj_val))?;
        let obj_borrowed = obj_val.borrow();

        match &*obj_borrowed {
            Object::Struct { fields, .. } => {
                let field_name = self.string_interner.resolve(*field)
                    .ok_or_else(|| InterpreterError::InternalError("Field name not found in string interner".to_string()))?;

                fields.get(field_name)
                    .cloned()
                    .map(EvaluationResult::Value)
                    .ok_or_else(|| InterpreterError::InternalError(format!("Field '{field_name}' not found")))
            }
            _ => Err(InterpreterError::InternalError(format!("Cannot access field on non-struct object: {obj_borrowed:?}")))
        }
    }

    /// Evaluates method call expressions
    pub(super) fn evaluate_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let obj_val = self.evaluate(obj)?;
        let obj_val = self.extract_value(Ok(obj_val))?;
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
            return Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(is_null)))));
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

                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(len)))))
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
                        let arg_obj = self.extract_value(Ok(arg_value))?;
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);

                        let contains = string_value.contains(&arg_string);
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(contains)))))
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
                        let arg_obj = self.extract_value(Ok(arg_value))?;
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(&self.string_interner);

                        let concatenated = format!("{}{}", string_value, arg_string);
                        // Return as dynamic String, not interned - this is the key improvement
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(concatenated)))))
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
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(trimmed)))))
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
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(upper)))))
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
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(lower)))))
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
                        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(len)))))
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
                        let arg_val = self.extract_value(Ok(arg_val))?;
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
        // Create a struct instance
        let mut field_values = HashMap::new();

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
                    self.extract_value(Ok(field_value))?
                }
            };

            let field_name_str = self.string_interner.resolve(*field_name).unwrap_or("unknown").to_string();
            field_values.insert(field_name_str, field_value);
        }

        let struct_obj = Object::Struct {
            type_name: *struct_name,
            fields: Box::new(field_values),
        };

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(struct_obj))))
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
                    let arg_obj = self.extract_value(Ok(arg_value))?;
                    arg_values.push(arg_obj);
                }
                let obj = Object::EnumVariant {
                    enum_name: *struct_name,
                    variant_name: *function_name,
                    values: arg_values,
                };
                return Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))));
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
            let arg_obj = self.extract_value(Ok(arg_value))?;
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
            let value = match self.evaluate(arg) {
                Ok(EvaluationResult::Value(v)) => v,
                Ok(EvaluationResult::Return(v)) => {
                    self.environment.exit_block();
                    return Ok(v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))));
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
            self.environment.set_val(name, value);
        }

        let res = self.evaluate_block(&block)?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            Ok(match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            })
        }
    }

    /// Evaluates function with pre-evaluated argument values (used when type checking has already been done)
    pub fn evaluate_function_with_values(&mut self, function: Rc<Function>, args: &[RcObject]) -> Result<RcObject, InterpreterError> {
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

        // Evaluate `requires` clauses with parameters in scope, before the body.
        // A false predicate aborts the call with ContractViolation; the env block
        // is unwound by the early return path's stack drop. Skipped entirely when
        // `INTERPRETER_CONTRACTS=post|off` so the predicates don't even evaluate
        // (matching D's `-release` semantics).
        if self.contract_mode.check_pre {
            for (idx, cond) in function.requires.iter().enumerate() {
                let cond_val = self.evaluate(cond);
                let cond_obj = self.extract_value(cond_val)?;
                let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
                if !passed {
                    self.environment.exit_block();
                    let fname = self.string_interner.resolve(function.name).unwrap_or("<unknown>").to_string();
                    return Err(InterpreterError::ContractViolation {
                        kind: "requires",
                        function: fname,
                        clause_index: idx,
                    });
                }
            }
        }

        let res = self.evaluate_block(&block)?;

        let return_value: RcObject = if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Rc::new(RefCell::new(Object::Unit))
        } else {
            match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => Rc::new(RefCell::new(Object::Unit)),
                EvaluationResult::Return(v) => v.unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))),
                EvaluationResult::Break | EvaluationResult::Continue | EvaluationResult::None => Rc::new(RefCell::new(Object::Unit)),
            }
        };

        // Evaluate `ensures` clauses with `result` bound to the return value.
        // Parameters are still in scope from the entry-time bindings above; the
        // type checker only allows `result` and parameters in postconditions.
        // Skipped under `INTERPRETER_CONTRACTS=pre|off`.
        if self.contract_mode.check_post && !function.ensures.is_empty() {
            let result_sym = self.string_interner.get_or_intern("result");
            self.environment.set_val(result_sym, return_value.clone());
            for (idx, cond) in function.ensures.iter().enumerate() {
                let cond_val = self.evaluate(cond);
                let cond_obj = self.extract_value(cond_val)?;
                let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
                if !passed {
                    self.environment.exit_block();
                    let fname = self.string_interner.resolve(function.name).unwrap_or("<unknown>").to_string();
                    return Err(InterpreterError::ContractViolation {
                        kind: "ensures",
                        function: fname,
                        clause_index: idx,
                    });
                }
            }
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
            // This is a regular function, call it directly
            let mut method_args = vec![object];
            method_args.extend_from_slice(args);
            let result = self.evaluate_function_with_values(method_func, &method_args)?;
            return Ok(EvaluationResult::Value(result));
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
            // This is a regular function, call it directly without self
            let result = self.evaluate_function_with_values(func, args)?;
            return Ok(EvaluationResult::Value(result));
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
