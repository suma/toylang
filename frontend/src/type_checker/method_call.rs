//! Method-call type checking — instance methods (`x.foo()`) and associated
//! functions (`Struct::new()`).
//!
//! Pulled out of `expression.rs` to keep that file focused on plain
//! expression visitors. Methods here remain inherent on
//! `TypeCheckerVisitor`; `visitor_impl.rs` still routes the trait method
//! calls into them via thin wrappers.

use string_interner::DefaultSymbol;
use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};
use crate::type_checker::generics::GenericTypeChecking;
use crate::type_checker::method::MethodProcessing;

/// Walk a declared `TypeDecl` against the actual `arg_ty`, populating
/// `out` with `Generic(P) -> ConcreteType` mappings whenever a generic
/// param `P` (one of `params`) appears in `declared`. Recurses through
/// `Struct(_, args)` / `Enum(_, args)` / `Tuple(_)` so nested generic
/// positions resolve too. Skips conflicting bindings — the caller is
/// trusted to only feed compatible (declared, arg) pairs.
fn collect_substitution(
    declared: &TypeDecl,
    arg_ty: &TypeDecl,
    params: &[DefaultSymbol],
    out: &mut HashMap<DefaultSymbol, TypeDecl>,
) {
    match declared {
        TypeDecl::Generic(p) if params.contains(p) => {
            out.entry(*p).or_insert_with(|| arg_ty.clone());
        }
        TypeDecl::Identifier(p) if params.contains(p) => {
            // Method-only params can sometimes still appear as
            // Identifier (defensive — the parser flow normally lifts
            // them to Generic via the generic_context).
            out.entry(*p).or_insert_with(|| arg_ty.clone());
        }
        TypeDecl::Struct(_, decl_args) | TypeDecl::Enum(_, decl_args) => {
            let arg_args = match arg_ty {
                TypeDecl::Struct(_, a) | TypeDecl::Enum(_, a) => a.clone(),
                _ => return,
            };
            for (d, a) in decl_args.iter().zip(arg_args.iter()) {
                collect_substitution(d, a, params, out);
            }
        }
        TypeDecl::Tuple(decl_elems) => {
            if let TypeDecl::Tuple(arg_elems) = arg_ty {
                for (d, a) in decl_elems.iter().zip(arg_elems.iter()) {
                    collect_substitution(d, a, params, out);
                }
            }
        }
        _ => {}
    }
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Type check method calls - implementation used by type_checker.rs
    pub fn visit_method_call_impl(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let method_name = self.resolve_symbol_name(*method);
        
        
        let obj_type = self.visit_expr(obj)?;
        
        
        // Check if obj is a variable and get its name for type mapping lookup
        let _var_name = if let Some(obj_expr) = self.core.expr_pool.get(obj) {
            match obj_expr {
                Expr::Identifier(name) => Some(name.clone()),
                _ => None
            }
        } else {
            None
        };
        
        // The obj_type should already contain the concrete type parameters
        // No need to look up mappings, just use the type as-is
        let resolved_obj_type = obj_type.clone();
        
        // Type check arguments
        let mut arg_types = Vec::new();
        for arg in args {
            arg_types.push(self.visit_expr(arg)?);
        }
        
        // Check for builtin methods
        let method_str = self.resolve_symbol_name(*method);
        let builtin_method = self.builtin_methods.get(&(resolved_obj_type.clone(), method_str.to_string())).cloned();
        if let Some(builtin_method) = builtin_method {
            // visit_builtin_method_call expects ExprRef, not TypeDecl
            return self.visit_builtin_method_call(obj, &builtin_method, args);
        }
        
        // Check struct methods
        // Note: Current struct definition does not include methods
        // Method support will be added in a future refactoring
        
        // Check other type methods
        let result = self.visit_method_call_on_type(&resolved_obj_type, method, args, &arg_types);
        
        // Debug: If result is Generic, show what happened
        if let Ok(TypeDecl::Generic(sym)) = &result {
            let sym_str = self.resolve_symbol_name(*sym);
            return Err(TypeCheckError::generic_error(&format!(
                "DEBUG: Method '{}' returned unresolved Generic('{}') for object type {:?}",
                method_name, sym_str, resolved_obj_type
            )));
        }
        
        result
    }

    /// Helper method to handle method calls on a specific type
    pub fn visit_method_call_on_type(&mut self, obj_type: &TypeDecl, method: &DefaultSymbol, args: &Vec<ExprRef>, _arg_types: &[TypeDecl]) -> Result<TypeDecl, TypeCheckError> {
        let method_name = self.resolve_symbol_name(*method);

        // Method call on a trait-bounded generic parameter, e.g. inside
        // `fn f<T: MyTrait>(x: T) { x.foo() }`. Resolve `foo` through the
        // trait's method signature table; `Self` in the return type is
        // mapped back to the generic parameter so the caller sees the
        // appropriate concrete type after monomorphization.
        if let TypeDecl::Generic(t_sym) = obj_type {
            if let Some(TypeDecl::Identifier(trait_sym)) = self.context.current_fn_generic_bounds.get(t_sym).cloned() {
                if let Some(sig) = self.context.get_trait_method(trait_sym, *method).cloned() {
                    let ret = sig.return_type.clone().unwrap_or(TypeDecl::Unit);
                    let resolved = match ret {
                        TypeDecl::Self_ => TypeDecl::Generic(*t_sym),
                        other => other,
                    };
                    return Ok(resolved);
                }
            }
        }

        // Check if this is a user-defined method for a struct
        if let TypeDecl::Struct(struct_name, type_params) = obj_type {
            let struct_name_str = self.resolve_symbol_name(*struct_name);
            
            // Check if this is a generic struct with type parameters
            
            if !type_params.is_empty() {
                // Handle generic struct method call
                let method_func_opt = self.context.get_struct_method(*struct_name, *method);
                
                
                if let Some(method_func) = method_func_opt {
                    // Create substitution map from generic parameters to concrete types
                    let generic_params = self.context.get_struct_generic_params(*struct_name).cloned();
                    
                    
                    let generic_params = generic_params.unwrap_or_default();
                    let mut substitutions = HashMap::new();
                    for (i, generic_param) in generic_params.iter().enumerate() {
                        if let Some(concrete_type) = type_params.get(i) {
                            substitutions.insert(*generic_param, concrete_type.clone());
                        }
                    }
                    
                    
                    // Apply substitutions to method return type
                    let method_return_type = method_func.return_type.as_ref().unwrap_or(&TypeDecl::Unit);
                    
                    let resolved_return_type = match method_return_type {
                        TypeDecl::Self_ => TypeDecl::Struct(*struct_name, type_params.clone()),
                        TypeDecl::Generic(param) => {
                            let result = substitutions.get(param).cloned().unwrap_or_else(|| TypeDecl::Generic(*param));
                            result
                        },
                        other => other.substitute_generics(&substitutions)
                    };
                    
                    
                    return Ok(resolved_return_type);
                } else {
                    // Method not found, but let's see what we have
                    return Err(TypeCheckError::generic_error(&format!(
                        "Method '{}' not found for struct '{}' with type params {:?}",
                        method_name, struct_name_str, type_params
                    )));
                }
            } else {
                // Handle non-generic struct method call. Method-only
                // generic params (`fn pick<U>(...)`) need substitution
                // from the actual argument types — pull the method
                // function and infer.
                if let Some(method_func) =
                    self.context.get_struct_method(*struct_name, *method).cloned()
                {
                    let method_return_type = method_func
                        .return_type
                        .clone()
                        .unwrap_or(TypeDecl::Unit);
                    let mut substitutions: HashMap<DefaultSymbol, TypeDecl> =
                        HashMap::new();
                    if !method_func.generic_params.is_empty() {
                        // Visit each call argument and bind any
                        // matching `Generic(P)` slot in the method's
                        // declared params to the runtime arg type.
                        // Skip the first parameter (self).
                        for (i, arg_ref) in args.iter().enumerate() {
                            let param_idx = i + 1;
                            if let Some((_, declared_ty)) =
                                method_func.parameter.get(param_idx)
                            {
                                let arg_ty = self.visit_expr(arg_ref)?;
                                collect_substitution(
                                    declared_ty,
                                    &arg_ty,
                                    &method_func.generic_params,
                                    &mut substitutions,
                                );
                            }
                        }
                    }
                    let resolved = match method_return_type {
                        TypeDecl::Self_ => TypeDecl::Struct(*struct_name, vec![]),
                        TypeDecl::Identifier(name)
                            if self.context.struct_definitions.contains_key(&name) =>
                        {
                            TypeDecl::Struct(name, vec![])
                        }
                        TypeDecl::Generic(p) => {
                            substitutions.get(&p).cloned().unwrap_or(TypeDecl::Generic(p))
                        }
                        other => other.substitute_generics(&substitutions),
                    };
                    return Ok(resolved);
                }
            }
        }

        // Check array methods
        if let TypeDecl::Array(_, _) = obj_type {
            if method_name == "len" {
                // Array len() returns u64
                return Ok(TypeDecl::UInt64);
            }
        }

        // Check builtin methods
        if let Some(builtin_method) = self.builtin_methods.get(&(obj_type.clone(), method_name.to_string())).cloned() {
            // For builtin methods, we need to create a temporary expression ref for the object
            // This is a bit of a hack but necessary for the current API
            let dummy_obj_ref = ExprRef(0); // Use dummy ref for now
            return self.visit_builtin_method_call(&dummy_obj_ref, &builtin_method, args);
        }
        
        Err(TypeCheckError::method_error(&method_name, obj_type.clone(), "method not found"))
    }

    /// Type check associated function calls - implementation
    /// Dispatch a `module::func(args)` qualified call. The qualifier
    /// has already been confirmed to match an imported module alias;
    /// the function lives in the (flat) main function table because
    /// module integration appends imported `pub fn` items there.
    /// Type-check the args against the callee parameter list (mirrors
    /// the non-generic branch of `visit_call`) and return the
    /// callee's declared return type.
    fn dispatch_module_function_call(
        &mut self,
        function_name: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<TypeDecl, TypeCheckError> {
        let fun = self.context.get_fn(function_name).ok_or_else(|| {
            TypeCheckError::not_found("Function", &self.resolve_symbol_name(function_name))
        })?;
        // Honour visibility (matches the bare-call path).
        self.check_function_access(&fun)?;
        // Generic module functions: synthesize an `ExprList` for the
        // args and reuse the regular generic-call path so the
        // existing inference / monomorphisation logic runs.
        if !fun.generic_params.is_empty() {
            let args_ref = self
                .core
                .expr_pool
                .add(Expr::ExprList(args.clone()));
            return self.visit_generic_call(function_name, &args_ref, &fun);
        }
        let params: Vec<_> = fun
            .parameter
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect();
        if args.len() != params.len() {
            return Err(TypeCheckError::generic_error(&format!(
                "module function '{}' expects {} argument(s), found {}",
                self.resolve_symbol_name(function_name),
                params.len(),
                args.len()
            )));
        }
        for (arg_expr, expected_ty) in args.iter().zip(params.iter()) {
            let actual_ty = self.visit_expr(arg_expr)?;
            if !actual_ty.is_equivalent(expected_ty) && !matches!(actual_ty, TypeDecl::Unknown) {
                return Err(TypeCheckError::type_mismatch(
                    expected_ty.clone(),
                    actual_ty,
                ).with_context(&format!(
                    "argument of module function '{}'",
                    self.resolve_symbol_name(function_name)
                )));
            }
        }
        let return_ty = fun.return_type.clone().unwrap_or(TypeDecl::Unit);
        Ok(return_ty)
    }

    pub fn visit_associated_function_call_impl(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Handle Container::function_name(args) type calls for any associated function

        // Enum tuple-variant construction: `Enum::Variant(args)` syntactically
        // matches `Struct::assoc(args)`. Intercept when the left side is a
        // registered enum and the right side names one of its variants. For
        // generic enums, infer the type parameters from argument types.
        if let Some(variants) = self.context.enum_definitions.get(&struct_name).cloned() {
            if let Some(variant_def) = variants.iter().find(|v| v.name == function_name) {
                if args.len() != variant_def.payload_types.len() {
                    let enum_str = self.resolve_symbol_name(struct_name);
                    let v_str = self.resolve_symbol_name(function_name);
                    return Err(TypeCheckError::generic_error(&format!(
                        "variant '{}::{}' expects {} argument(s), found {}",
                        enum_str, v_str, variant_def.payload_types.len(), args.len()
                    )));
                }
                let generic_params = self.context.enum_generic_params.get(&struct_name).cloned().unwrap_or_default();
                let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                // Seed substitutions from the outer type hint so nested
                // variant construction (`Option::Some(Option::None)` with
                // hint `Option<Option<i64>>`) can flow the inner type args
                // down to the payload expression.
                let outer_hint = self.type_inference.type_hint.clone();
                let hint_args: Vec<TypeDecl> = match &outer_hint {
                    Some(TypeDecl::Enum(hint_name, a)) if *hint_name == struct_name => a.clone(),
                    Some(TypeDecl::Struct(hint_name, a)) if *hint_name == struct_name => a.clone(),
                    _ => Vec::new(),
                };
                if hint_args.len() == generic_params.len() {
                    for (param, arg) in generic_params.iter().zip(hint_args.iter()) {
                        substitutions.insert(*param, arg.clone());
                    }
                }
                let saved_hint = outer_hint;
                for (arg_expr, expected_ty) in args.iter().zip(variant_def.payload_types.iter()) {
                    // Push a hint equal to the payload type with current
                    // substitutions applied so inner literals / nested
                    // variants see the concrete expected type.
                    let resolved_hint = expected_ty.substitute_generics(&substitutions);
                    self.type_inference.type_hint = Some(resolved_hint);
                    let actual_ty = self.visit_expr(arg_expr)?;
                    // When the declared payload references a generic parameter,
                    // record the argument's concrete type as that parameter.
                    if let TypeDecl::Generic(p) = expected_ty {
                        if generic_params.contains(p) {
                            if let Some(prev) = substitutions.get(p) {
                                if !prev.is_equivalent(&actual_ty) {
                                    let enum_str = self.resolve_symbol_name(struct_name);
                                    let v_str = self.resolve_symbol_name(function_name);
                                    return Err(TypeCheckError::generic_error(&format!(
                                        "variant '{}::{}' generic parameter conflict: {:?} vs {:?}",
                                        enum_str, v_str, prev, actual_ty
                                    )));
                                }
                            } else {
                                substitutions.insert(*p, actual_ty.clone());
                            }
                            continue;
                        }
                    }
                    let expected_resolved = expected_ty.substitute_generics(&substitutions);
                    if !actual_ty.is_equivalent(&expected_resolved) && !matches!(actual_ty, TypeDecl::Unknown) {
                        let enum_str = self.resolve_symbol_name(struct_name);
                        let v_str = self.resolve_symbol_name(function_name);
                        return Err(TypeCheckError::generic_error(&format!(
                            "variant '{}::{}' payload type mismatch: expected {:?}, found {:?}",
                            enum_str, v_str, expected_resolved, actual_ty
                        )));
                    }
                }
                self.type_inference.type_hint = saved_hint;
                let type_args: Vec<TypeDecl> = generic_params.iter()
                    .map(|p| substitutions.get(p).cloned().unwrap_or(TypeDecl::Generic(*p)))
                    .collect();
                return Ok(TypeDecl::Enum(struct_name, type_args));
            }
        }

        // Module-qualified call: `module::func(args)` where `module`
        // is an imported module alias. The actual function definition
        // lives in the main program (module integration flattens it
        // into `program.function`), so the qualifier is essentially a
        // namespace check — once we confirm `struct_name` is a known
        // import alias, dispatch to the regular function-call path
        // using `function_name` directly. This keeps `math::add(...)`
        // working alongside the bare `add(...)` form for backward
        // compatibility while signalling intent at the call site.
        let module_alias = vec![struct_name];
        if self.imported_modules.contains_key(&module_alias) {
            if self.context.get_fn(function_name).is_some() {
                return self.dispatch_module_function_call(function_name, args);
            }
            // Module name was recognised but the function isn't in the
            // (flat) function table — surface a targeted diagnostic
            // rather than falling through to the struct-not-found
            // path which would mention "Struct".
            let module_str = self.resolve_symbol_name(struct_name);
            let func_str = self.resolve_symbol_name(function_name);
            return Err(TypeCheckError::generic_error(&format!(
                "module '{}' has no exported function '{}'",
                module_str, func_str
            )));
        }

        // Verify the struct exists — generic and non-generic both count.
        if !self.context.struct_definitions.contains_key(&struct_name) {
            return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
        }

        let function_name_str = self.resolve_symbol_name(function_name);

        let method = self.context.get_struct_method(struct_name, function_name)
            .cloned()
            .ok_or_else(|| TypeCheckError::generic_error(&format!(
                "Associated function '{}' not found for struct '{:?}'",
                function_name_str, struct_name
            )))?;

        if self.context.is_generic_struct(struct_name) {
            // Generic struct: delegate to the constraint-based inference path.
            return self.handle_generic_associated_function_call(struct_name, function_name, args, &method);
        }

        // Non-generic struct: type-check arguments directly against the method
        // parameter list (skipping any leading `self` since `Struct::fn(...)` is
        // called without an instance).
        let params: Vec<_> = if method.has_self_param {
            method.parameter.iter().skip(1).cloned().collect()
        } else {
            method.parameter.iter().cloned().collect()
        };

        if args.len() != params.len() {
            return Err(TypeCheckError::generic_error(&format!(
                "Associated function '{}::{}' expects {} arguments, found {}",
                self.resolve_symbol_name(struct_name),
                function_name_str,
                params.len(),
                args.len()
            )));
        }

        for (arg_expr, (_, expected_ty)) in args.iter().zip(params.iter()) {
            let actual_ty = self.visit_expr(arg_expr)?;
            if !actual_ty.is_equivalent(expected_ty) && !matches!(actual_ty, TypeDecl::Unknown) {
                return Err(TypeCheckError::type_mismatch(
                    expected_ty.clone(),
                    actual_ty,
                ).with_context(&format!(
                    "argument of associated function '{}::{}'",
                    self.resolve_symbol_name(struct_name),
                    function_name_str
                )));
            }
        }

        // Normalize the method's return type so downstream dispatch sees the
        // struct form. `Self` and bare `Identifier(struct_name)` both become
        // `Struct(struct_name, [])`.
        let return_ty = method.return_type.clone().unwrap_or(TypeDecl::Unit);
        let return_ty = match return_ty {
            TypeDecl::Self_ => TypeDecl::Struct(struct_name, vec![]),
            TypeDecl::Identifier(name)
                if self.context.struct_definitions.contains_key(&name) =>
            {
                TypeDecl::Struct(name, vec![])
            }
            other => other,
        };
        Ok(return_ty)
    }
}
