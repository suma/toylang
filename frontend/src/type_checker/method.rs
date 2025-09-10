use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, BuiltinMethod};

/// Method processing and Self type handling for type checker
pub trait MethodProcessing {
    /// Resolve Self type to the actual struct type in impl block context
    fn resolve_self_type(&self, type_decl: &TypeDecl) -> TypeDecl;
    
    /// Check method arguments against parameter types, handling Self type specially
    fn check_method_arguments(&self, obj_type: &TypeDecl, method: &Rc<MethodFunction>, 
                             _args: &Vec<ExprRef>, arg_types: &Vec<TypeDecl>, method_name: &str) -> Result<(), TypeCheckError>;
    
    /// Process builtin method calls
    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    
    /// Process impl block method validation
    fn process_impl_method_validation(&mut self, target_type: DefaultSymbol, method: &Rc<MethodFunction>, has_generics: bool) -> Result<(), TypeCheckError>;
    
    /// Setup method parameter context for type checking
    fn setup_method_parameter_context(&mut self, method: &Rc<MethodFunction>);
    
    /// Restore method parameter context after type checking
    fn restore_method_parameter_context(&mut self);
    
    /// Validate method return type compatibility
    fn validate_method_return_type(&mut self, method: &Rc<MethodFunction>, body_result: Result<TypeDecl, TypeCheckError>, has_generics: bool) -> Result<(), TypeCheckError>;
}

/// Implementation of method processing for TypeCheckerVisitor
impl<'a> MethodProcessing for TypeCheckerVisitor<'a> {
    /// Resolve Self type to the actual struct type in impl block context
    fn resolve_self_type(&self, type_decl: &TypeDecl) -> TypeDecl {
        match type_decl {
            TypeDecl::Self_ => {
                if let Some(target_symbol) = self.context.current_impl_target {
                    TypeDecl::Struct(target_symbol, vec![])
                } else {
                    // Self used outside impl context - should be an error
                    type_decl.clone()
                }
            }
            _ => type_decl.clone(),
        }
    }

    /// Check method arguments against parameter types, handling Self type specially
    fn check_method_arguments(&self, obj_type: &TypeDecl, method: &Rc<MethodFunction>, 
                             _args: &Vec<ExprRef>, arg_types: &Vec<TypeDecl>, method_name: &str) -> Result<(), TypeCheckError> {
        // Check argument count
        if arg_types.len() + 1 != method.parameter.len() {
            return Err(TypeCheckError::method_error(
                method_name, 
                obj_type.clone(),
                &format!("expected {} arguments, found {}", method.parameter.len() - 1, arg_types.len())
            ));
        }

        // Check the first parameter (self parameter)
        if !method.parameter.is_empty() {
            let (_, first_param_type) = &method.parameter[0];
            
            // For Self type, we need to match it with the actual struct type
            let expected_self_type = match first_param_type {
                TypeDecl::Self_ => obj_type.clone(), // Self should match the object type
                _ => first_param_type.clone()
            };
            
            // Check if obj_type is compatible with the first parameter type
            if !self.are_types_compatible(&expected_self_type, obj_type) {
                return Err(TypeCheckError::method_error(
                    method_name,
                    obj_type.clone(),
                    &format!("self parameter type mismatch: expected {:?}, found {:?}", expected_self_type, obj_type)
                ));
            }
        }

        // Check remaining arguments (starting from index 1 since index 0 is self)
        for (i, arg_type) in arg_types.iter().enumerate() {
            if i + 1 < method.parameter.len() {
                let (_, param_type) = &method.parameter[i + 1];
                
                // For Self type in method parameters, resolve to object type
                let resolved_param_type = match param_type {
                    TypeDecl::Self_ => obj_type.clone(),
                    _ => param_type.clone()
                };
                
                if !self.are_types_compatible(&resolved_param_type, arg_type) {
                    return Err(TypeCheckError::method_error(
                        method_name,
                        obj_type.clone(),
                        &format!("argument {} type mismatch: expected {:?}, found {:?}", i + 1, resolved_param_type, arg_type)
                    ));
                }
            }
        }
        
        Ok(())
    }

