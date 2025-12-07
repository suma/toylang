use crate::ast::{ExprRef, Function, MethodFunction};
use crate::type_checker::{TypeCheckError, TypeDecl, TypeCheckerVisitor};
use crate::type_checker::context::StructDefinition;
use crate::visitor::AstVisitor;
use string_interner::DefaultSymbol;
use std::rc::Rc;
use std::collections::HashMap;

/// Extension trait for generic type checking functionality
pub trait GenericTypeChecking {
    /// Handle generic function calls with type inference and instantiation recording
    fn visit_generic_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef, fun: &Function) -> Result<TypeDecl, TypeCheckError>;
    
    /// Handle generic struct literal type inference  
    fn visit_generic_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>, 
                                   struct_definition: &StructDefinition, 
                                   generic_params: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError>;
    
    /// Handle generic method calls
    fn handle_generic_method_call(&mut self, struct_name: DefaultSymbol, method_name: &str, 
                                 method_return_type: &TypeDecl, obj: &ExprRef, args: &Vec<ExprRef>, 
                                 arg_types: &[TypeDecl]) -> Result<TypeDecl, TypeCheckError>;
    
    /// Handle generic associated function calls (like Container::new) with type inference
    fn handle_generic_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, 
                                             args: &Vec<ExprRef>, method: &Rc<MethodFunction>) -> Result<TypeDecl, TypeCheckError>;
    
    /// Generate a unique name for an instantiated generic function/struct
    fn generate_instantiated_name(&self, original_name: DefaultSymbol, substitutions: &HashMap<DefaultSymbol, TypeDecl>) -> String;
    
    /// Generate a unique name for instantiated generic struct
    fn generate_instantiated_struct_name(&self, struct_name: DefaultSymbol, substitutions: &HashMap<DefaultSymbol, TypeDecl>) -> String;
    
    /// Create type substitutions for methods
    fn create_type_substitutions_for_method(&self, generic_params: &[DefaultSymbol],
                                          struct_name: DefaultSymbol) -> Result<HashMap<DefaultSymbol, TypeDecl>, TypeCheckError>;
}

