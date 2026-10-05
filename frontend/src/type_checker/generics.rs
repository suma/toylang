use crate::ast::{ExprRef, Function, MethodFunction};
use crate::type_checker::{TypeCheckError, TypeDecl, TypeCheckerVisitor};
use string_interner::DefaultSymbol;
use std::rc::Rc;
use rustc_hash::FxHashMap as HashMap;

impl TypeCheckerVisitor<'_> {
    pub(crate) fn visit_generic_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef, fun: &Function) -> Result<TypeDecl, TypeCheckError> {
        use crate::ast::Expr;
        
        // Extract argument expressions from the reference
        let args_data = if let Some(args_expr) = self.core.expr_pool.get(args_ref) {
            if let Expr::ExprList(args) = args_expr {
                Some(args.clone())
            } else {
                None
            }
        } else {
            self.pop_context();
            return Err(TypeCheckError::coded(crate::diagnostic::codes::INTERNAL, "Invalid arguments reference"));
        };
        
        let args = args_data.ok_or_else(|| {
            self.pop_context();
            TypeCheckError::coded(crate::diagnostic::codes::INTERNAL, "Invalid arguments expression")
        })?;
        
        // Verify argument count matches parameter count
        if args.len() != fun.parameter.len() {
            self.pop_context();
            let fn_name_str = self.resolve_symbol_name(fn_name);
            return Err(TypeCheckError::coded(crate::diagnostic::codes::ARITY, format!(
                "Generic function '{}' argument count mismatch: expected {}, found {}",
                fn_name_str, fun.parameter.len(), args.len()
            )));
        }
        
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();
        
        // Collect argument types and add constraints
        let mut arg_types = Vec::new();
        for (i, (arg_expr, (_, param_type))) in args.iter().zip(&fun.parameter).enumerate() {
            // REF-REBORROW: rewrite before the argument is visited, so
            // the constraint sees `&mut Vec<u64>` rather than the
            // auto-dereferenced `Vec<u64>` -- otherwise solving hits
            // "cannot unify `&mut Vec<T>` with `Vec<u64>`" and the
            // borrow has to be written by hand after all.
            self.try_reborrow_mut_arg_for_inference(arg_expr, param_type);
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
        
        // STDLIB-TRAIT-BASE B5: where the call's value lands is
        // evidence too -- but **only** for a parameter the arguments
        // could not name.
        //
        // Constraints came from arguments alone, so a function whose
        // type parameter appears only in its return type
        // (`fn make<T: Default>() -> T`) had nothing to infer from and
        // was rejected as "cannot infer T", with the answer written
        // down one token away in `val p: P = make()`.
        //
        // Adding the constraint unconditionally is wrong: the type
        // hint at a call site is not always the expected return type
        // (it also carries numeric-literal context down into the
        // arguments), so unifying against it broke inference that had
        // been working. Solve first, and only reach for the hint when
        // something is genuinely missing.
        let solved = self.type_inference.solve_constraints(self.core.string_interner);
        let needs_return_evidence = match &solved {
            Ok(solution) => fun
                .generic_params
                .iter()
                .any(|p| !solution.contains_key(p)),
            Err(_) => false,
        };
        if needs_return_evidence
            && let (Some(ret_ty), Some(hint)) = (
                fun.return_type.as_ref(),
                self.type_inference.type_hint.clone(),
            )
            && !matches!(hint, TypeDecl::Unknown | TypeDecl::Number)
        {
            self.type_inference.add_constraint(
                ret_ty.clone(),
                hint,
                crate::type_checker::inference::ConstraintContext::Generic,
            );
        }

        // Solve constraints to get type substitutions
        let substitutions = match self.type_inference.solve_constraints(self.core.string_interner) {
            Ok(solution) => solution,
            Err(e) => {
                self.pop_context();
                let fn_name_str = self.resolve_symbol_name(fn_name);
                return Err(TypeCheckError::coded(crate::diagnostic::codes::GENERIC_INFERENCE, format!(
                    "Type inference failed for generic function '{}': {}",
                    fn_name_str, e
                )));
            }
        };
        
        // Ensure all generic parameters have been inferred
        for generic_param in &fun.generic_params {
            if !substitutions.contains_key(generic_param) {
                self.pop_context();
                let param_name = self.resolve_symbol_name(*generic_param);
                let fn_name_str = self.resolve_symbol_name(fn_name);
                return Err(TypeCheckError::coded(crate::diagnostic::codes::GENERIC_INFERENCE, format!(
                    "Cannot infer generic type parameter '{}' for function '{}'",
                    param_name, fn_name_str
                )));
            }
        }

        // Enforce any declared bounds on generic parameters, e.g. `<A: Allocator>`
        // or `<T: MyTrait>` where `MyTrait` is a user-defined trait.
        // The bound chain is transparent: if the caller supplies a bounded
        // generic of its own (`fn g<B: Allocator>(b: B) { f(b) }`), treat
        // that as satisfying the same bound.
        //
        // TRAIT-BOUND: a generic-trait bound (`I: Iter<i64>`) parses as
        // `Struct(iter_sym, [i64])`; it is split into a trait symbol plus
        // type args and checked through `satisfies_trait_bound` against
        // the impl's recorded type args. Shared with the method-call
        // path (STDLIB-ORD) through `check_generic_bounds`.
        let fn_name_str = self.resolve_symbol_name(fn_name);
        if let Err(e) = self.check_generic_bounds(
            &fun.generic_params,
            &fun.generic_bounds,
            &substitutions,
            "Function",
            &fn_name_str,
        ) {
            self.pop_context();
            return Err(e);
        }

        // COLLECTIONS C0(a): the declared bounds are not the only thing
        // a call site owes the callee. A body that compares two values
        // of a type parameter needs that parameter's type argument to
        // have an answer for `==`; which arguments those are is not
        // known until every body has been checked, so record the
        // instantiation and let `eq_requirement.rs` join the two.
        self.note_generic_instantiation(crate::type_checker::context::EqInstantiation {
            owner: crate::type_checker::context::EqOwner::Function(fn_name),
            substitutions: substitutions.iter().map(|(k, v)| (*k, v.clone())).collect(),
            owner_kind: "Function",
            owner_name: fn_name_str.clone(),
            // The first argument is the closest thing to the call this
            // path holds a reference to — the method paths point at the
            // same place, so the two read alike.
            location: args.first().and_then(|a| self.get_expr_location(a)),
        });
        
        // Substitute generic types in return type with concrete types using the new inference engine
        let return_type = if let Some(ret_type) = &fun.return_type {
            self.type_inference.apply_solution(ret_type, &substitutions)
        } else {
            TypeDecl::Unknown
        };
        
        self.pop_context();
        Ok(return_type)
    }

    pub(crate) fn handle_generic_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, 
                                             args: &Vec<ExprRef>, method: &Rc<MethodFunction>) -> Result<TypeDecl, TypeCheckError> {
        // Get the generic parameters for this struct
        let generic_params = self.context.get_struct_generic_params(struct_name)
            .cloned()
            .unwrap_or_default();
        
        // Verify argument count matches parameter count
        if args.len() != method.parameter.len() {
            let fn_name_str = self.resolve_symbol_name(function_name);
            return Err(TypeCheckError::coded(crate::diagnostic::codes::ARITY, format!(
                "Associated function '{}' argument count mismatch: expected {}, found {}",
                fn_name_str, method.parameter.len(), args.len()
            )));
        }
        
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();
        
        // Push generic parameters onto the scope for proper resolution
        let mut generic_scope = HashMap::default();
        for param in &generic_params {
            generic_scope.insert(*param, TypeDecl::Generic(*param));
        }
        self.type_inference.push_generic_scope(generic_scope);
        
        // Collect argument types and add constraints for type inference
        let fn_name_str = self.resolve_symbol_name(function_name);

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
        let substitutions = match self.type_inference.solve_constraints(self.core.string_interner) {
            Ok(solution) => {
                solution
            }
            Err(e) => {
                self.type_inference.pop_generic_scope();
                return Err(TypeCheckError::coded(crate::diagnostic::codes::GENERIC_INFERENCE, format!(
                    "Type inference failed for associated function '{}': {}",
                    fn_name_str, e
                )));
            }
        };

        // The arguments are not the only evidence. A constructor can
        // put `T` in the return type alone — `Ptr::try_from_raw(p:
        // ptr) -> Option<Self>` takes nothing that mentions it — and
        // then the expected type is what says which instance is
        // wanted. Read it off the hint at whatever depth it appears
        // (GENERIC-IN-ENUM-PAYLOAD), and only for parameters the
        // arguments left unbound, so an argument always wins.
        let mut substitutions = substitutions;
        if generic_params.iter().any(|p| !substitutions.contains_key(p))
            && let Some(hint) = self.type_inference.type_hint.clone()
        {
            let hint_args = match &hint {
                TypeDecl::Struct(name, a) | TypeDecl::Enum(name, a) if *name == struct_name => {
                    a.clone()
                }
                other => other.nested_type_args(struct_name).unwrap_or_default(),
            };
            for (param, ty) in generic_params.iter().zip(hint_args) {
                if !matches!(ty, TypeDecl::Unknown) {
                    substitutions.entry(*param).or_insert(ty);
                }
            }
        }

        // Ensure all generic parameters have been inferred or are available in outer scope
        // If we're calling from within a generic method, the substitution might resolve to Generic(T)
        // which is valid - it means the type is still generic but will be resolved when the method is called
        for generic_param in &generic_params {
            if !substitutions.contains_key(generic_param) {
                // Check if it's available in outer scope (e.g., we're inside a generic method)
                if self.type_inference.lookup_generic_type(*generic_param).is_none() {
                    self.type_inference.pop_generic_scope();
                    let param_name = self.resolve_symbol_name(*generic_param);
                    let fn_name_str = self.resolve_symbol_name(function_name);
                    return Err(TypeCheckError::coded(crate::diagnostic::codes::GENERIC_INFERENCE, format!(
                        "Cannot infer generic type parameter '{}' for associated function '{}'",
                        param_name, fn_name_str
                    )));
                }
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
            TypeDecl::Struct(name, type_params) => {
                // Recursively substitute generic parameters in Struct type arguments
                let substituted_params: Vec<TypeDecl> = type_params.iter()
                    .map(|param| self.substitute_type_params(param, &substitutions))
                    .collect();
                TypeDecl::Struct(*name, substituted_params)
            }
            _ => substituted_return_type
        };
        // SELF-IN-TYPE-ARG: the arms above only see `Self` at the top
        // level, so `-> Option<Self>` reached the caller unresolved and
        // failed against a perfectly explicit `val o: Option<Win<u64>>`.
        // The `Self_` arm has already produced a concrete type, so this
        // is a no-op there.
        let self_ty = {
            let mut type_params = Vec::new();
            for generic_param in &generic_params {
                if let Some(concrete_type) = substitutions.get(generic_param) {
                    type_params.push(concrete_type.clone());
                } else {
                    type_params.push(TypeDecl::Generic(*generic_param));
                }
            }
            TypeDecl::Struct(struct_name, type_params)
        };
        let resolved_return_type = resolved_return_type.substitute_self(&self_ty);

        // Record struct instance types for method calls (if needed)
        // This functionality can be implemented later for persistent type storage

        self.type_inference.pop_generic_scope();
        Ok(resolved_return_type)
    }

}
