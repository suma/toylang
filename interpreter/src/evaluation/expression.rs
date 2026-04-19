use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::{Object, ObjectKey, RcObject};
use crate::error::InterpreterError;
use super::{convert_object, EvaluationContext, EvaluationResult};

impl EvaluationContext<'_> {
    pub fn evaluate(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Check recursion depth to prevent stack overflow
        if self.recursion_depth >= self.max_recursion_depth {
            return Err(InterpreterError::InternalError(
                "Maximum recursion depth reached in expression evaluation - possible circular reference".to_string()
            ));
        }

        self.recursion_depth += 1;
        let result = self.evaluate_impl(e);
        self.recursion_depth -= 1;
        result
    }

    fn evaluate_impl(&mut self, e: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        let expr = self.expr_pool.get(e)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", e)))?;
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                self.evaluate_binary(&op, &lhs, &rhs)
            }
            Expr::Unary(op, operand) => {
                self.evaluate_unary(&op, &operand)
            }
            Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) | Expr::True | Expr::False => {
                self.evaluate_literal(&expr)
            }
            Expr::Number(_v) => {
                // Type-unspecified numbers should be resolved during type checking
                Err(InterpreterError::InternalError("Expr::Number should be transformed to concrete type during type checking".to_string()))
            }
            Expr::Identifier(s) => {
                let val = self.environment.get_val(s)
                    .ok_or_else(|| InterpreterError::UndefinedVariable(format!("Variable not found: {s:?}")))?;
                Ok(EvaluationResult::Value(val))
            }
            Expr::IfElifElse(cond, then, elif_pairs, _else) => {
                self.evaluate_if_elif_else(&cond, &then, &elif_pairs, &_else)
            }
            Expr::Call(name, args) => {
                self.evaluate_function_call(&name, &args)
            }
            Expr::ArrayLiteral(elements) => {
                self.evaluate_array_literal(&elements)
            }
            Expr::FieldAccess(obj, field) => {
                self.evaluate_field_access(&obj, &field)
            }
            Expr::MethodCall(obj, method, args) => {
                self.evaluate_method_call(&obj, &method, &args)
            }
            Expr::BuiltinMethodCall(receiver, method, args) => {
                self.evaluate_builtin_method_call(&receiver, &method, &args)
            }
            Expr::BuiltinCall(func, args) => {
                self.evaluate_builtin_call(&func, &args)
            }
            Expr::StructLiteral(struct_name, fields) => {
                self.evaluate_struct_literal(&struct_name, &fields)
            }
            Expr::QualifiedIdentifier(path) => {
                self.evaluate_qualified_identifier(&path)
            }
            Expr::Null => {
                Err(InterpreterError::InternalError("Null reference error".to_string()))
            }
            Expr::SliceAssign(object, start, end, value) => {
                self.evaluate_slice_assign(&object, &start, &end, &value)
            }
            Expr::SliceAccess(object, slice_info) => {
                self.evaluate_slice_access_with_info(&object, &slice_info)
            }
            Expr::DictLiteral(entries) => {
                self.evaluate_dict_literal(&entries)
            }
            Expr::TupleLiteral(elements) => {
                self.evaluate_tuple_literal(&elements)
            }
            Expr::TupleAccess(tuple, index) => {
                self.evaluate_tuple_access(&tuple, index)
            }
            Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
                self.evaluate_associated_function_call(&struct_name, &function_name, &args)
            }
            Expr::Cast(expr, target_type) => {
                self.evaluate_cast(&expr, &target_type)
            }
            Expr::Match(scrutinee, arms) => {
                self.evaluate_match(&scrutinee, &arms)
            }
            Expr::With(allocator, body) => {
                // Evaluate the allocator expression. The type checker already ensures
                // the value is of type Allocator; extract the underlying Rc<dyn Allocator>
                // and push it onto the scope stack for the duration of the body. The
                // pop must happen on every exit path (value, return, break, continue,
                // error) so nested `with` blocks always restore the outer binding.
                let allocator_val = self.evaluate(&allocator);
                let allocator_val = self.extract_value(allocator_val)?;
                let allocator_rc = match &*allocator_val.borrow() {
                    Object::Allocator(rc) => rc.clone(),
                    other => return Err(InterpreterError::InternalError(format!(
                        "with: allocator expression did not produce an Allocator value: {:?}",
                        other
                    ))),
                };
                self.allocator_stack.push(allocator_rc);
                let body_expr = self.expr_pool.get(&body)
                    .ok_or_else(|| InterpreterError::InternalError("Invalid with-body reference".to_string()))?;
                let result = if let Expr::Block(statements) = body_expr {
                    self.environment.enter_block();
                    let res = self.evaluate_block(&statements);
                    self.environment.exit_block();
                    res
                } else {
                    Err(InterpreterError::InternalError("with body is not a block".to_string()))
                };
                self.allocator_stack.pop();
                result
            }
            _ => Err(InterpreterError::InternalError(format!("evaluate: unexpected expr: {expr:?}"))),
        }
    }

    /// Evaluates literal values (Int64, UInt64, String, True, False)
    pub(super) fn evaluate_literal(&self, expr: &Expr) -> Result<EvaluationResult, InterpreterError> {
        let obj = convert_object(expr)?;
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))))
    }

    /// Evaluates if-elif-else control structure
    pub(super) fn evaluate_if_elif_else(&mut self, cond: &ExprRef, then: &ExprRef, elif_pairs: &[(ExprRef, ExprRef)], _else: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Evaluate if condition
        let cond = self.evaluate(cond);
        let cond = self.extract_value(cond)?;
        let cond = cond.borrow();
        if cond.get_type() != TypeDecl::Bool {
            return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: cond.get_type(), message: "evaluate: Bad types for if condition".to_string()});
        }

        let mut selected_block = None;

        // Check if condition
        if cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
            let then_expr = self.expr_pool.get(&then)
                .ok_or_else(|| InterpreterError::InternalError("Invalid then block reference".to_string()))?;
            if !then_expr.is_block() {
                return Err(InterpreterError::InternalError("if-then is not block".to_string()));
            }
            selected_block = Some(then);
        } else {
            // Check elif conditions
            for (elif_cond, elif_block) in elif_pairs {
                let elif_cond = self.evaluate(elif_cond);
                let elif_cond = self.extract_value(elif_cond)?;
                let elif_cond = elif_cond.borrow();
                if elif_cond.get_type() != TypeDecl::Bool {
                    return Err(InterpreterError::TypeError{expected: TypeDecl::Bool, found: elif_cond.get_type(), message: "evaluate: Bad types for elif condition".to_string()});
                }

                if elif_cond.try_unwrap_bool().map_err(InterpreterError::ObjectError)? {
                    let elif_expr = self.expr_pool.get(&elif_block)
                        .ok_or_else(|| InterpreterError::InternalError("Invalid elif block reference".to_string()))?;
                    if !elif_expr.is_block() {
                        return Err(InterpreterError::InternalError("elif block is not block".to_string()));
                    }
                    selected_block = Some(elif_block);
                    break;
                }
            }

            // If no elif condition matched, use else block
            if selected_block.is_none() {
                let else_expr = self.expr_pool.get(&_else)
                    .ok_or_else(|| InterpreterError::InternalError("Invalid else block reference".to_string()))?;
                if !else_expr.is_block() {
                    return Err(InterpreterError::InternalError("else block is not block".to_string()));
                }
                selected_block = Some(_else);
            }
        }

        // Execute selected block
        if let Some(block_expr) = selected_block {
            self.environment.enter_block();
            let res = {
                if let Some(Expr::Block(statements)) = self.expr_pool.get(&block_expr) {
                    self.evaluate_block(&statements)
                } else {
                    return Err(InterpreterError::InternalError("evaluate: selected block is not block".to_string()))
                }
            };
            self.environment.exit_block();
            res
        } else {
            Err(InterpreterError::InternalError("evaluate: no block selected in if-elif-else".to_string()))
        }
    }

    /// Evaluates array literal expressions
    pub(super) fn evaluate_array_literal(&mut self, elements: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let mut array_objects = Vec::new();
        for element in elements {
            let value = self.evaluate(element)?;
            let obj = self.extract_value(Ok(value))?;
            array_objects.push(obj);
        }
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(array_objects))))))
    }

    pub(super) fn evaluate_dict_literal(&mut self, entries: &[(ExprRef, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        let mut dict = HashMap::new();

        for (key_ref, value_ref) in entries {
            // Evaluate key - now supports any Object type that can be used as a key
            let key_val = self.evaluate(key_ref)?;
            let key_obj_rc = self.extract_value(Ok(key_val))?;

            // Convert to ObjectKey - clone the object for use as a key
            let key_object = key_obj_rc.borrow().clone();
            let object_key = ObjectKey::new(key_object);

            // Evaluate value
            let value_val = self.evaluate(value_ref)?;
            let value_obj = self.extract_value(Ok(value_val))?;

            dict.insert(object_key, value_obj);
        }

        let dict_obj = Object::Dict(Box::new(dict));
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(dict_obj))))
    }

    pub(super) fn evaluate_tuple_literal(&mut self, elements: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let mut tuple_elements = Vec::new();

        for element_ref in elements {
            let element_val = self.evaluate(element_ref);
            let element_obj = self.extract_value(element_val)?;
            tuple_elements.push(element_obj);
        }

        let tuple_obj = Object::Tuple(Box::new(tuple_elements));
        Ok(EvaluationResult::Value(Rc::new(RefCell::new(tuple_obj))))
    }

    pub(super) fn evaluate_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<EvaluationResult, InterpreterError> {
        let tuple_val = self.evaluate(tuple);
        let tuple_obj = self.extract_value(tuple_val)?;

        let tuple_borrowed = tuple_obj.borrow();
        match &*tuple_borrowed {
            Object::Tuple(elements) => {
                if index >= elements.len() {
                    return Err(InterpreterError::IndexOutOfBounds {
                        index: index as isize,
                        size: elements.len()
                    });
                }
                Ok(EvaluationResult::Value(Rc::clone(&elements[index])))
            }
            _ => {
                Err(InterpreterError::InternalError(format!(
                    "Cannot access index {} on non-tuple type",
                    index
                )))
            }
        }
    }

    /// Evaluate type cast expression (e.g., `x as i64`)
    pub(super) fn evaluate_cast(&mut self, expr: &ExprRef, target_type: &TypeDecl) -> Result<EvaluationResult, InterpreterError> {
        let value = self.evaluate(expr);
        let value_obj = self.extract_value(value)?;

        let borrowed = value_obj.borrow();
        let result = match (&*borrowed, target_type) {
            // i64 -> u64
            (Object::Int64(v), TypeDecl::UInt64) => Object::UInt64(*v as u64),
            // u64 -> i64
            (Object::UInt64(v), TypeDecl::Int64) => Object::Int64(*v as i64),
            // Identity casts
            (Object::Int64(v), TypeDecl::Int64) => Object::Int64(*v),
            (Object::UInt64(v), TypeDecl::UInt64) => Object::UInt64(*v),
            // Other cases that should not happen after type checking
            _ => {
                return Err(InterpreterError::InternalError(format!(
                    "Invalid cast from {:?} to {:?}",
                    borrowed, target_type
                )));
            }
        };

        Ok(EvaluationResult::Value(Rc::new(RefCell::new(result))))
    }

    /// Resolve module qualified name (e.g., math.add -> module [math], variable add)
    pub(super) fn resolve_module_qualified_name(&self, module_name: DefaultSymbol, variable_name: DefaultSymbol) -> Option<RcObject> {
        // Convert single module name to module path (could be extended for nested modules)
        let module_path = vec![module_name];

        // Look up variable in the specified module
        if let Some(variable_value) = self.environment.resolve_qualified_name(&module_path, variable_name) {
            Some(variable_value.value.clone())
        } else {
            None
        }
    }

    /// Evaluate qualified identifier (e.g., math::add)
    pub(super) fn evaluate_qualified_identifier(&mut self, path: &Vec<DefaultSymbol>) -> Result<EvaluationResult, InterpreterError> {
        if path.is_empty() {
            return Err(InterpreterError::InternalError("Empty qualified identifier path".to_string()));
        }

        // Enum variant reference: `Enum::Variant` resolves to a unit
        // EnumVariant. Tuple variants use `Enum::Variant(args)` and flow
        // through evaluate_associated_function_call instead.
        if path.len() == 2 {
            if let Some(variants) = self.enum_definitions.get(&path[0]) {
                if let Some((_, arity)) = variants.iter().find(|(n, _)| *n == path[1]) {
                    if *arity == 0 {
                        let obj = Object::EnumVariant {
                            enum_name: path[0],
                            variant_name: path[1],
                            values: Vec::new(),
                        };
                        return Ok(EvaluationResult::Value(Rc::new(RefCell::new(obj))));
                    }
                }
            }
        }

        // For now, treat qualified identifiers as simple variable lookups using the last component
        // In the future, this can be enhanced for proper module resolution
        if let Some(last_symbol) = path.last() {
            // Try to look up the qualified name in the environment
            if let Some(val) = self.environment.get_val(*last_symbol) {
                Ok(EvaluationResult::Value(val))
            } else {
                Err(InterpreterError::UndefinedVariable(format!("Qualified identifier not found: {:?}", path)))
            }
        } else {
            Err(InterpreterError::InternalError("Empty qualified identifier path".to_string()))
        }
    }

    pub(super) fn evaluate_match(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<(Pattern, ExprRef)>,
    ) -> Result<EvaluationResult, InterpreterError> {
        let scrutinee_val = self.evaluate(scrutinee);
        let scrutinee_val = self.extract_value(scrutinee_val)?;
        let (enum_name, variant_name, values): (DefaultSymbol, DefaultSymbol, Vec<RcObject>) = match &*scrutinee_val.borrow() {
            Object::EnumVariant { enum_name, variant_name, values } => {
                (*enum_name, *variant_name, values.iter().cloned().collect())
            }
            other => {
                return Err(InterpreterError::InternalError(format!(
                    "match scrutinee must be an enum variant, got {:?}", other
                )));
            }
        };
        for (pattern, body) in arms {
            match pattern {
                Pattern::Wildcard => {
                    return self.evaluate(body);
                }
                Pattern::EnumVariant(p_enum, p_variant, bindings) => {
                    if *p_enum != enum_name || *p_variant != variant_name {
                        continue;
                    }
                    if !bindings.is_empty() {
                        // Push a scope and bind each Name slot to the payload
                        // value at that index; Wildcard slots bind nothing.
                        self.environment.enter_block();
                        for (binding, payload) in bindings.iter().zip(values.iter()) {
                            if let PatternBinding::Name(sym) = binding {
                                self.environment.set_val(*sym, payload.clone());
                            }
                        }
                        let result = self.evaluate(body);
                        self.environment.exit_block();
                        return result;
                    }
                    return self.evaluate(body);
                }
            }
        }
        // Type checker is expected to catch non-exhaustive matches on known
        // enums; this runtime check is a defensive fallback.
        Err(InterpreterError::InternalError(
            "no matching arm in match expression".to_string(),
        ))
    }
}
