use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    Acceptable, TypeInferenceManager
};
use crate::type_checker::generics::GenericTypeChecking;

/// Expression type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Main entry point for expression type checking
    pub fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Check cache first
        if let Some(cached_type) = self.get_cached_type(expr) {
            return Ok(cached_type.clone());
        }
        
        // Set up context hint for nested expressions
        let original_hint = self.type_inference.type_hint.clone();
        let expr_obj = self.core.expr_pool.get(&expr)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        
        let result = expr_obj.clone().accept(self);
        
        // Add location information to errors if not already present
        let result = match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_expr_location(expr);
                Err(error)
            }
            other => other,
        };
        
        // Cache result and record type if successful
        if let Ok(ref result_type) = result {
            self.cache_type(&expr, result_type.clone());
            self.type_inference.set_expr_type(*expr, result_type.clone());
            
            // Context propagation for numeric types
            if original_hint.is_none() && (result_type == &TypeDecl::Int64 || result_type == &TypeDecl::UInt64) {
                if self.type_inference.type_hint.is_none() {
                    self.type_inference.type_hint = Some(result_type.clone());
                }
            }
        }
        
        result
    }

    /// Type check unary operators
    pub fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let operand = operand.clone();
        let operand_ty = {
            let operand_obj = self.core.expr_pool.get(&operand)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid operand expression reference"))?;
            operand_obj.clone().accept(self)?
        };
        
        // Resolve type with automatic conversion for Number type. Negation
        // implies a signed result, so coerce an unspecified Number to Int64
        // the same way an explicit `-3i64` literal would land.
        let resolved_ty = if operand_ty == TypeDecl::Number {
            match op {
                UnaryOp::BitwiseNot => TypeDecl::UInt64,
                UnaryOp::Negate => TypeDecl::Int64,
                UnaryOp::LogicalNot => TypeDecl::Bool,
            }
        } else {
            operand_ty.clone()
        };
        
        // Transform AST node if type conversion occurred
        if operand_ty == TypeDecl::Number && resolved_ty != TypeDecl::Number {
            self.transform_numeric_expr(&operand, &resolved_ty)?;
        }
        
        let result_type = match op {
            UnaryOp::BitwiseNot => {
                if resolved_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("bitwise NOT", resolved_ty.clone(), TypeDecl::Unit),
                        &operand,
                    ));
                }
            }
            UnaryOp::LogicalNot => {
                if resolved_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("logical NOT", resolved_ty.clone(), TypeDecl::Unit),
                        &operand,
                    ));
                }
            }
            UnaryOp::Negate => {
                // Only signed integers and f64 may be negated. Rejecting u64
                // avoids the silent-wraparound surprise `-(1u64) == 2^64 - 1`;
                // users who really want the two's-complement representation
                // can cast first.
                if resolved_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else if resolved_ty == TypeDecl::Float64 {
                    TypeDecl::Float64
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("unary minus", resolved_ty.clone(), TypeDecl::Int64),
                        &operand,
                    ));
                }
            }
        };
        
        Ok(result_type)
    }

    /// Type check binary operators
    pub fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        
        // Special handling for shift operations where right operand must be UInt64
        let (resolved_lhs_ty, resolved_rhs_ty) = if matches!(op, Operator::LeftShift | Operator::RightShift) {
            self.resolve_shift_operand_types(&lhs_ty, &rhs_ty)
        } else {
            self.resolve_numeric_types(&lhs_ty, &rhs_ty)
                .map_err(|error| self.error_with_location(error, &lhs))?
        };

        // Context propagation: if we have a type hint, propagate it to Number expressions
        if let Some(hint) = self.type_inference.type_hint.clone() {
            if lhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&lhs, &hint)?;
            }
            if rhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&rhs, &hint)?;
            }
        }
        
        // Record Number usage context for later finalization
        self.record_number_usage_context(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.record_number_usage_context(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        
        // Immediate propagation: if one side has concrete type, propagate to Number variables
        if resolved_lhs_ty != TypeDecl::Number && rhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&rhs, &resolved_lhs_ty)?;
        }
        if resolved_rhs_ty != TypeDecl::Number && lhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&lhs, &resolved_rhs_ty)?;
        }
        
        // Transform AST nodes if type conversion occurred
        if lhs_ty == TypeDecl::Number && resolved_lhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&lhs, &resolved_lhs_ty)?;
        }
        if rhs_ty == TypeDecl::Number && resolved_rhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&rhs, &resolved_rhs_ty)?;
        }
        
        // Update variable types if identifiers were involved in type conversion
        self.update_identifier_types(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.update_identifier_types(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        
        // Determine result type based on operator
        let result_type = match op {
            Operator::IAdd if resolved_lhs_ty == TypeDecl::String && resolved_rhs_ty == TypeDecl::String => {
                TypeDecl::String
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul | Operator::IMod => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else if resolved_lhs_ty == TypeDecl::Float64 && resolved_rhs_ty == TypeDecl::Float64 {
                    // f64 supports +, -, *, /, %. `%` follows Rust's `f64::rem`,
                    // matching the IEEE 754 remainder via fmod-style truncation.
                    TypeDecl::Float64
                } else if let (TypeDecl::Generic(left_param), TypeDecl::Generic(right_param)) = (&resolved_lhs_ty, &resolved_rhs_ty) {
                    // Allow arithmetic operations on generic types if they are the same parameter
                    if left_param == right_param {
                        resolved_lhs_ty.clone()
                    } else {
                        return Err(self.error_with_location(
                            TypeCheckError::type_mismatch_operation("arithmetic", resolved_lhs_ty.clone(), resolved_rhs_ty.clone()),
                            &lhs,
                        ));
                    }
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("arithmetic", resolved_lhs_ty.clone(), resolved_rhs_ty.clone()),
                        &lhs,
                    ));
                }
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                if (resolved_lhs_ty == TypeDecl::UInt64 || resolved_lhs_ty == TypeDecl::Int64) &&
                   (resolved_rhs_ty == TypeDecl::UInt64 || resolved_rhs_ty == TypeDecl::Int64) {
                    TypeDecl::Bool
                } else if resolved_lhs_ty == TypeDecl::Float64 && resolved_rhs_ty == TypeDecl::Float64 {
                    // f64 comparisons use IEEE 754 semantics — NaN compares
                    // false for ordering and equality, matching Rust's PartialOrd.
                    TypeDecl::Bool
                } else if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else if matches!(op, Operator::EQ | Operator::NE)
                          && self.is_allocator_compatible(&resolved_lhs_ty)
                          && self.is_allocator_compatible(&resolved_rhs_ty) {
                    // Allocator handles support only identity (== / !=), not ordering.
                    // A generic parameter bounded by Allocator counts as allocator-compatible
                    // so expressions like `current_allocator() == a` type-check inside a
                    // `<A: Allocator>` function body.
                    TypeDecl::Bool
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("comparison", resolved_lhs_ty.clone(), resolved_rhs_ty.clone()),
                        &lhs,
                    ));
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("logical", resolved_lhs_ty.clone(), resolved_rhs_ty.clone()),
                        &lhs,
                    ));
                }
            }
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("bitwise", resolved_lhs_ty.clone(), resolved_rhs_ty.clone()),
                        &lhs,
                    ));
                }
            }
            Operator::LeftShift | Operator::RightShift => {
                // For shift operations, right operand must be UInt64
                if resolved_rhs_ty != TypeDecl::UInt64 {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("shift", TypeDecl::UInt64, resolved_rhs_ty.clone()),
                        &rhs,
                    ));
                }
                // Left operand can be either UInt64 or Int64
                if resolved_lhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    return Err(self.error_with_location(
                        TypeCheckError::type_mismatch_operation("shift", resolved_lhs_ty.clone(), TypeDecl::UInt64),
                        &lhs,
                    ));
                }
            }
        };
        
        Ok(result_type)
    }

    /// Type check block expressions
    pub fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        
        // Clear type cache at the start of each block to limit cache scope
        self.optimization.type_cache.clear();
        
        // Pre-scan for explicit type declarations and establish global type context
        let original_hint = self.type_inference.type_hint.clone();
        // Only override the inherited hint when it's unset, so an outer hint
        // (e.g. the method's declared return type) isn't clobbered by a
        // numeric-scan result from a transient `val x: u64 = ...` in the body.
        if original_hint.is_none() {
            if let Some(numeric_type) = self.scan_numeric_type_hint(statements) {
                self.type_inference.type_hint = Some(numeric_type);
            }
        }

        // Process each statement
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements.iter() {
            let stmt = self.core.stmt_pool.get(&s)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference in block"))?;
            
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let expr_obj = self.core.expr_pool.get(&e)
                            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
                        let ty = expr_obj.clone().accept(self)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                return Err(TypeCheckError::type_mismatch(last_ty, ty).with_context("return statement"));
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => {
                    let stmt_obj = self.core.stmt_pool.get(&s)
                        .ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
                    stmt_obj.clone().accept(self)
                }
            };

            match stmt_type {
                Ok(def_ty) => last = Some(def_ty),
                Err(e) => return Err(e),
            }
        }
        
        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::generic_error("Empty block - no return value"))
        }
    }

    /// Type check if-elif-else expressions
    pub fn visit_if_elif_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let mut block_types = Vec::new();

        // Check if-block
        let if_block = then_block.clone();
        let is_if_empty = match self.core.expr_pool.get(&if_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_expr = self.core.expr_pool.get(&if_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))?;
            let if_ty = if_expr.clone().accept(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = elif_block.clone();
            let is_elif_empty = match self.core.expr_pool.get(&elif_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))? {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_expr = self.core.expr_pool.get(&elif_block)
                    .ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))?;
                let elif_ty = elif_expr.clone().accept(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = else_block.clone();
        let is_else_empty = match self.core.expr_pool.get(&else_block)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_expr = self.core.expr_pool.get(&else_block)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))?;
            let else_ty = else_expr.clone().accept(self)?;
            block_types.push(else_ty);
        }

        // If no blocks have values or all blocks are empty, return Unit
        if block_types.is_empty() {
            return Ok(TypeDecl::Unit);
        }

        // Pick the first concrete (non-Unknown) branch type as the result;
        // Unknown branches (e.g. ones ending in `panic("...")`) unify with
        // any concrete sibling. If every branch is Unknown the if-expression
        // itself is Unknown — the surrounding context resolves it.
        let result_ty = block_types.iter()
            .find(|t| **t != TypeDecl::Unknown)
            .cloned()
            .unwrap_or(TypeDecl::Unknown);
        for block_type in &block_types {
            if *block_type != TypeDecl::Unknown && !block_type.is_equivalent(&result_ty) {
                return Ok(TypeDecl::Unit); // Different types, return Unit
            }
        }

        Ok(result_ty)
    }

    /// Type check assignment expressions
    pub fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        
        // Allow assignment compatibility
        if lhs_ty != rhs_ty {
            match (&lhs_ty, &rhs_ty) {
                // Allow unknown type (null values) assignment to any concrete type
                (_, TypeDecl::Unknown) => {
                    // Allow assignment of unknown/null to any type
                }
                // Allow assignment when types are equivalent (for type inference)
                (TypeDecl::Unknown, _) => {
                    // Allow assignment from any type to unknown (type inference)
                }
                _ => {
                    return Err(TypeCheckError::type_mismatch(lhs_ty, rhs_ty).with_context("assignment"));
                }
            }
        }
        Ok(lhs_ty)
    }

    /// Type check identifiers
    pub fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        if let Some(val_type) = self.context.get_var(name) {
            // Return the stored type, which may be Number for type inference
            let _name_str = self.resolve_symbol_name(name);
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else if let Some(generic_type) = self.type_inference.lookup_generic_type(name) {
            // Check if this is a generic type parameter
            Ok(generic_type.clone())
        } else if let Some(_struct_def) = self.context.get_struct_definition(name) {
            // Check if this is a struct type
            // If the struct has generic parameters, include them
            let type_params = if let Some(generic_params) = self.context.get_struct_generic_params(name) {
                generic_params.iter().map(|param| {
                    // Try to resolve from current generic scope, otherwise use Generic type
                    self.type_inference.lookup_generic_type(*param)
                        .unwrap_or_else(|| TypeDecl::Generic(*param))
                }).collect()
            } else {
                vec![]
            };
            Ok(TypeDecl::Struct(name, type_params))
        } else {
            let name_str = self.resolve_symbol_name(name);
            // Note: Location information will be added by visit_expr
            return Err(TypeCheckError::not_found("Identifier", &name_str));
        }
    }

    /// Whether `ty` can participate in an Allocator equality comparison — either
    /// the concrete Allocator type or a generic parameter bounded by Allocator.
    fn is_allocator_compatible(&self, ty: &TypeDecl) -> bool {
        match ty {
            TypeDecl::Allocator => true,
            TypeDecl::Generic(sym) => matches!(
                self.context.current_fn_generic_bounds.get(sym),
                Some(TypeDecl::Allocator)
            ),
            _ => false,
        }
    }

    /// If the call to `fun` omits trailing Allocator-typed parameters, extend the
    /// argument `ExprList` with synthetic `__builtin_current_allocator()` calls so
    /// downstream type checking and interpretation see the defaults. A parameter
    /// is considered defaultable when its declared type is `TypeDecl::Allocator`
    /// or a generic parameter bounded by `Allocator`. Only trailing positions are
    /// filled; once a non-defaultable parameter is reached the rest is left alone
    /// so the existing arity-mismatch error path still triggers.
    fn inject_ambient_defaults(&mut self, args_ref: &ExprRef, fun: &Function) {
        let args = match self.core.expr_pool.get(args_ref) {
            Some(Expr::ExprList(args)) => args,
            _ => return,
        };
        if args.len() >= fun.parameter.len() {
            return;
        }
        let mut extended = args.clone();
        for (_, param_ty) in fun.parameter.iter().skip(extended.len()) {
            let is_defaultable = match param_ty {
                TypeDecl::Allocator => true,
                TypeDecl::Generic(sym) => matches!(
                    fun.generic_bounds.get(sym),
                    Some(TypeDecl::Allocator)
                ),
                _ => false,
            };
            if !is_defaultable {
                break;
            }
            let ambient_call = Expr::BuiltinCall(
                crate::ast::BuiltinFunction::CurrentAllocator,
                vec![],
            );
            let expr_ref = self.core.expr_pool.add(ambient_call);
            extended.push(expr_ref);
        }
        if extended.len() > args.len() {
            self.core.expr_pool.update(args_ref, Expr::ExprList(extended));
        }
    }

    /// Type check function calls
    pub fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let _fn_name_str = self.resolve_symbol_name(fn_name);
        
        
        self.push_context();
        
        if let Some(fun) = self.context.get_fn(fn_name) {
            // Check visibility access control
            if let Err(err) = self.check_function_access(&fun) {
                self.pop_context();
                return Err(err);
            }

            // Auto-inject `ambient` for omitted trailing Allocator-typed parameters.
            // A parameter is defaultable when its type is `TypeDecl::Allocator` or a
            // generic parameter bounded by `Allocator`. Injection happens before the
            // generic-call dispatch so both paths see the extended argument list.
            self.inject_ambient_defaults(args_ref, &fun);

            // Handle generic function calls
            if !fun.generic_params.is_empty() {
                return self.visit_generic_call(fn_name, args_ref, &fun);
            }
            
            // Check if function has been type checked
            let status = self.function_checking.is_checked_fn.get(&fn_name);
            if status.is_none() || status.as_ref().and_then(|s| s.as_ref()).is_none() {
                // not checked yet
                let fun_copy = self.context.get_fn(fn_name)
                    .ok_or_else(|| TypeCheckError::not_found("Function", "<INTERNAL_ERROR>"))?;
                self.type_check(fun_copy.clone())?;
            }

            // Type check function arguments with proper type hints
            // Clone data we need to avoid borrowing conflicts
            let args_data = if let Some(args_expr) = self.core.expr_pool.get(&args_ref) {
                if let Expr::ExprList(args) = args_expr {
                    Some(args.clone())
                } else {
                    None
                }
            } else {
                self.pop_context();
                return Err(TypeCheckError::generic_error("Invalid arguments reference"));
            };
            
            if let Some(args) = args_data {
                let param_types: Vec<_> = fun.parameter.iter().map(|(_, ty)| {
                    // Normalize Identifier to Struct for known struct types
                    if let TypeDecl::Identifier(name) = ty {
                        if self.context.struct_definitions.contains_key(name) {
                            return TypeDecl::Struct(*name, vec![]);
                        }
                    }
                    ty.clone()
                }).collect();
                
                // Check argument count
                if args.len() != param_types.len() {
                    self.pop_context();
                    let fn_name_str = self.resolve_symbol_name(fn_name);
                    return Err(TypeCheckError::generic_error(&format!(
                        "Function '{}' argument count mismatch: expected {}, found {}",
                        fn_name_str, param_types.len(), args.len()
                    )));
                }
                
                // Type check each argument with expected type as hint
                let original_hint = self.type_inference.type_hint.clone();
                for (arg_index, (arg, expected_type)) in args.iter().zip(&param_types).enumerate() {
                    // Set type hint for this argument
                    self.type_inference.type_hint = Some(expected_type.clone());
                    let arg_type = self.visit_expr(arg)?;

                    // Check type compatibility — `is_equivalent` handles the
                    // Identifier↔Struct and Identifier↔Enum cases so user-named
                    // types unify with their resolved form.
                    if !arg_type.is_equivalent(expected_type) && arg_type != TypeDecl::Unknown {
                        // Restore hint before returning error
                        self.type_inference.type_hint = original_hint;
                        self.pop_context();
                        let fn_name_str = self.resolve_symbol_name(fn_name);
                        return Err(TypeCheckError::generic_error(&format!(
                            "Type error: expected {:?}, found {:?}. Function '{}' argument {} type mismatch",
                            expected_type, arg_type, fn_name_str, arg_index + 1
                        )));
                    }
                }
                // Restore original hint
                self.type_inference.type_hint = original_hint;
            }
            
            self.pop_context();
            // Normalize `Identifier(name)` return types to `Struct(name, [])` for
            // known structs so downstream method dispatch (which matches on
            // Struct) works on values produced by `fn make_list() -> List { ... }`.
            let ret = fun.return_type.clone().unwrap_or(TypeDecl::Unknown);
            let ret = if let TypeDecl::Identifier(name) = &ret {
                if self.context.struct_definitions.contains_key(name) {
                    TypeDecl::Struct(*name, vec![])
                } else {
                    ret
                }
            } else {
                ret
            };
            Ok(ret)
        } else {
            self.pop_context();
            let fn_name_str = self.resolve_symbol_name(fn_name);
            Err(TypeCheckError::not_found("Function", &fn_name_str))
        }
    }

    /// Type check literal values
    pub fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    pub fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    pub fn visit_float64_literal(&mut self, _value: &f64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Float64)
    }

    pub fn visit_number_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Number)
    }

    pub fn visit_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    pub fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Bool)
    }

    pub fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        // Null value type is determined by context
        // If we have a type hint, use that; otherwise return Unknown
        if let Some(hint) = self.type_inference.get_type_hint() {
            Ok(hint)
        } else {
            Ok(TypeDecl::Unknown)
        }
    }

    /// Type check expression lists
    pub fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    /// Type check array literals
    pub fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::array_error("Empty array literals are not supported"));
        }

        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in array literal type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        
        // Execute the main logic and capture result
        let result = self.visit_array_literal_impl(elements);
        
        // Always decrement recursion depth before returning
        self.type_inference.recursion_depth -= 1;
        
        result
    }

}
