use string_interner::DefaultSymbol;
use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};
use crate::type_checker::method::MethodProcessing;

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

            // Type check method body
            let body_result = self.visit_stmt(&method.code);
            
            // Restore parameter context
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
}