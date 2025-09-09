use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

/// Type conversion and transformation implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Set up type hint for variable declarations based on explicit type annotation
    pub fn setup_type_hint_for_val(&mut self, type_decl: &Option<TypeDecl>) -> Option<TypeDecl> {
        let old_hint = self.type_inference.type_hint.clone();
        
        if let Some(decl) = type_decl {
            match decl {
                TypeDecl::Array(element_types, _) => {
                    // For array types (including struct arrays), set the array type as hint for array literal processing
                    if !element_types.is_empty() {
                        self.type_inference.type_hint = Some(decl.clone());
                    }
                },
                TypeDecl::Struct(_) => {
                    // For struct types, set the struct type as hint for struct literal processing
                    self.type_inference.type_hint = Some(decl.clone());
                },
                TypeDecl::Ptr => {
                    // For pointer types, set the pointer type as hint for builtin allocation functions
                    self.type_inference.type_hint = Some(decl.clone());
                },
                TypeDecl::Dict(_, _) => {
                    // For dict types, set the dict type as hint for dict literal processing
                    self.type_inference.type_hint = Some(decl.clone());
                },
                _ if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => {
                    self.type_inference.type_hint = Some(decl.clone());
                },
                _ => {}
            }
        }
        
        old_hint
    }

    /// Update variable-expression mapping for type inference
    pub fn update_variable_expr_mapping(&mut self, name: DefaultSymbol, expr_ref: &ExprRef) {
        let expr_ty = if let Ok(ty) = self.visit_expr(expr_ref) { ty } else { return };
        self.update_variable_expr_mapping_internal(name, expr_ref, &expr_ty);
    }
    
    /// Apply type transformations for numeric expressions
    pub fn apply_type_transformations(&mut self, name: DefaultSymbol, type_decl: &TypeDecl) -> Result<(), TypeCheckError> {
        self.apply_type_transformations_internal(name, type_decl)
    }
    
    /// Determine final type for variable declarations
    pub fn determine_final_type(&mut self, name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError> {
        self.determine_final_type_internal(name, inferred_type, declared_type)
    }

    /// Updates variable-expression mapping for type inference (internal implementation)
    pub fn update_variable_expr_mapping_internal(&mut self, name: DefaultSymbol, expr_ref: &ExprRef, expr_ty: &TypeDecl) {
        if *expr_ty == TypeDecl::Number || (*expr_ty != TypeDecl::Number && self.has_number_in_expr(expr_ref)) {
            self.type_inference.variable_expr_mapping.insert(name, expr_ref.clone());
        } else {
            // Remove old mapping for non-Number types to prevent stale references
            self.type_inference.variable_expr_mapping.remove(&name);
            // Also remove from number_usage_context to prevent stale type inference
            let indices_to_remove: Vec<usize> = self.type_inference.number_usage_context
                .iter()
                .enumerate()
                .filter_map(|(i, (old_expr, _))| {
                    if self.is_old_number_for_variable(name, old_expr) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            
            // Remove in reverse order to maintain valid indices
            for &index in indices_to_remove.iter().rev() {
                self.type_inference.number_usage_context.remove(index);
            }
        }
    }

    /// Applies type transformations for numeric expressions (internal implementation)
    pub fn apply_type_transformations_internal(&mut self, _name: DefaultSymbol, _type_decl: &TypeDecl) -> Result<(), TypeCheckError> {
        // Implementation for trait method - delegating to existing logic
        Ok(())
    }
    
    /// Applies type transformations for numeric expressions based on context
    pub fn apply_type_transformations_for_expr(&mut self, type_decl: &Option<TypeDecl>, expr_ty: &TypeDecl, expr_ref: &ExprRef) -> Result<(), TypeCheckError> {
        if type_decl.is_none() && *expr_ty == TypeDecl::Number {
            // No explicit type, but we have a Number - use type hint if available
            if let Some(hint) = self.type_inference.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    // Transform Number to hinted type
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
            }
        } else if type_decl.as_ref().map_or(false, |decl| *decl == TypeDecl::Unknown) && *expr_ty == TypeDecl::Int64 {
            // Unknown type declaration with Int64 inference - also transform
            if let Some(hint) = self.type_inference.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
            }
        } else if let Some(decl) = type_decl {
            if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number && *expr_ty == *decl {
                // Expression returned the hinted type, transform Number literals to concrete type
                if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
                    if let Expr::Number(_) = expr {
                        self.transform_numeric_expr(expr_ref, decl)?;
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Determines the final type for a variable declaration
    pub fn determine_final_type_internal(&mut self, _name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError> {
        // Implementation for trait method - delegating to existing logic
        Ok(self.determine_final_type_for_expr(declared_type, &inferred_type))
    }
    
    /// Determine final type for expressions
    pub fn determine_final_type_for_expr(&self, type_decl: &Option<TypeDecl>, expr_ty: &TypeDecl) -> TypeDecl {
        match (type_decl, expr_ty) {
            (Some(TypeDecl::Unknown), _) => expr_ty.clone(),
            // For ptr types, the declared type should match the expression type
            (Some(TypeDecl::Ptr), TypeDecl::Ptr) => TypeDecl::Ptr,
            // For dict types, if we have explicit type annotation, prefer it over inferred type
            (Some(TypeDecl::Dict(key_type, value_type)), TypeDecl::Dict(inferred_key, inferred_value)) => {
                // If both key and value types are explicit (not Unknown), use the declared type
                if **key_type != TypeDecl::Unknown && **value_type != TypeDecl::Unknown {
                    TypeDecl::Dict(key_type.clone(), value_type.clone())
                } else {
                    TypeDecl::Dict(inferred_key.clone(), inferred_value.clone())
                }
            },
            (Some(decl), _) if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => decl.clone(),
            (None, _) => expr_ty.clone(),
            _ => expr_ty.clone(),
        }
    }

    /// Transform Expr::Number nodes to concrete types based on resolved types
    pub fn transform_numeric_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        // Get the expression from the pool
        if let Some(expr) = self.core.expr_pool.get(expr_ref) {
            if let Expr::Number(value) = expr {
                let num_str = self.core.string_interner.resolve(value)
                    .ok_or_else(|| TypeCheckError::generic_error("Failed to resolve number literal"))?;
                
                // Create the new expression based on target type
                let new_expr = match target_type {
                    TypeDecl::UInt64 => {
                        let val = if num_str.starts_with("0x") || num_str.starts_with("0X") {
                            // Parse hexadecimal literal
                            u64::from_str_radix(&num_str[2..], 16)
                                .map_err(|_| TypeCheckError::conversion_error(num_str, "UInt64"))?
                        } else {
                            // Parse decimal literal
                            num_str.parse::<u64>()
                                .map_err(|_| TypeCheckError::conversion_error(num_str, "UInt64"))?
                        };
                        Expr::UInt64(val)
                    },
                    TypeDecl::Int64 => {
                        let val = if num_str.starts_with("0x") || num_str.starts_with("0X") {
                            // Parse hexadecimal literal
                            i64::from_str_radix(&num_str[2..], 16)
                                .map_err(|_| TypeCheckError::conversion_error(num_str, "Int64"))?
                        } else {
                            // Parse decimal literal
                            num_str.parse::<i64>()
                                .map_err(|_| TypeCheckError::conversion_error(num_str, "Int64"))?
                        };
                        Expr::Int64(val)
                    },
                    _ => {
                        return Err(TypeCheckError::unsupported_operation("transform", target_type.clone()));
                    }
                };
                
                // Replace the expression in the pool with the new one
                // Since we can't modify the pool directly with the new API,
                // we need to track this transformation separately
                self.transformed_exprs.insert(*expr_ref, new_expr);
            }
        }
        Ok(())
    }

    /// Apply all accumulated expression transformations to the expression pool
    pub fn apply_expr_transformations(&mut self) {
        for (expr_ref, new_expr) in &self.transformed_exprs.clone() {
            self.core.expr_pool.update(expr_ref, new_expr.clone());
        }
        self.transformed_exprs.clear();
    }

    /// Update variable type in context if identifier was type-converted
    pub fn update_identifier_types(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
                if let Expr::Identifier(name) = expr {
                    // Update the variable's type
                    self.context.update_var_type(name, resolved_ty.clone());
                }
            }
        }
        Ok(())
    }

    /// Record Number usage context for both identifiers and direct Number literals
    pub fn record_number_usage_context(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
                match expr {
                    Expr::Identifier(name) => {
                        // Find all Number expressions that might belong to this variable
                        // and record the context type
                        for i in 0..self.core.expr_pool.len() {
                            if let Some(candidate_expr) = self.core.expr_pool.get(&ExprRef(i as u32)) {
                                if let Expr::Number(_) = candidate_expr {
                                    let candidate_ref = ExprRef(i as u32);
                                    // Check if this Number might be associated with this variable
                                    if self.is_number_for_variable(name, &candidate_ref) {
                                        self.type_inference.number_usage_context.push((candidate_ref, resolved_ty.clone()));
                                    }
                                }
                            }
                        }
                    }
                    Expr::Number(_) => {
                        // Direct Number literal - record its resolved type
                        self.type_inference.number_usage_context.push((expr_ref.clone(), resolved_ty.clone()));
                    }
                    _ => {}
                }
            }
        }
        
        Ok(())
    }

    /// Check if an expression contains Number literals
    pub fn has_number_in_expr(&self, expr_ref: &ExprRef) -> bool {
        if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
            match expr {
                Expr::Number(_) => true,
                _ => false, // For now, only check direct Number literals
            }
        } else {
            false
        }
    }

    /// Check if a Number expression is associated with a specific variable
    pub fn is_number_for_variable(&self, var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Use the recorded mapping to check if this Number expression belongs to this variable
        if let Some(mapped_expr_ref) = self.type_inference.variable_expr_mapping.get(&var_name) {
            return mapped_expr_ref == number_expr_ref;
        }
        false
    }
    
    /// Check if an old Number expression might be associated with a variable for cleanup
    pub fn is_old_number_for_variable(&self, _var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Check if this Number expression was previously mapped to this variable
        // This is used for cleanup when variables are redefined
        if let Some(expr) = self.core.expr_pool.get(&number_expr_ref) {
            if let Expr::Number(_) = expr {
                // For now, we'll be conservative and remove all Number contexts when variables are redefined
                return true;
            }
        }
        false
    }

    /// Propagate concrete type to Number variable immediately
    pub fn propagate_to_number_variable(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
            if let Expr::Identifier(name) = expr {
                if let Some(var_type) = self.context.get_var(name) {
                    if var_type == TypeDecl::Number {
                        // Find and record the Number expression for this variable
                        for i in 0..self.core.expr_pool.len() {
                            if let Some(candidate_expr) = self.core.expr_pool.get(&ExprRef(i as u32)) {
                                if let Expr::Number(_) = candidate_expr {
                                    let candidate_ref = ExprRef(i as u32);
                                    if self.is_number_for_variable(name, &candidate_ref) {
                                        self.type_inference.number_usage_context.push((candidate_ref, target_type.clone()));
                                        // Update variable type in context
                                        self.context.update_var_type(name, target_type.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Finalize any remaining Number types with context-aware inference
    pub fn finalize_number_types(&mut self) -> Result<(), TypeCheckError> {
        // Use recorded context information to transform Number expressions
        let context_info = self.type_inference.number_usage_context.clone();
        for (expr_ref, target_type) in &context_info {
            if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
                if let Expr::Number(_) = expr {
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping.clone() {
                        if mapped_expr_ref == expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
            }
        }
        
        // Second pass: handle any remaining Number types by using variable context
        let expr_len = self.core.expr_pool.len();
        for i in 0..expr_len {
            if let Some(expr) = self.core.expr_pool.get(&ExprRef(i as u32)) {
                if let Expr::Number(_) = expr {
                    let expr_ref = ExprRef(i as u32);
                    
                    // Skip if already processed in first pass
                    let already_processed = context_info.iter().any(|(processed_ref, _)| processed_ref == &expr_ref);
                    if already_processed {
                        continue;
                    }
                    
                    // Find if this Number is associated with a variable and use its final type
                    // Use type hint if available, otherwise determine based on the literal value
                    let mut target_type = if let Some(hint) = self.type_inference.type_hint.clone() {
                        hint
                    } else {
                        // Check if the number is negative by looking at the actual value
                        if let Expr::Number(value) = expr {
                            let num_str = self.core.string_interner.resolve(value)
                                .unwrap_or("");
                            if num_str.starts_with('-') {
                                TypeDecl::Int64  // Negative numbers default to Int64
                            } else {
                                TypeDecl::UInt64  // Positive numbers default to UInt64
                            }
                        } else {
                            TypeDecl::UInt64  // Fallback
                        }
                    };
                    
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping {
                        if mapped_expr_ref == &expr_ref {
                            // Check the current type of this variable in context
                            if let Some(var_type) = self.context.get_var(*var_name) {
                                if var_type != TypeDecl::Number {
                                    target_type = var_type;
                                    break;
                                }
                            }
                        }
                    }
                    
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping.clone() {
                        if mapped_expr_ref == &expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Helper method to resolve numeric types with automatic conversion
    pub fn resolve_numeric_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> Result<(TypeDecl, TypeDecl), TypeCheckError> {
        match (lhs_ty, rhs_ty) {
            // Both types are already concrete - no conversion needed
            (TypeDecl::UInt64, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Int64, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Bool, TypeDecl::Bool) => Ok((TypeDecl::Bool, TypeDecl::Bool)),
            (TypeDecl::String, TypeDecl::String) => Ok((TypeDecl::String, TypeDecl::String)),
            
            // Number type automatic conversion
            (TypeDecl::Number, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::UInt64, TypeDecl::Number) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Number, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Int64, TypeDecl::Number) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            
            // Two Number types - check if we have a context hint, otherwise default to UInt64
            (TypeDecl::Number, TypeDecl::Number) => {
                if let Some(hint) = &self.type_inference.type_hint {
                    match hint {
                        TypeDecl::Int64 => Ok((TypeDecl::Int64, TypeDecl::Int64)),
                        TypeDecl::UInt64 => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                        _ => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                    }
                } else {
                    Ok((TypeDecl::UInt64, TypeDecl::UInt64))
                }
            },
            
            // Cross-type operations (UInt64 vs Int64) - generally not allowed for safety
            (TypeDecl::UInt64, TypeDecl::Int64) | (TypeDecl::Int64, TypeDecl::UInt64) => {
                Err(TypeCheckError::type_mismatch_operation("mixed signed/unsigned", lhs_ty.clone(), rhs_ty.clone()))
            },
            
            // Other type mismatches
            _ => {
                if lhs_ty == rhs_ty {
                    Ok((lhs_ty.clone(), rhs_ty.clone()))
                } else {
                    Err(TypeCheckError::type_mismatch(lhs_ty.clone(), rhs_ty.clone()))
                }
            }
        }
    }
    
    /// Propagate type to Number expression and associated variables
    pub fn propagate_type_to_number_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(&expr_ref) {
            match expr {
                Expr::Identifier(name) => {
                    // If this is an identifier with Number type, update it
                    if let Some(var_type) = self.context.get_var(name) {
                        if var_type == TypeDecl::Number {
                            self.context.update_var_type(name, target_type.clone());
                            // Also record for Number expression transformation
                            if let Some(mapped_expr) = self.type_inference.variable_expr_mapping.get(&name) {
                                self.type_inference.number_usage_context.push((mapped_expr.clone(), target_type.clone()));
                            }
                        }
                    }
                },
                Expr::Number(_) => {
                    // Direct Number literal
                    self.type_inference.number_usage_context.push((expr_ref.clone(), target_type.clone()));
                },
                _ => {
                    // For other expression types, we might need to recurse
                }
            }
        }
        Ok(())
    }
}