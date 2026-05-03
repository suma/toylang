use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, BuiltinMethod};

/// Method processing and Self type handling for type checker
pub trait MethodProcessing {
    /// Resolve Self type to the actual struct type in impl block context
    fn resolve_self_type(&self, type_decl: &TypeDecl) -> TypeDecl;
    
    /// Check method arguments against parameter types, handling Self type specially.
    /// Currently unused — kept for future callers once method-call type checking
    /// goes through the trait directly.
    #[allow(dead_code)]
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
                    // Extension-trait support (Step A): when the impl
                    // block targets a primitive type (`impl Foo for i64
                    // { ... }`), `Self` resolves directly to the
                    // matching `TypeDecl::Int64` etc. so method bodies
                    // don't see the `Struct(sym_for_i64, _)` they
                    // would otherwise — the type-checker would then
                    // reject every `self`-typed expression as a
                    // struct/primitive mismatch.
                    if let Some(prim) = self.primitive_type_decl_from_symbol(target_symbol) {
                        return prim;
                    }
                    // Include generic parameters if available
                    let type_params = if let Some(ref generic_params) = self.context.current_impl_generic_params {
                        generic_params.iter().map(|param| TypeDecl::Generic(*param)).collect()
                    } else {
                        vec![]
                    };
                    TypeDecl::Struct(target_symbol, type_params)
                } else {
                    // Self used outside impl context - should be an error
                    type_decl.clone()
                }
            }
            TypeDecl::Identifier(name) => {
                // Convert Identifier to Struct with generic parameters if it's a generic struct
                if let Some(generic_params) = self.context.get_struct_generic_params(*name) {
                    let type_params = generic_params.iter().map(|param| {
                        // Try to resolve from current generic scope, otherwise use Generic type
                        self.type_inference.lookup_generic_type(*param)
                            .unwrap_or_else(|| TypeDecl::Generic(*param))
                    }).collect();
                    TypeDecl::Struct(*name, type_params)
                } else {
                    // Not a generic struct, keep as Identifier
                    type_decl.clone()
                }
            }
            // Recursively resolve nested Struct types
            TypeDecl::Struct(name, type_params) => {
                let resolved_params: Vec<TypeDecl> = type_params.iter()
                    .map(|param| self.resolve_self_type(param))
                    .collect();
                TypeDecl::Struct(*name, resolved_params)
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
            // NOTE: numeric value-method arms (`I64Abs` / `F64Abs` /
            // `F64Sqrt`) lived here before Step F. The prelude's
            // extension-trait impls now cover the same surface, so
            // call sites resolve through `visit_method_call_on_type`'s
            // primitive-receiver path against the user-method
            // registry instead.
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
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Float64 | TypeDecl::Bool |
                TypeDecl::String | TypeDecl::Ptr |
                // NUM-W: narrow ints valid as method param types so
                // `impl Hash for u8 { fn hash(self: Self) -> u64 }`
                // (and any user-defined inherent impl on a narrow
                // primitive) survives validation.
                TypeDecl::Int8 | TypeDecl::Int16 | TypeDecl::Int32 |
                TypeDecl::UInt8 | TypeDecl::UInt16 | TypeDecl::UInt32 |
                TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) |
                TypeDecl::Array(_, _) | TypeDecl::Dict(_, _) | TypeDecl::Tuple(_) |
                // REF-Stage-2: `&T` parameter type is accepted as
                // long as the inner type is one of the supported
                // shapes; the impl-block validator only needs to
                // know the wrapper exists (lowering peels it).
                TypeDecl::Ref(_) => {
                    // Valid parameter types — primitives, structs,
                    // generics, and collections. `Float64` / `Ptr`
                    // were added when extension traits over
                    // primitives landed (Step A).
                },
                _ => {
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    let method_name = self.resolve_symbol_name(method.name);
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
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Float64 | TypeDecl::Bool |
                TypeDecl::String | TypeDecl::Ptr |
                // NUM-W: narrow ints valid as method return types.
                TypeDecl::Int8 | TypeDecl::Int16 | TypeDecl::Int32 |
                TypeDecl::UInt8 | TypeDecl::UInt16 | TypeDecl::UInt32 |
                TypeDecl::Unit | TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) |
                TypeDecl::Array(_, _) | TypeDecl::Dict(_, _) | TypeDecl::Tuple(_) => {
                    // Valid return types — primitives, structs,
                    // generics, and collections. `Float64` / `Ptr`
                    // were added when extension traits over
                    // primitives landed (Step A).
                },
                _ => {
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    let method_name = self.resolve_symbol_name(method.name);
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
        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter` (the parser
        // only sets `has_self_param=true` and tracks mutability via
        // `self_is_mut`). Bind the `self` identifier to the
        // resolved Self type explicitly so the method body can
        // reference `self.field` without a per-form parser hack.
        // The `self: Self` form continues to bind `self` through
        // the regular parameter loop below.
        if method.has_self_param {
            let self_type = self.resolve_self_type(&TypeDecl::Self_);
            // "self" is virtually always pre-interned because any
            // method body that has `&self` / `&mut self` reaches
            // here only after the parser has tokenised the
            // identifier `self` at least once. `get` is sufficient
            // for the same reason `instantiate_generic_method_with_self_type`
            // uses it for the synthetic `Self` subst entry.
            if let Some(self_sym) = self.core.string_interner.get("self") {
                self.context.set_var(self_sym, self_type);
            }
        }
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
                                    let method_name = self.resolve_symbol_name(method.name);
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
                            let method_name = self.resolve_symbol_name(method.name);
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