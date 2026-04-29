use string_interner::DefaultSymbol;
use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};
use crate::type_checker::method::MethodProcessing;
use crate::type_checker::traits::Acceptable;

/// Implementation block type checking
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check implementation blocks
    pub fn visit_impl_block_impl(&mut self, target_type: DefaultSymbol, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        // target_type is already a symbol
        let struct_symbol = target_type;

        // Set current impl target for Self resolution
        let old_impl_target = self.context.current_impl_target;
        self.context.current_impl_target = Some(struct_symbol);

        // Check if this is a generic struct and set up generic scope
        let generic_params = self.context.get_struct_generic_params(struct_symbol).cloned();
        let has_generics = generic_params.is_some() && !generic_params.as_ref().unwrap().is_empty();

        // Store generic parameters in context for Self type resolution
        let old_impl_generic_params = self.context.current_impl_generic_params.clone();
        if has_generics {
            self.context.current_impl_generic_params = generic_params.clone();
            // Push generic parameters into scope for method type checking
            let generic_substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> =
                generic_params.as_ref().unwrap().iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            self.type_inference.push_generic_scope(generic_substitutions);
        }

        // Impl block type checking - validate methods
        for method in methods {
            // Use method.rs module for validation
            if let Err(err) = self.process_impl_method_validation(struct_symbol, method, has_generics) {
                return Err(err);
            }

            // Type check method body using method.rs module
            self.setup_method_parameter_context(method);

            // Install the method's declared bounds (inherited from its impl block)
            // so the body can see `<A: Allocator>` style constraints, mirroring the
            // Function path in type_check. Also merge in the struct's own
            // generic bounds so that `struct List<T, A: Allocator>` makes A's
            // bound visible inside methods that don't restate it.
            let mut merged_bounds = method.generic_bounds.clone();
            if let Some(struct_bounds) = self.context.get_struct_generic_bounds(struct_symbol).cloned() {
                for (param, bound) in struct_bounds {
                    merged_bounds.entry(param).or_insert(bound);
                }
            }
            let prev_bounds = std::mem::replace(
                &mut self.context.current_fn_generic_bounds,
                merged_bounds,
            );

            // Seed the body's type hint with the method's declared return
            // type so struct literals at the tail position can pick up type
            // parameters that aren't constrained by any field (e.g. the `T`
            // in `List<T, A>` when no field directly references T).
            let prev_hint = self.type_inference.type_hint.clone();
            if let Some(ret_ty) = &method.return_type {
                let resolved = self.resolve_self_type(ret_ty);
                self.type_inference.type_hint = Some(resolved);
            }

            // `requires` clauses see only the method's parameters (incl. `self`).
            // Type-check before the body so failures point at the contract.
            for cond in &method.requires {
                if let Err(e) = self.check_method_contract_clause(cond, "requires") {
                    self.context.current_fn_generic_bounds = prev_bounds;
                    self.restore_method_parameter_context();
                    if has_generics {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(e);
                }
            }

            // Type check method body
            let body_result = self.visit_stmt(&method.code);

            // `ensures` runs after the body. Bind `result` to the method's
            // return type before checking each clause.
            if !method.ensures.is_empty() {
                let result_ty = match &method.return_type {
                    Some(ret) => self.resolve_self_type(ret),
                    None => match &body_result {
                        Ok(t) => t.clone(),
                        Err(_) => TypeDecl::Unit,
                    },
                };
                if let Some(result_sym) = self.core.string_interner.get("result") {
                    self.context.set_var(result_sym, result_ty);
                }
                for cond in &method.ensures {
                    if let Err(e) = self.check_method_contract_clause(cond, "ensures") {
                        self.context.current_fn_generic_bounds = prev_bounds;
                        self.restore_method_parameter_context();
                        if has_generics {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(e);
                    }
                }
            }

            // Restore type hint
            self.type_inference.type_hint = prev_hint;

            // Restore generic bounds and parameter context
            self.context.current_fn_generic_bounds = prev_bounds;
            self.restore_method_parameter_context();

            // Validate method return type compatibility using method.rs module
            if let Err(err) = self.validate_method_return_type(method, body_result, has_generics) {
                return Err(err);
            }

            // Register method in context
            self.context.register_struct_method(struct_symbol, method.name, method.clone());
        }
        
        // Pop generic scope if it was pushed
        if has_generics {
            self.type_inference.pop_generic_scope();
        }

        // Restore previous impl target context
        self.context.current_impl_target = old_impl_target;
        self.context.current_impl_generic_params = old_impl_generic_params;
        
        // Impl block declaration returns Unit
        Ok(TypeDecl::Unit)
    }

    /// Type-check a single contract predicate inside an impl method. Same
    /// shape as the free-function helper in visitor.rs but lives here so it
    /// can be a method on `TypeCheckerVisitor` without crossing modules.
    fn check_method_contract_clause(
        &mut self,
        cond: &ExprRef,
        kind: &str,
    ) -> Result<(), TypeCheckError> {
        let expr = self.core.expr_pool.get(cond)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid contract expression reference"))?;
        let ty = expr.clone().accept(self)?;
        if ty != TypeDecl::Bool {
            return Err(TypeCheckError::generic_error(
                &format!("`{kind}` clause must be of type bool, got {ty:?}")
            ));
        }
        Ok(())
    }
}