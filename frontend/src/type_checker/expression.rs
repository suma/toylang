use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::AstVisitor;
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
        
        // Resolve type with automatic conversion for Number type
        let resolved_ty = if operand_ty == TypeDecl::Number {
            if op == UnaryOp::BitwiseNot {
                TypeDecl::UInt64
            } else {
                TypeDecl::Bool
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
                    let mut error = TypeCheckError::type_mismatch_operation("bitwise NOT", resolved_ty.clone(), TypeDecl::Unit);
                    if let Some(location) = self.get_expr_location(&operand) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            UnaryOp::LogicalNot => {
                if resolved_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("logical NOT", resolved_ty.clone(), TypeDecl::Unit);
                    if let Some(location) = self.get_expr_location(&operand) {
                        error = error.with_location(location);
                    }
                    return Err(error);
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
                .map_err(|mut error| {
                    if error.location.is_none() {
                        if let Some(location) = self.get_expr_location(&lhs) {
                            error = error.with_location(location);
                        }
                    }
                    error
                })?
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
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("arithmetic", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                if (resolved_lhs_ty == TypeDecl::UInt64 || resolved_lhs_ty == TypeDecl::Int64) && 
                   (resolved_rhs_ty == TypeDecl::UInt64 || resolved_rhs_ty == TypeDecl::Int64) {
                    TypeDecl::Bool
                } else if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("comparison", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("logical", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("bitwise", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LeftShift | Operator::RightShift => {
                // For shift operations, right operand must be UInt64
                if resolved_rhs_ty != TypeDecl::UInt64 {
                    let mut error = TypeCheckError::type_mismatch_operation("shift", TypeDecl::UInt64, resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&rhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
                // Left operand can be either UInt64 or Int64
                if resolved_lhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("shift", resolved_lhs_ty.clone(), TypeDecl::UInt64);
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
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
        let mut global_numeric_type: Option<TypeDecl> = None;
        for s in statements.iter() {
            if let Some(stmt) = self.core.stmt_pool.get(&s) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            global_numeric_type = Some(type_decl.clone());
                            break; // Use the first explicit numeric type found
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Set global type hint if found
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(ref global_type) = global_numeric_type {
            self.type_inference.type_hint = Some(global_type.clone());
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

        // Check if all blocks have the same type
        let first_type = &block_types[0];
        for block_type in &block_types[1..] {
            if block_type != first_type {
                return Ok(TypeDecl::Unit); // Different types, return Unit
            }
        }

        Ok(first_type.clone())
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
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else if let Some(generic_type) = self.type_inference.lookup_generic_type(name) {
            // Check if this is a generic type parameter
            Ok(generic_type.clone())
        } else if let Some(_struct_def) = self.context.get_struct_definition(name) {
            // Check if this is a struct type
            Ok(TypeDecl::Struct(name))
        } else {
            let name_str = self.core.string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
            // Note: Location information will be added by visit_expr
            return Err(TypeCheckError::not_found("Identifier", name_str));
        }
    }

    /// Type check function calls
    pub fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        
        if let Some(fun) = self.context.get_fn(fn_name) {
            // Check visibility access control
            if let Err(err) = self.check_function_access(&fun) {
                self.pop_context();
                return Err(err);
            }
            
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
                let param_types: Vec<_> = fun.parameter.iter().map(|(_, ty)| ty.clone()).collect();
                
                // Check argument count
                if args.len() != param_types.len() {
                    self.pop_context();
                    let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
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
                    
                    // Check type compatibility
                    if arg_type != *expected_type && arg_type != TypeDecl::Unknown {
                        // Restore hint before returning error
                        self.type_inference.type_hint = original_hint;
                        self.pop_context();
                        let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
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
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            self.pop_context();
            let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
            Err(TypeCheckError::not_found("Function", fn_name_str))
        }
    }

    /// Type check literal values
    pub fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    pub fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
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

    /// Type check slice access
    pub fn visit_slice_access(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        // Check object type
        let obj_type = self.visit_expr(object)?;
        
        // Must be array or string type
        match &obj_type {
            TypeDecl::Array(element_types, _size) => {
                // Get the element type (assuming homogeneous array)
                let element_type = element_types.first().map(|t| Box::new(t.clone())).unwrap_or(Box::new(TypeDecl::Unknown));
                // Check index types
                if let Some(ref start) = slice_info.start {
                    let start_type = self.visit_expr(start)?;
                    if start_type != TypeDecl::Int64 && start_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice start index", 
                            start_type, 
                            TypeDecl::Int64
                        ));
                    }
                }
                
                if let Some(ref end) = slice_info.end {
                    let end_type = self.visit_expr(end)?;
                    if end_type != TypeDecl::Int64 && end_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice end index",
                            end_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                // Slice result is same array type with unknown size for now
                Ok(TypeDecl::Array(vec![*element_type.clone()], 0))
            }
            TypeDecl::String => {
                // String slicing works similarly
                if let Some(ref start) = slice_info.start {
                    let start_type = self.visit_expr(start)?;
                    if start_type != TypeDecl::Int64 && start_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice start index",
                            start_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                if let Some(ref end) = slice_info.end {
                    let end_type = self.visit_expr(end)?;
                    if end_type != TypeDecl::Int64 && end_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice end index",
                            end_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                Ok(TypeDecl::String)
            }
            _ => {
                Err(TypeCheckError::type_mismatch_operation(
                    "slice access",
                    obj_type,
                    TypeDecl::Array(vec![TypeDecl::Unknown], 0)
                ))
            }
        }
    }

    /// Type check field access
    pub fn visit_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        let obj_type = self.visit_expr(obj)?;
        
        match &obj_type {
            TypeDecl::Struct(struct_name) => {
                if let Some(struct_def) = self.context.get_struct_definition(*struct_name) {
                    for field_def in &struct_def.fields {
                        let field_name_str = self.core.string_interner.resolve(*field).unwrap_or("");
                        if field_def.name == field_name_str {
                            return Ok(field_def.type_decl.clone());
                        }
                    }
                    let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                    let struct_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<NOT_FOUND>");
                    Err(TypeCheckError::not_found(
                        "Field",
                        &format!("{} in struct '{}'", field_str, struct_str)
                    ))
                } else {
                    let struct_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<NOT_FOUND>");
                    Err(TypeCheckError::not_found("Struct definition", struct_str))
                }
            }
            TypeDecl::Generic(_generic_param) => {
                // Generic types don't have fields
                let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                Err(TypeCheckError::generic_error(&format!("Cannot access field '{}' on generic type parameter", field_str)))
            }
            _ => {
                let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                Err(TypeCheckError::type_mismatch_operation(
                    &format!("field access '{}'", field_str),
                    obj_type,
                    TypeDecl::Struct(*field)
                ))
            }
        }
    }

    /// Type check method calls
    pub fn visit_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let obj_type = self.visit_expr(obj)?;
        
        // Type check arguments
        let mut arg_types = Vec::new();
        for arg in args {
            arg_types.push(self.visit_expr(arg)?);
        }
        
        // Check for builtin methods
        let method_str = self.core.string_interner.resolve(*method).unwrap_or("<NOT_FOUND>");
        let builtin_method = self.builtin_methods.get(&(obj_type.clone(), method_str.to_string())).cloned();
        if let Some(builtin_method) = builtin_method {
            // visit_builtin_method_call expects ExprRef, not TypeDecl
            return self.visit_builtin_method_call(obj, &builtin_method, args);
        }
        
        // Check struct methods
        // Note: Current struct definition does not include methods
        // Method support will be added in a future refactoring
        
        // Check other type methods
        self.visit_method_call_on_type(&obj_type, method, args, &arg_types)
    }
}