impl GenericTypeChecking for TypeCheckerVisitor<'_> {
    fn visit_generic_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef, fun: &Function) -> Result<TypeDecl, TypeCheckError> {
        use crate::ast::Expr;
        
        // Extract argument expressions from the reference
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
        
        let args = args_data.ok_or_else(|| {
            self.pop_context();
            TypeCheckError::generic_error("Invalid arguments expression")
        })?;
        
        // Verify argument count matches parameter count
        if args.len() != fun.parameter.len() {
            self.pop_context();
            let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
            return Err(TypeCheckError::generic_error(&format!(
                "Generic function '{}' argument count mismatch: expected {}, found {}",
                fn_name_str, fun.parameter.len(), args.len()
            )));
        }
        
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();
        
        // Collect argument types and add constraints
        let mut arg_types = Vec::new();
        for (i, (arg_expr, (_, param_type))) in args.iter().zip(&fun.parameter).enumerate() {
            let arg_type = self.visit_expr(arg_expr)?;
            arg_types.push(arg_type.clone());
            
            // Add constraint for parameter-argument type unification
            self.type_inference.add_constraint(
                param_type.clone(),
                arg_type,
                crate::type_checker::inference::ConstraintContext::FunctionCall {
                    function_name: fn_name,
                    arg_index: i,
                }
            );
        }
        
        // Solve constraints to get type substitutions
        let substitutions = match self.type_inference.solve_constraints() {
            Ok(solution) => solution,
            Err(e) => {
                self.pop_context();
                let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Type inference failed for generic function '{}': {}",
                    fn_name_str, e
                )));
            }
        };
        
        // Ensure all generic parameters have been inferred
        for generic_param in &fun.generic_params {
            if !substitutions.contains_key(generic_param) {
                self.pop_context();
                let param_name = self.core.string_interner.resolve(*generic_param).unwrap_or("<NOT_FOUND>");
                let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Cannot infer generic type parameter '{}' for function '{}'",
                    param_name, fn_name_str
                )));
            }
        }
        
        // Generate unique name for the instantiated function
        let _instantiated_name = self.generate_instantiated_name(fn_name, &substitutions);
        
        // Substitute generic types in return type with concrete types using the new inference engine
        let return_type = if let Some(ret_type) = &fun.return_type {
            self.type_inference.apply_solution(ret_type, &substitutions)
        } else {
            TypeDecl::Unknown
        };
        
        self.pop_context();
        Ok(return_type)
    }

    fn visit_generic_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>,
                                   struct_definition: &StructDefinition,
                                   generic_params: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();

        // Validate provided fields against struct definition
        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;
        
        // Check if generic parameters are already in scope (e.g., within a generic method body)
        // If all parameters can be found in outer scope, use them directly without constraint solving
        let mut all_in_scope = true;
        let mut outer_scope_types = Vec::new();

        for param in generic_params {
            if let Some(outer_type) = self.type_inference.lookup_generic_type(*param) {
                outer_scope_types.push(outer_type.clone());
            } else {
                all_in_scope = false;
                break;
            }
        }

        // If we're already in a generic scope (e.g., method body), directly use those types
        if all_in_scope && !outer_scope_types.is_empty() {
            // Type parameters are already resolved in outer scope, just return struct with those types
            return Ok(TypeDecl::Struct(*struct_name, outer_scope_types));
        }

        // Push generic parameters onto the scope for proper resolution
        let mut generic_scope = HashMap::new();
        for param in generic_params {
            generic_scope.insert(*param, TypeDecl::Generic(*param));
        }
        self.type_inference.push_generic_scope(generic_scope);
        let should_push_scope = true;  // We pushed a scope, so we need to pop it later

        // Collect field types and create constraints for type parameter inference
        let mut field_types = HashMap::new();

        for (field_name, field_expr) in fields {
            // Find expected field type from struct definition
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("<unknown>");
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);
            
            if let Some(expected_type) = expected_field_type {
                // Type check the field expression without hint first
                let field_type = self.visit_expr(field_expr)?;
                
                
                // Add constraint for generic type unification
                self.type_inference.add_constraint(
                    expected_type.clone(),
                    field_type.clone(),
                    crate::type_checker::inference::ConstraintContext::FieldAccess {
                        struct_name: *struct_name,
                        field_name: *field_name,
                    }
                );
                
                field_types.insert(*field_name, field_type);
            }
        }
        
        // Solve constraints to get type substitutions
        let substitutions = match self.type_inference.solve_constraints() {
            Ok(solution) => {
                solution
            },
            Err(e) => {
                if should_push_scope {
                    self.type_inference.pop_generic_scope();
                }
                let struct_name_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<unknown>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Type inference failed for generic struct '{}': {}",
                    struct_name_str, e
                )));
            }
        };
        
        // Now verify field types with the resolved substitutions
        for (field_name, field_expr) in fields {
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("<unknown>");
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);
            
            if let Some(expected_type) = expected_field_type {
                // Apply substitutions to the expected type
                let substituted_expected = expected_type.substitute_generics(&substitutions);
                
                // Check type compatibility with substitution
                let field_type = self.visit_expr(field_expr)?;
                if !self.types_are_compatible(&field_type, &substituted_expected) {
                    if should_push_scope {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(TypeCheckError::type_mismatch(
                        substituted_expected,
                        field_type
                    ));
                }
            }
        }

        // Ensure all generic parameters have been inferred or can be found in outer scope
        for generic_param in generic_params {
            if !substitutions.contains_key(generic_param) {
                // Check if it's available in outer scope
                if self.type_inference.lookup_generic_type(*generic_param).is_none() {
                    if should_push_scope {
                        self.type_inference.pop_generic_scope();
                    }
                    let param_name = self.core.string_interner.resolve(*generic_param).unwrap_or("<unknown>");
                    let struct_name_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<unknown>");
                    return Err(TypeCheckError::generic_error(&format!(
                        "Cannot infer generic type parameter '{}' for struct '{}'",
                        param_name, struct_name_str
                    )));
                }
            }
        }

        // Record the type substitutions for later use in method calls
        // Implementation delegated to type inference engine

        // Pop the generic scope if we pushed it
        if should_push_scope {
            self.type_inference.pop_generic_scope();
        }
        
        // Return struct type with resolved type parameters
        let _instantiated_name_str = self.generate_instantiated_struct_name(*struct_name, &substitutions);
        
        // Preserve the order of generic parameters as defined in the struct
        let mut type_params = Vec::new();
        for generic_param in generic_params {
            if let Some(concrete_type) = substitutions.get(generic_param) {
                type_params.push(concrete_type.clone());
            } else {
                // Check if there's an existing generic substitution in the outer scope
                if let Some(outer_subst) = self.type_inference.lookup_generic_type(*generic_param) {
                    type_params.push(outer_subst.clone());
                } else {
                    // Fallback to Generic type if substitution is missing
                    type_params.push(TypeDecl::Generic(*generic_param));
                }
            }
        }

        Ok(TypeDecl::Struct(*struct_name, type_params))
    }

    fn handle_generic_method_call(&mut self, struct_name: DefaultSymbol, method_name: &str, 
                                 method_return_type: &TypeDecl, _obj: &ExprRef, _args: &Vec<ExprRef>, 
                                 _arg_types: &[TypeDecl]) -> Result<TypeDecl, TypeCheckError> {
        // Get the generic parameters for this struct
        let generic_params = self.context.get_struct_generic_params(struct_name)
            .cloned()
            .unwrap_or_default();
        
        
        // Create type substitutions based on recent generic inference
        let substitutions = self.create_type_substitutions_for_method(&generic_params, struct_name)?;
        
        
        // Apply substitutions to the method return type
        let substituted_return_type = method_return_type.substitute_generics(&substitutions);
        
        // Resolve Self type in the return type if present
        let resolved_return_type = match &substituted_return_type {
            TypeDecl::Self_ => {
                // For generic structs, return the substituted type with generic parameters resolved
                // For non-generic structs, return the simple struct type
                if !generic_params.is_empty() {
                    // This is a generic struct - build type params
                    let type_params: Vec<TypeDecl> = generic_params.iter().map(|param| {
                        substitutions.get(param).cloned()
                            .or_else(|| self.type_inference.lookup_generic_type(*param))
                            .unwrap_or_else(|| TypeDecl::Generic(*param))
                    }).collect();
                    TypeDecl::Struct(struct_name, type_params)
                } else {
                    // Non-generic struct - resolve Self to simple struct type
                    TypeDecl::Struct(struct_name, vec![])
                }
            }
            _ => substituted_return_type
        };
        
        
        Ok(resolved_return_type)
    }

    fn handle_generic_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, 
                                             args: &Vec<ExprRef>, method: &Rc<MethodFunction>) -> Result<TypeDecl, TypeCheckError> {
        // Get the generic parameters for this struct
        let generic_params = self.context.get_struct_generic_params(struct_name)
            .cloned()
            .unwrap_or_default();
        
        // Verify argument count matches parameter count
        if args.len() != method.parameter.len() {
            let fn_name_str = self.core.string_interner.resolve(function_name).unwrap_or("<NOT_FOUND>");
            return Err(TypeCheckError::generic_error(&format!(
                "Associated function '{}' argument count mismatch: expected {}, found {}",
                fn_name_str, method.parameter.len(), args.len()
            )));
        }
        
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();
        
        // Push generic parameters onto the scope for proper resolution
        let mut generic_scope = HashMap::new();
        for param in &generic_params {
            generic_scope.insert(*param, TypeDecl::Generic(*param));
        }
        self.type_inference.push_generic_scope(generic_scope);
        
        // Collect argument types and add constraints for type inference
        let mut arg_types = Vec::new();
        for (i, (arg_expr, (_, param_type))) in args.iter().zip(&method.parameter).enumerate() {
            let arg_type = self.visit_expr(arg_expr)?;
            arg_types.push(arg_type.clone());
            
            // Add constraint for parameter-argument type unification
            self.type_inference.add_constraint(
                param_type.clone(),
                arg_type,
                crate::type_checker::inference::ConstraintContext::FunctionCall {
                    function_name,
                    arg_index: i,
                }
            );
        }
        
        // Solve constraints to get type substitutions
        let substitutions = match self.type_inference.solve_constraints() {
            Ok(solution) => solution,
            Err(e) => {
                self.type_inference.pop_generic_scope();
                let fn_name_str = self.core.string_interner.resolve(function_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Type inference failed for associated function '{}': {}",
                    fn_name_str, e
                )));
            }
        };
        
        // Ensure all generic parameters have been inferred
        for generic_param in &generic_params {
            if !substitutions.contains_key(generic_param) {
                self.type_inference.pop_generic_scope();
                let param_name = self.core.string_interner.resolve(*generic_param).unwrap_or("<NOT_FOUND>");
                let fn_name_str = self.core.string_interner.resolve(function_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Cannot infer generic type parameter '{}' for associated function '{}'",
                    param_name, fn_name_str
                )));
            }
        }
        
        // Get the method's return type and apply substitutions
        let return_type = method.return_type.as_ref().unwrap_or(&TypeDecl::Unit);
        let substituted_return_type = self.type_inference.apply_solution(return_type, &substitutions);
        
        // Resolve Self type in the return type if present
        let resolved_return_type = match &substituted_return_type {
            TypeDecl::Self_ => {
                // For generic structs, create the concrete struct type with resolved type parameters
                // Preserve the order of generic parameters as defined in the struct
                let mut type_params = Vec::new();
                for generic_param in &generic_params {
                    if let Some(concrete_type) = substitutions.get(generic_param) {
                        type_params.push(concrete_type.clone());
                    } else {
                        // Fallback to Generic type if substitution is missing
                        type_params.push(TypeDecl::Generic(*generic_param));
                    }
                }
                TypeDecl::Struct(struct_name, type_params)
            }
            _ => substituted_return_type
        };
        
        // Record struct instance types for method calls (if needed)
        // This functionality can be implemented later for persistent type storage
        
        self.type_inference.pop_generic_scope();
        Ok(resolved_return_type)
    }

    fn generate_instantiated_name(&self, original_name: DefaultSymbol, substitutions: &HashMap<DefaultSymbol, TypeDecl>) -> String {
        let original_str = self.core.string_interner.resolve(original_name).unwrap_or("unknown");
        let mut name_parts = vec![original_str.to_string()];
        
        // Sort substitutions for consistent naming across different call sites
        let mut sorted_subs: Vec<_> = substitutions.iter().collect();
        sorted_subs.sort_by_key(|(k, _)| *k);
        
        // Append type suffixes to create unique instantiated name
        for (_, type_decl) in sorted_subs {
            let type_suffix = match type_decl {
                TypeDecl::Int64 => "i64",
                TypeDecl::UInt64 => "u64", 
                TypeDecl::Bool => "bool",
                TypeDecl::String => "str",
                _ => "unknown",
            };
            name_parts.push(type_suffix.to_string());
        }
        
        name_parts.join("_")
    }

    fn generate_instantiated_struct_name(&self, struct_name: DefaultSymbol, substitutions: &HashMap<DefaultSymbol, TypeDecl>) -> String {
        let base_name = self.core.string_interner.resolve(struct_name).unwrap_or("<unknown>");
        
        // Sort substitutions for consistent naming
        let mut sorted_subs: Vec<_> = substitutions.iter().collect();
        sorted_subs.sort_by_key(|&(k, _)| k);
        
        let mut name = base_name.to_string();
        for (param, typ) in sorted_subs {
            let param_name = self.core.string_interner.resolve(*param).unwrap_or("<unknown>");
            name.push_str(&format!("_{}_", param_name));
            name.push_str(&self.type_to_string(typ));
        }
        name
    }

    fn create_type_substitutions_for_method(&self, _generic_params: &[DefaultSymbol],
                                          _struct_name: DefaultSymbol) -> Result<HashMap<DefaultSymbol, TypeDecl>, TypeCheckError> {
        // Basic implementation: return empty substitution map
        // More advanced inference can be implemented here if needed
        Ok(HashMap::new())
    }
}

// Helper trait methods that need to be accessible
impl<'a> TypeCheckerVisitor<'a> {
    /// Helper method to convert TypeDecl to string representation
    fn type_to_string(&self, typ: &TypeDecl) -> String {
        match typ {
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::String => "str".to_string(),
            TypeDecl::Identifier(sym) => self.core.string_interner.resolve(*sym).unwrap_or("<unknown>").to_string(),
            TypeDecl::Generic(sym) => self.core.string_interner.resolve(*sym).unwrap_or("<unknown>").to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// Helper method to check type compatibility
    fn types_are_compatible(&self, type1: &TypeDecl, type2: &TypeDecl) -> bool {
        // Basic implementation - extend as needed
        type1 == type2 || 
        matches!((type1, type2), (TypeDecl::Unknown, _) | (_, TypeDecl::Unknown))
    }

}