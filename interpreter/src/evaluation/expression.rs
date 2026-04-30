use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::{Object, ObjectKey, RcObject};
use crate::value::Value;
use crate::error::InterpreterError;
use crate::try_value;
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
            Expr::Int64(_) | Expr::UInt64(_) | Expr::Float64(_) | Expr::String(_) | Expr::True | Expr::False => {
                self.evaluate_literal(&expr)
            }
            Expr::Number(_v) => {
                // Type-unspecified numbers should be resolved during type checking
                Err(InterpreterError::InternalError("Expr::Number should be transformed to concrete type during type checking".to_string()))
            }
            Expr::Identifier(s) => {
                let val = self.environment.get_val(s)
                    .ok_or_else(|| InterpreterError::UndefinedVariable(format!("Variable not found: {s:?}")))?;
                Ok(EvaluationResult::Value(val.into()))
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
            Expr::Range(start, end) => {
                let start_val = self.evaluate(&start);
                let start_val = try_value!(start_val);
                let end_val = self.evaluate(&end);
                let end_val = try_value!(end_val);
                let obj = Object::Range { start: start_val, end: end_val };
                Ok(EvaluationResult::Value((obj).into()))
            }
            Expr::With(allocator, body) => {
                // Evaluate the allocator expression. The type checker already ensures
                // the value is of type Allocator; extract the underlying Rc<dyn Allocator>
                // and push it onto the scope stack for the duration of the body. The
                // pop must happen on every exit path (value, return, break, continue,
                // error) so nested `with` blocks always restore the outer binding.
                let allocator_val = self.evaluate(&allocator);
                let allocator_val = try_value!(allocator_val);
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
        Ok(EvaluationResult::Value((obj).into()))
    }

    /// Evaluates if-elif-else control structure
    pub(super) fn evaluate_if_elif_else(&mut self, cond: &ExprRef, then: &ExprRef, elif_pairs: &[(ExprRef, ExprRef)], _else: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Evaluate if condition. We expect a `Bool` primitive — the type
        // checker has already enforced this — so we work directly on the
        // inline `Value` and skip the `Rc::clone` + `borrow()` round-trip.
        use crate::try_value_v;
        let cond = self.evaluate(cond);
        let cond_v = try_value_v!(cond);
        let cond_bool = match &cond_v {
            Value::Bool(b) => *b,
            other => return Err(InterpreterError::TypeError {
                expected: TypeDecl::Bool,
                found: other.get_type(),
                message: "evaluate: Bad types for if condition".to_string(),
            }),
        };

        let mut selected_block = None;

        // Check if condition
        if cond_bool {
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
                let elif_cond_v = try_value_v!(elif_cond);
                let elif_bool = match &elif_cond_v {
                    Value::Bool(b) => *b,
                    other => return Err(InterpreterError::TypeError {
                        expected: TypeDecl::Bool,
                        found: other.get_type(),
                        message: "evaluate: Bad types for elif condition".to_string(),
                    }),
                };

                if elif_bool {
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
            let obj = try_value!(Ok(value));
            array_objects.push(obj);
        }
        Ok(EvaluationResult::Value(Object::Array(Box::new(array_objects)).into()))
    }

    pub(super) fn evaluate_dict_literal(&mut self, entries: &[(ExprRef, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        let mut dict = HashMap::new();

        for (key_ref, value_ref) in entries {
            // Evaluate key - now supports any Object type that can be used as a key
            let key_val = self.evaluate(key_ref)?;
            let key_obj_rc = try_value!(Ok(key_val));

            // Convert to ObjectKey - clone the object for use as a key
            let key_object = key_obj_rc.borrow().clone();
            let object_key = ObjectKey::new(key_object);

            // Evaluate value
            let value_val = self.evaluate(value_ref)?;
            let value_obj = try_value!(Ok(value_val));

            dict.insert(object_key, value_obj);
        }

        let dict_obj = Object::Dict(Box::new(dict));
        Ok(EvaluationResult::Value((dict_obj).into()))
    }

    pub(super) fn evaluate_tuple_literal(&mut self, elements: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        let mut tuple_elements = Vec::new();

        for element_ref in elements {
            let element_val = self.evaluate(element_ref);
            let element_obj = try_value!(element_val);
            tuple_elements.push(element_obj);
        }

        let tuple_obj = Object::Tuple(Box::new(tuple_elements));
        Ok(EvaluationResult::Value((tuple_obj).into()))
    }

    pub(super) fn evaluate_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<EvaluationResult, InterpreterError> {
        let tuple_val = self.evaluate(tuple);
        let tuple_obj = try_value!(tuple_val);

        let tuple_borrowed = tuple_obj.borrow();
        match &*tuple_borrowed {
            Object::Tuple(elements) => {
                if index >= elements.len() {
                    return Err(InterpreterError::IndexOutOfBounds {
                        index: index as isize,
                        size: elements.len()
                    });
                }
                Ok(EvaluationResult::Value(Rc::clone(&elements[index]).into()))
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
        let value_obj = try_value!(value);

        let borrowed = value_obj.borrow();
        let result = match (&*borrowed, target_type) {
            // i64 -> u64
            (Object::Int64(v), TypeDecl::UInt64) => Object::UInt64(*v as u64),
            // u64 -> i64
            (Object::UInt64(v), TypeDecl::Int64) => Object::Int64(*v as i64),
            // i64 / u64 -> f64
            (Object::Int64(v), TypeDecl::Float64) => Object::Float64(*v as f64),
            (Object::UInt64(v), TypeDecl::Float64) => Object::Float64(*v as f64),
            // f64 -> i64 / u64 (Rust's `as`: truncation toward zero, saturation
            // on out-of-range, NaN becomes 0).
            (Object::Float64(v), TypeDecl::Int64) => Object::Int64(*v as i64),
            (Object::Float64(v), TypeDecl::UInt64) => Object::UInt64(*v as u64),
            // Identity casts
            (Object::Int64(v), TypeDecl::Int64) => Object::Int64(*v),
            (Object::UInt64(v), TypeDecl::UInt64) => Object::UInt64(*v),
            (Object::Float64(v), TypeDecl::Float64) => Object::Float64(*v),
            // Other cases that should not happen after type checking
            _ => {
                return Err(InterpreterError::InternalError(format!(
                    "Invalid cast from {:?} to {:?}",
                    borrowed, target_type
                )));
            }
        };

        Ok(EvaluationResult::Value((result).into()))
    }

    /// Resolve module qualified name (e.g., math.add -> module [math], variable add)
    pub(super) fn resolve_module_qualified_name(&self, module_name: DefaultSymbol, variable_name: DefaultSymbol) -> Option<RcObject> {
        // Convert single module name to module path (could be extended for nested modules)
        let module_path = vec![module_name];

        // Look up variable in the specified module
        if let Some(variable_value) = self.environment.resolve_qualified_name(&module_path, variable_name) {
            Some(variable_value.value.clone().into_rc())
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
                        return Ok(EvaluationResult::Value((obj).into()));
                    }
                }
            }
        }

        // For now, treat qualified identifiers as simple variable lookups using the last component
        // In the future, this can be enhanced for proper module resolution
        if let Some(last_symbol) = path.last() {
            // Try to look up the qualified name in the environment
            if let Some(val) = self.environment.get_val(*last_symbol) {
                Ok(EvaluationResult::Value(val.into()))
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
        arms: &Vec<MatchArm>,
    ) -> Result<EvaluationResult, InterpreterError> {
        let scrutinee_val = self.evaluate(scrutinee);
        let scrutinee_val = try_value!(scrutinee_val);
        for arm in arms {
            // Probe each arm in a fresh scope so bindings that were set
            // during a partial match don't leak across arms when the
            // match ultimately fails or the guard is false.
            self.environment.enter_block();
            let matched = self.try_match_pattern(&arm.pattern, &scrutinee_val)?;
            if matched {
                // Guard runs after the bindings are in scope. A `false`
                // guard skips this arm and falls through to the next.
                let guard_passed = if let Some(guard_expr) = arm.guard {
                    let g = self.evaluate(&guard_expr);
                    let g = try_value!(g);
                    let b = matches!(&*g.borrow(), Object::Bool(true));
                    b
                } else {
                    true
                };
                if guard_passed {
                    let result = self.evaluate(&arm.body);
                    self.environment.exit_block();
                    return result;
                }
            }
            self.environment.exit_block();
        }
        Err(InterpreterError::InternalError(
            "no matching arm in match expression".to_string(),
        ))
    }

    /// Try to match `pattern` against `value`, binding any `Name`
    /// sub-patterns into the current environment scope. Returns `true` if
    /// the pattern matches; on mismatch the caller should unwind the scope
    /// it pushed so abandoned bindings don't persist.
    fn try_match_pattern(
        &mut self,
        pattern: &Pattern,
        value: &RcObject,
    ) -> Result<bool, InterpreterError> {
        match pattern {
            Pattern::Wildcard => Ok(true),
            Pattern::Name(sym) => {
                self.environment.set_val(*sym, (value.clone().into()));
                Ok(true)
            }
            Pattern::Literal(literal_expr) => {
                // Pattern literals are constants by construction; reject any
                // control-flow signal as an internal bug rather than
                // propagating it (this helper can't return EvaluationResult).
                let lit_res = self.evaluate(literal_expr)?;
                let lit_value = self.unwrap_value(lit_res)?;
                let eq = *value.borrow() == *lit_value.borrow();
                Ok(eq)
            }
            Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                let (enum_name, variant_name, values) = match &*value.borrow() {
                    Object::EnumVariant { enum_name, variant_name, values } => {
                        (*enum_name, *variant_name, values.clone())
                    }
                    _ => return Ok(false),
                };
                if *p_enum != enum_name || *p_variant != variant_name {
                    return Ok(false);
                }
                if sub_patterns.len() != values.len() {
                    return Ok(false);
                }
                for (sub, payload) in sub_patterns.iter().zip(values.iter()) {
                    if !self.try_match_pattern(sub, payload)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Pattern::Tuple(sub_patterns) => {
                let elements = match &*value.borrow() {
                    Object::Tuple(elements) => elements.clone(),
                    _ => return Ok(false),
                };
                if sub_patterns.len() != elements.len() {
                    return Ok(false);
                }
                for (sub, element) in sub_patterns.iter().zip(elements.iter()) {
                    if !self.try_match_pattern(sub, element)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }
}
