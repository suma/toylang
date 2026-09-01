use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

/// Type conversion and transformation implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Rewrite a `TypeDecl::Identifier(s)` to `TypeDecl::Generic(s)`
    /// when `s` is one of the generic params currently in scope
    /// (impl-level params from `current_impl_generic_params`).
    /// The parser produces `Identifier(s)` for any annotation
    /// inside a method body because it doesn't thread the impl's
    /// generic context that far; the type checker sees the same
    /// symbol reach the val/var validation step where
    /// `are_types_compatible(Identifier(V), UInt64)` would fail
    /// even though `Generic(V)` matches anything. This helper
    /// patches the form so downstream comparisons see the right
    /// variant.
    pub fn normalize_generic_identifier(&self, ty: &TypeDecl) -> TypeDecl {
        if let TypeDecl::Identifier(sym) = ty
            && let Some(params) = &self.context.current_impl_generic_params
                && params.contains(sym) {
                    return TypeDecl::Generic(*sym);
                }
        ty.clone()
    }

    /// Set up type hint for variable declarations based on explicit type annotation
    pub fn setup_type_hint_for_val(&mut self, type_decl: &Option<TypeDecl>) -> Option<TypeDecl> {
        let old_hint = self.type_inference.type_hint.clone();

        if let Some(decl_in) = type_decl {
            // If the annotation is an Identifier that names an
            // impl-level generic param, treat it as Generic so
            // downstream consumers (PtrRead's `Generic(_)` hint
            // match, type-compat) see it as the generic it
            // semantically is.
            let normalized = self.normalize_generic_identifier(decl_in);
            let decl = &normalized;
            match decl {
                TypeDecl::Array(element_types, _, _)
                    // For array types (including struct arrays), set the array type as hint for array literal processing
                    if !element_types.is_empty() => {
                        self.type_inference.type_hint = Some(decl.clone());
                    },
                TypeDecl::Struct(_, _) => {
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
            self.type_inference.variable_expr_mapping.insert(name, *expr_ref);
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
            if let Some(hint) = self.type_inference.type_hint.clone()
                && matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    // Transform Number to hinted type
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
        } else if type_decl.as_ref().is_some_and(|decl| *decl == TypeDecl::Unknown) && *expr_ty == TypeDecl::Int64 {
            // Unknown type declaration with Int64 inference - also transform
            if let Some(hint) = self.type_inference.type_hint.clone()
                && matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
        } else if let Some(decl) = type_decl
            && decl != &TypeDecl::Unknown && decl != &TypeDecl::Number && *expr_ty == *decl {
                // Expression returned the hinted type, transform Number literals to concrete type
                if let Some(expr) = self.core.expr_pool.get(expr_ref)
                    && let Expr::Number(_) = expr {
                        self.transform_numeric_expr(expr_ref, decl)?;
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
        if let Some(expr) = self.core.expr_pool.get(expr_ref)
            && let Expr::Number(value) = expr {
                let num_str_owned = self.core.string_interner.resolve(value)
                    .ok_or_else(|| TypeCheckError::generic_error("Failed to resolve number literal"))?
                    .to_string();
                // Numeric literal separators: `_` between digits is
                // legal in source (`1_000_000`) but `str::parse` and
                // `from_str_radix` don't accept it. Strip every `_`
                // before invoking either.
                let cleaned = num_str_owned.replace("_", "");
                let num_str = cleaned.as_str();
                let num_orig = num_str_owned.as_str();

                // NUM-W: shared literal parsing for the narrow widths.
                // Parse at the widest signed/unsigned width, then
                // range-check, so `300` for a `u8` parameter reports a
                // conversion error rather than silently wrapping.
                let ty_name = self.type_name_for_error(target_type);
                let parse_unsigned = |max: u128| -> Result<u128, TypeCheckError> {
                    let v = if let Some(hex) = num_str.strip_prefix("0x").or_else(|| num_str.strip_prefix("0X")) {
                        u128::from_str_radix(hex, 16)
                    } else {
                        num_str.parse::<u128>()
                    }
                    .map_err(|_| TypeCheckError::conversion_error(num_orig, &ty_name))?;
                    if v > max {
                        return Err(TypeCheckError::conversion_error(num_orig, &ty_name));
                    }
                    Ok(v)
                };
                let parse_signed = |min: i128, max: i128| -> Result<i128, TypeCheckError> {
                    let v = if let Some(hex) = num_str.strip_prefix("0x").or_else(|| num_str.strip_prefix("0X")) {
                        i128::from_str_radix(hex, 16)
                    } else {
                        num_str.parse::<i128>()
                    }
                    .map_err(|_| TypeCheckError::conversion_error(num_orig, &ty_name))?;
                    if v < min || v > max {
                        return Err(TypeCheckError::conversion_error(num_orig, &ty_name));
                    }
                    Ok(v)
                };

                // Create the new expression based on target type
                let new_expr = match target_type {
                    TypeDecl::UInt64 => {
                        let val = if num_str.starts_with("0x") || num_str.starts_with("0X") {
                            // Parse hexadecimal literal
                            u64::from_str_radix(&num_str[2..], 16)
                                .map_err(|_| TypeCheckError::conversion_error(num_orig, "UInt64"))?
                        } else {
                            // Parse decimal literal
                            num_str.parse::<u64>()
                                .map_err(|_| TypeCheckError::conversion_error(num_orig, "UInt64"))?
                        };
                        Expr::UInt64(val)
                    },
                    TypeDecl::Int64 => {
                        let val = if num_str.starts_with("0x") || num_str.starts_with("0X") {
                            // Parse hexadecimal literal
                            i64::from_str_radix(&num_str[2..], 16)
                                .map_err(|_| TypeCheckError::conversion_error(num_orig, "Int64"))?
                        } else {
                            // Parse decimal literal
                            num_str.parse::<i64>()
                                .map_err(|_| TypeCheckError::conversion_error(num_orig, "Int64"))?
                        };
                        Expr::Int64(val)
                    },
                    // NUM-W: the same coercion for the narrow widths.
                    // Reached from NUMBER-HINT positions (`f(3)` where
                    // the parameter is `i8`); out-of-range literals get
                    // the same conversion error as the wide types.
                    TypeDecl::UInt8 => Expr::UInt8(parse_unsigned(u8::MAX as u128)? as u8),
                    TypeDecl::UInt16 => Expr::UInt16(parse_unsigned(u16::MAX as u128)? as u16),
                    TypeDecl::UInt32 => Expr::UInt32(parse_unsigned(u32::MAX as u128)? as u32),
                    TypeDecl::Int8 => Expr::Int8(parse_signed(i8::MIN as i128, i8::MAX as i128)? as i8),
                    TypeDecl::Int16 => Expr::Int16(parse_signed(i16::MIN as i128, i16::MAX as i128)? as i16),
                    TypeDecl::Int32 => Expr::Int32(parse_signed(i32::MIN as i128, i32::MAX as i128)? as i32),
                    _ => {
                        return Err(TypeCheckError::unsupported_operation("transform", target_type.clone()));
                    }
                };
                
                // Replace the expression in the pool with the new one
                // Since we can't modify the pool directly with the new API,
                // we need to track this transformation separately
                self.transformed_exprs.insert(*expr_ref, new_expr);
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
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number
            && let Some(expr) = self.core.expr_pool.get(expr_ref)
                && let Expr::Identifier(name) = expr {
                    // Update the variable's type
                    self.context.update_var_type(name, resolved_ty.clone());
                }
        Ok(())
    }

    /// Record Number usage context for both identifiers and direct Number literals
    pub fn record_number_usage_context(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number
            && let Some(expr) = self.core.expr_pool.get(expr_ref) {
                match expr {
                    Expr::Identifier(name) => {
                        // FRONTEND-PERF: `is_number_for_variable` matches
                        // only the single Number expr this variable is
                        // mapped to, so the old full-pool scan was a
                        // one-entry lookup in disguise.
                        if let Some(&candidate_ref) = self.type_inference.variable_expr_mapping.get(&name)
                            && let Some(Expr::Number(_)) = self.core.expr_pool.get(&candidate_ref) {
                                self.type_inference.number_usage_context.push((candidate_ref, resolved_ty.clone()));
                            }
                    }
                    Expr::Number(_) => {
                        // Direct Number literal - record its resolved type
                        self.type_inference.number_usage_context.push((*expr_ref, resolved_ty.clone()));
                    }
                    _ => {}
                }
            }
        
        Ok(())
    }

    /// Check if an expression contains Number literals
    pub fn has_number_in_expr(&self, expr_ref: &ExprRef) -> bool {
        if let Some(expr) = self.core.expr_pool.get(expr_ref) {
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
        if let Some(expr) = self.core.expr_pool.get(number_expr_ref)
            && let Expr::Number(_) = expr {
                // For now, we'll be conservative and remove all Number contexts when variables are redefined
                return true;
            }
        false
    }

    /// Propagate concrete type to Number variable immediately
    pub fn propagate_to_number_variable(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(expr_ref)
            && let Expr::Identifier(name) = expr
                && let Some(var_type) = self.context.get_var(name)
                    && var_type == TypeDecl::Number {
                        // FRONTEND-PERF: same one-entry lookup as
                        // `record_number_usage_context` — the old
                        // full-pool scan matched only this.
                        if let Some(&candidate_ref) = self.type_inference.variable_expr_mapping.get(&name)
                            && let Some(Expr::Number(_)) = self.core.expr_pool.get(&candidate_ref) {
                                self.type_inference.number_usage_context.push((candidate_ref, target_type.clone()));
                                // Update variable type in context
                                self.context.update_var_type(name, target_type.clone());
                            }
                    }
        Ok(())
    }

    /// Finalize any remaining Number types with context-aware inference
    pub fn finalize_number_types(&mut self) -> Result<(), TypeCheckError> {
        // Use recorded context information to transform Number expressions
        let context_info = self.type_inference.number_usage_context.clone();

        // FRONTEND-PERF: the old code scanned `variable_expr_mapping`
        // (cloned, per entry!) to find which variables map to each
        // Number node. `finalize_number_types` runs once per function,
        // so those clones made it O(pool × mapping) per function, on top
        // of the whole-pool scan in the second pass. Build the
        // expr -> vars reverse map once — iteration order is preserved
        // so the "first concrete-typed variable wins" logic below is
        // unchanged.
        let mut expr_to_vars: std::collections::HashMap<ExprRef, Vec<DefaultSymbol>> =
            std::collections::HashMap::with_capacity(self.type_inference.variable_expr_mapping.len());
        for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping {
            expr_to_vars.entry(*mapped_expr_ref).or_default().push(*var_name);
        }

        for (expr_ref, target_type) in &context_info {
            if let Some(expr) = self.core.expr_pool.get(expr_ref)
                && let Expr::Number(_) = expr {
                    self.transform_numeric_expr(expr_ref, target_type)?;
                    // Update variable types in context if this expression is mapped to a variable
                    if let Some(vars) = expr_to_vars.get(expr_ref) {
                        for var_name in vars {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
        }
        
        // Second pass: handle any remaining Number types by using variable context.
        // NUMBER-HINT: only the nodes *this* function's body reached.
        // This used to iterate every Number node in the pool, which
        // made the first function checked decide the type of every
        // unsuffixed literal in the program — including ones in
        // functions not yet visited, whose parameter and return types
        // would have named a better answer. `visited_numbers` is
        // saved/restored around each function check, so what is left
        // here is exactly this body's leftovers.
        let processed: std::collections::HashSet<ExprRef> =
            context_info.iter().map(|(r, _)| *r).collect();
        let number_exprs = std::mem::take(&mut self.type_inference.visited_numbers);
        for &expr_ref in &number_exprs {
            if let Some(expr) = self.core.expr_pool.get(&expr_ref)
                && let Expr::Number(_) = expr {
                    // Skip if already processed in first pass, or
                    // already claimed by a position that knew what it
                    // expected (NUMBER-HINT). `transformed_exprs` is
                    // this function's pending rewrites; it is applied
                    // and cleared right after finalization, so an
                    // entry here always means "decided, not defaulted".
                    if processed.contains(&expr_ref)
                        || self.transformed_exprs.contains_key(&expr_ref)
                    {
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

                    if let Some(vars) = expr_to_vars.get(&expr_ref) {
                        for var_name in vars {
                            // Check the current type of this variable in context
                            if let Some(var_type) = self.context.get_var(*var_name)
                                && var_type != TypeDecl::Number {
                                    target_type = var_type;
                                    break;
                                }
                        }
                    }
                    
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    if let Some(vars) = expr_to_vars.get(&expr_ref) {
                        for var_name in vars {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
        }
        Ok(())
    }

    /// Helper method to resolve numeric types with automatic conversion
    pub fn resolve_numeric_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> Result<(TypeDecl, TypeDecl), TypeCheckError> {
        // First, try to resolve generic types to concrete types
        let resolved_lhs = if let TypeDecl::Generic(param) = lhs_ty {
            self.type_inference.lookup_generic_type(*param).unwrap_or_else(|| lhs_ty.clone())
        } else {
            lhs_ty.clone()
        };

        let resolved_rhs = if let TypeDecl::Generic(param) = rhs_ty {
            self.type_inference.lookup_generic_type(*param).unwrap_or_else(|| rhs_ty.clone())
        } else {
            rhs_ty.clone()
        };

        match (&resolved_lhs, &resolved_rhs) {
            // Both types are already concrete - no conversion needed
            (TypeDecl::UInt64, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Int64, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Float64, TypeDecl::Float64) => Ok((TypeDecl::Float64, TypeDecl::Float64)),
            // SIMD-F32: same-width float pair. Mixing f32 with f64 (or
            // with any integer) stays rejected — cross-width moves go
            // through an explicit `as`, matching the NUM-W rule.
            (TypeDecl::Float32, TypeDecl::Float32) => Ok((TypeDecl::Float32, TypeDecl::Float32)),
            (TypeDecl::Bool, TypeDecl::Bool) => Ok((TypeDecl::Bool, TypeDecl::Bool)),
            (TypeDecl::String, TypeDecl::String) => Ok((TypeDecl::String, TypeDecl::String)),

            // Number type automatic conversion
            (TypeDecl::Number, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::UInt64, TypeDecl::Number) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Number, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Int64, TypeDecl::Number) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            // NUM-W: the same for the narrow widths. A suffix-less
            // literal takes the width of the value it meets, so a
            // byte can be compared against `'0'` or `48` rather than
            // only against `48u8`. Without this arm the pair fell
            // through to the strict `==` below and reported
            // "expected u8, but got Number" — a type the user never
            // wrote, on the operand that was not the literal. The
            // literal's value is range-checked when
            // `transform_numeric_expr` narrows the AST node, so
            // `b == 300` on a `u8` is still an error.
            (TypeDecl::Number, other) | (other, TypeDecl::Number) if other.is_integer() => {
                Ok((other.clone(), other.clone()))
            }
            // Number is integer-flavored; mixing with Float64 is rejected so users
            // are forced to write `1.0f64` or cast explicitly. This avoids surprise
            // when an integer literal silently becomes a float on the other side.

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
                Err(TypeCheckError::type_mismatch_operation("mixed signed/unsigned", resolved_lhs.clone(), resolved_rhs.clone()))
            },

            // Generic types - if both are the same generic parameter, allow the operation
            (TypeDecl::Generic(left_param), TypeDecl::Generic(right_param)) => {
                if left_param == right_param {
                    Ok((resolved_lhs.clone(), resolved_rhs.clone()))
                } else {
                    Err(TypeCheckError::type_mismatch(resolved_lhs.clone(), resolved_rhs.clone()))
                }
            },

            // Bare identifier vs generic parameter of the same name:
            // a mention of `K` in an expression resolves to the
            // `Identifier` form while the binding itself is
            // `Generic(K)`. `Dict::get`'s `existing == key` (a
            // generic key compared against another key value) hit
            // exactly this pair — the strict `==` fallback below
            // rejected it once if/elif conditions started being
            // checked (mirrors `is_equivalent`'s leniency, without
            // its generic-wildcard slack).
            (TypeDecl::Identifier(s1), TypeDecl::Generic(s2))
            | (TypeDecl::Generic(s1), TypeDecl::Identifier(s2))
                if s1 == s2 =>
            {
                Ok((resolved_lhs.clone(), resolved_rhs.clone()))
            },

            // A user type written in an annotation arrives as the bare
            // `Identifier(name)` — the parser cannot tell a struct from
            // an enum — while a *value* of that type carries the
            // resolved `Struct(name, args)` / `Enum(name, args)` form.
            // `is_equivalent` already unifies the pair, but the strict
            // `==` fallback below did not, so a binary expression with
            // exactly one annotated side was rejected as
            // "expected P, but got P" (`val h: P = f + P { .. }`, and
            // through the desugar every `f += P { .. }`). Adopt the
            // resolved form on both sides so the operator-overload
            // dispatch downstream sees the type arguments.
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2, _) | TypeDecl::Enum(s2, _))
                if s1 == s2 =>
            {
                Ok((resolved_rhs.clone(), resolved_rhs.clone()))
            },
            (TypeDecl::Struct(s1, _) | TypeDecl::Enum(s1, _), TypeDecl::Identifier(s2))
                if s1 == s2 =>
            {
                Ok((resolved_lhs.clone(), resolved_lhs.clone()))
            },

            // Allocator handle vs generic parameter bounded by Allocator — pass through so
            // the caller's comparison/op logic can validate (e.g. `current_allocator() == a`
            // inside a `<A: Allocator>` function body).
            (TypeDecl::Allocator, TypeDecl::Generic(sym))
            | (TypeDecl::Generic(sym), TypeDecl::Allocator) => {
                if matches!(
                    self.context.current_fn_generic_bounds.get(sym),
                    Some(TypeDecl::Allocator)
                ) {
                    Ok((resolved_lhs.clone(), resolved_rhs.clone()))
                } else {
                    Err(TypeCheckError::type_mismatch(resolved_lhs.clone(), resolved_rhs.clone()))
                }
            },

            // Other type mismatches
            _ => {
                if resolved_lhs == resolved_rhs {
                    Ok((resolved_lhs.clone(), resolved_rhs.clone()))
                } else {
                    Err(TypeCheckError::type_mismatch(resolved_lhs.clone(), resolved_rhs.clone()))
                }
            }
        }
    }
    
    /// Propagate type to Number expression and associated variables
    pub fn propagate_type_to_number_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(expr_ref) {
            match expr {
                Expr::Identifier(name) => {
                    // If this is an identifier with Number type, update it
                    if let Some(var_type) = self.context.get_var(name)
                        && var_type == TypeDecl::Number {
                            self.context.update_var_type(name, target_type.clone());
                            // Also record for Number expression transformation
                            if let Some(mapped_expr) = self.type_inference.variable_expr_mapping.get(&name) {
                                self.type_inference.number_usage_context.push((*mapped_expr, target_type.clone()));
                            }
                        }
                },
                Expr::Number(_) => {
                    // Direct Number literal
                    self.type_inference.number_usage_context.push((*expr_ref, target_type.clone()));
                },
                _ => {
                    // For other expression types, we might need to recurse
                }
            }
        }
        Ok(())
    }

    /// NUMBER-HINT: remember that this function's body reached an
    /// expression still carrying the unresolved-literal placeholder,
    /// so `finalize_number_types` knows the node is *this* function's
    /// to default. Cheap no-op for every other type.
    pub fn note_visited_number(&mut self, expr_ref: &ExprRef, ty: &TypeDecl) {
        if *ty == TypeDecl::Number {
            self.type_inference.visited_numbers.push(*expr_ref);
        }
    }

    /// NUMBER-HINT: is `ty` an integer type an unsuffixed literal is
    /// allowed to land in? `Number` itself is excluded — it is the
    /// placeholder, not a destination.
    pub fn is_integer_target(ty: &TypeDecl) -> bool {
        matches!(
            ty,
            TypeDecl::UInt64
                | TypeDecl::Int64
                | TypeDecl::UInt8
                | TypeDecl::UInt16
                | TypeDecl::UInt32
                | TypeDecl::Int8
                | TypeDecl::Int16
                | TypeDecl::Int32
        )
    }

    /// NUMBER-HINT: claim an expression that type-checked to the
    /// unresolved-literal placeholder `Number` for `target`.
    ///
    /// A position that *knows* what it expects — a declared return
    /// type, a parameter type — calls this so the literals in that
    /// expression resolve to the expected type instead of waiting for
    /// `finalize_number_types` to apply the bare default. Without it
    /// `fn main() -> i64 { 0 }` reported "expected i64, but got
    /// Number" (or `u64` once the default had run), forcing a suffix
    /// on every literal in a return or argument position.
    ///
    /// Returns the type the expression now has: `target` when the
    /// coercion applied, `ty` unchanged otherwise.
    /// A char literal takes the integer type the position asks for,
    /// as long as its code point fits (CHAR-LITERAL-NUM).
    ///
    /// A char literal is held as `u32` — that is what `val c = 'a'`
    /// infers and what the `char` alias names — but a position that
    /// wants another integer width may have it: `val b: u8 = '0'`,
    /// `byte == 'h'`, a `i64` parameter. This is the one exception to
    /// the NUM-W rule that integer types never convert implicitly,
    /// and it is deliberately narrow: only a literal *written as a
    /// character* qualifies. A suffixed literal still needs an `as`,
    /// because its suffix already named its type — nothing was left
    /// to decide.
    ///
    /// The node is rewritten to the concrete width, so backends see
    /// an ordinary literal of the target type. `Ok(None)` means the
    /// expression was not a char literal, or the target was not an
    /// integer type to claim it.
    pub fn coerce_char_literal(
        &mut self,
        expr_ref: &ExprRef,
        target: &TypeDecl,
    ) -> Result<Option<TypeDecl>, TypeCheckError> {
        let Some(Expr::CharLiteral(code_point)) = self.core.expr_pool.get(expr_ref) else {
            return Ok(None);
        };
        if !Self::is_integer_target(target) || *target == TypeDecl::UInt32 {
            return Ok(None);
        }
        let value = code_point as i128;
        let fits = match target {
            TypeDecl::UInt64 | TypeDecl::Int64 => true,
            TypeDecl::UInt16 => value <= u16::MAX as i128,
            TypeDecl::UInt8 => value <= u8::MAX as i128,
            TypeDecl::Int32 => value <= i32::MAX as i128,
            TypeDecl::Int16 => value <= i16::MAX as i128,
            TypeDecl::Int8 => value <= i8::MAX as i128,
            _ => false,
        };
        if !fits {
            // The same report an out-of-range integer literal gets:
            // the value, and the type it will not fit in.
            return Err(TypeCheckError::conversion_error(
                &code_point.to_string(),
                &self.type_name_for_error(target),
            ));
        }
        let rewritten = match target {
            TypeDecl::UInt64 => Expr::UInt64(code_point as u64),
            TypeDecl::UInt16 => Expr::UInt16(code_point as u16),
            TypeDecl::UInt8 => Expr::UInt8(code_point as u8),
            TypeDecl::Int64 => Expr::Int64(code_point as i64),
            TypeDecl::Int32 => Expr::Int32(code_point as i32),
            TypeDecl::Int16 => Expr::Int16(code_point as i16),
            TypeDecl::Int8 => Expr::Int8(code_point as i8),
            _ => return Ok(None),
        };
        self.core.expr_pool.update(expr_ref, rewritten);
        self.type_inference.set_expr_type(*expr_ref, target.clone());
        self.optimization.cache_type(*expr_ref, target.clone());
        Ok(Some(target.clone()))
    }

    pub fn coerce_number_expr(
        &mut self,
        expr_ref: &ExprRef,
        ty: &TypeDecl,
        target: &TypeDecl,
    ) -> Result<TypeDecl, TypeCheckError> {
        // CHAR-LITERAL-NUM: a char literal arrives with a concrete
        // `u32` type rather than `Number`, and the position still
        // gets to claim it when the code point fits.
        if let Some(coerced) = self.coerce_char_literal(expr_ref, target)? {
            return Ok(coerced);
        }
        if *ty != TypeDecl::Number {
            return Ok(ty.clone());
        }
        let target = if Self::is_integer_target(target) {
            target.clone()
        } else {
            // The target is not an integer type, and an integer
            // literal can never become one, so this is a mismatch.
            // Settle the literal on its default anyway: the caller is
            // about to report the error, and `expected bool, but got
            // u64` names a type the reader can act on where `got
            // Number` names an internal placeholder. Leaving it
            // unresolved also let the default pass trip over it later
            // ("Unsupported operation 'transform' for type Unknown"),
            // burying the real diagnostic under a cascade.
            //
            // A target that is still generic is *not* a mismatch —
            // the literal is what decides the parameter — so it keeps
            // its placeholder and the usual inference runs.
            let normalized = self.normalize_generic_identifier(target);
            if normalized.contains_generic()
                || matches!(
                    normalized,
                    TypeDecl::Unknown | TypeDecl::Number | TypeDecl::Self_
                )
            {
                return Ok(ty.clone());
            }
            TypeDecl::UInt64
        };
        let target = &target;
        // Claim the type only if every literal underneath was
        // actually rewritten. Reporting the resolved type while
        // leaving an `Expr::Number` in the pool type-checks a program
        // no backend can run ("Internal error: Expr::Number should be
        // transformed to concrete type during type checking"), which
        // is strictly worse than making the author write a suffix.
        if !self.propagate_number_subtree(expr_ref, target)? {
            return Ok(ty.clone());
        }
        // The visit cached `Number` for this node; later readers
        // (the return-type comparison, an enclosing block) must see
        // the resolved type instead.
        self.type_inference.set_expr_type(*expr_ref, target.clone());
        self.optimization.cache_type(*expr_ref, target.clone());
        Ok(target.clone())
    }

    /// Walk the structure of an expression whose type came back as
    /// `Number`, rewriting every literal leaf to `target`.
    ///
    /// Returns whether the whole subtree was accounted for. A shape
    /// this does not know how to descend into returns `false` so the
    /// caller leaves the expression alone rather than claiming a type
    /// for literals it never rewrote.
    fn propagate_number_subtree(
        &mut self,
        expr_ref: &ExprRef,
        target: &TypeDecl,
    ) -> Result<bool, TypeCheckError> {
        let Some(expr) = self.core.expr_pool.get(expr_ref) else {
            return Ok(false);
        };
        match expr {
            // Transform the leaf right here rather than queueing it in
            // `number_usage_context`: that list is pruned whenever a
            // later `val` redefines a binding, which silently dropped
            // the record and let the default pass claim the literal
            // back. A direct rewrite cannot be undone that way.
            Expr::Number(_) => {
                self.transform_numeric_expr(expr_ref, target)?;
                Ok(true)
            }
            Expr::Identifier(name) => {
                if self.context.get_var(name) == Some(TypeDecl::Number) {
                    self.context.update_var_type(name, target.clone());
                    if let Some(mapped) =
                        self.type_inference.variable_expr_mapping.get(&name).copied()
                    {
                        self.transform_numeric_expr(&mapped, target)?;
                    }
                }
                Ok(true)
            }
            Expr::Binary(_, lhs, rhs) => {
                let l = self.propagate_number_subtree(&lhs, target)?;
                let r = self.propagate_number_subtree(&rhs, target)?;
                Ok(l && r)
            }
            Expr::Unary(_, operand) => self.propagate_number_subtree(&operand, target),
            // A block's value is its tail expression, so that is where
            // the literal to rewrite lives — as in a closure body
            // (`fn() -> i64 { 5 }`) or a braced branch.
            Expr::Block(statements) => match statements.last() {
                Some(last) => match self.core.stmt_pool.get(last) {
                    Some(Stmt::Expression(e)) => self.propagate_number_subtree(&e, target),
                    _ => Ok(false),
                },
                None => Ok(false),
            },
            // Every branch of an `if` / `match` contributes a value of
            // the expression's type, so all of them carry literals to
            // rewrite.
            Expr::IfElifElse(_, then_block, elifs, else_block) => {
                let mut all = self.propagate_number_subtree(&then_block, target)?;
                for (_, block) in &elifs {
                    all &= self.propagate_number_subtree(block, target)?;
                }
                all &= self.propagate_number_subtree(&else_block, target)?;
                Ok(all)
            }
            Expr::Match(_, arms) => {
                let mut all = true;
                for arm in &arms {
                    all &= self.propagate_number_subtree(&arm.body, target)?;
                }
                Ok(all)
            }
            _ => Ok(false),
        }
    }
}