    /// Process builtin method calls
    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Simplified implementation for now
        match method {
            BuiltinMethod::StrLen => Ok(TypeDecl::UInt64),
            BuiltinMethod::IsNull => Ok(TypeDecl::Bool),
            // Add more builtin methods as needed
            _ => {
                // For other builtin methods, check receiver type and args
                let _receiver_type = self.visit_expr(receiver)?;
                let _arg_types: Result<Vec<_>, _> = args.iter().map(|arg| self.visit_expr(arg)).collect();
                
                // Return appropriate type based on method
                Ok(TypeDecl::Unit)
            }
        }
    }

    /// Process impl block method validation
    fn process_impl_method_validation(&mut self, target_type: DefaultSymbol, method: &Rc<MethodFunction>, has_generics: bool) -> Result<(), TypeCheckError> {
        // Check method parameter types
        for (_, param_type) in &method.parameter {
            // Resolve Self type to the actual struct type
            let resolved_type = self.resolve_self_type(param_type);
            
            match &resolved_type {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) => {
                    // Valid parameter types (including struct types and generic types)
                },
                _ => {
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("parameter type in method '{}' for impl block '{:?}'", method_name, target_type),
                        resolved_type
                    ));
                }
            }
        }
        
        // Check return type if specified - now with proper generic support
        if let Some(ref ret_type) = method.return_type {
            // Try to resolve return type in generic context
            let resolved_ret_type = self.resolve_self_type(ret_type);
            
            // For generic types, we need to validate they can be resolved
            // but don't enforce strict type checking here since generics will be resolved later
            match &resolved_ret_type {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                TypeDecl::Unit | TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) => {
                    // Valid return types (including generic types and struct types)
                },
                _ => {
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("return type in method '{}' for impl block", method_name),
                        resolved_ret_type
                    ));
                }
            }
        }
        
        Ok(())
    }

    /// Setup method parameter context for type checking
    fn setup_method_parameter_context(&mut self, method: &Rc<MethodFunction>) {
        self.context.push_scope();
        for (param_name, param_type) in &method.parameter {
            let resolved_param_type = self.resolve_self_type(param_type);
            self.context.set_var(*param_name, resolved_param_type);
        }
    }

    /// Restore method parameter context after type checking
    fn restore_method_parameter_context(&mut self) {
        self.context.pop_scope();
    }

    /// Validate method return type compatibility
    fn validate_method_return_type(&mut self, method: &Rc<MethodFunction>, body_result: Result<TypeDecl, TypeCheckError>, has_generics: bool) -> Result<(), TypeCheckError> {
        if let Some(ref expected_return_type) = method.return_type {
            let resolved_expected_type = self.resolve_self_type(expected_return_type);
            match body_result {
                Ok(actual_return_type) => {
                    // For generic methods, use more sophisticated type checking
                    if has_generics {
                        // Try to apply generic substitutions for better matching
                        // Skip strict checking for generic return types - they'll be resolved during instantiation
                        match (&resolved_expected_type, &actual_return_type) {
                            (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => {
                                // Allow generic types to match - will be resolved later
                            }
                            _ => {
                                // Use normal type compatibility checking
                                if !self.are_types_compatible(&actual_return_type, &resolved_expected_type) {
                                    if has_generics {
                                        self.type_inference.pop_generic_scope();
                                    }
                                    let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                                    return Err(TypeCheckError::generic_error(&format!(
                                        "method '{}' return type mismatch: expected {:?}, found {:?}",
                                        method_name, resolved_expected_type, actual_return_type
                                    )));
                                }
                            }
                        }
                    } else {
                        // For non-generic methods, use strict type checking
                        if !self.are_types_compatible(&actual_return_type, &resolved_expected_type) {
                            let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                            return Err(TypeCheckError::generic_error(&format!(
                                "method '{}' return type mismatch: expected {:?}, found {:?}",
                                method_name, resolved_expected_type, actual_return_type
                            )));
                        }
                    }
                },
                Err(err) => {
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}