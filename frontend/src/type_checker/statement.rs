use string_interner::DefaultSymbol;
use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    Acceptable, TypeInferenceManager
};

/// Statement type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Main entry point for statement type checking
    pub fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        let mut stmt_val = self.core.stmt_pool.get(&stmt).unwrap_or(Stmt::Break).clone();
        
        let result = stmt_val.accept(self);
        
        // If an error occurred, try to add location information if not already present
        match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_stmt_location(stmt);
                Err(error)
            }
            other => other,
        }
    }

    /// Type check expression statements
    pub fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_obj = self.core.expr_pool.get(&expr)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in statement"))?;
        expr_obj.clone().accept(self)
    }

    /// Type check variable declarations (var)
    pub fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let type_decl = type_decl.clone();
        let expr = expr.clone();
        self.process_val_type(name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    /// Type check value declarations (val)
    pub fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_ref = expr.clone();
        let type_decl = type_decl.clone();
        
        // Set type hint and evaluate expression
        let old_hint = self.setup_type_hint_for_val(&type_decl);
        let expr_ty = self.visit_expr(&expr_ref)?;
        
        // Manage variable-expression mapping
        self.update_variable_expr_mapping_internal(name, &expr_ref, &expr_ty);
        
        // Apply type transformations
        self.apply_type_transformations_for_expr(&type_decl, &expr_ty, &expr_ref)?;
        
        // Determine final type and store variable
        let final_type = self.determine_final_type_for_expr(&type_decl, &expr_ty);
        
        // Debug: Print variable type information
        let var_name_str = self.core.string_interner.resolve(name).unwrap_or("<unknown>");
        
        // Extract type parameter mappings for generic struct instances
        if let TypeDecl::Struct(struct_name, type_params) = &final_type {
            if !type_params.is_empty() {
                // Get the generic parameter names for this struct
                if let Some(generic_param_names) = self.context.get_struct_generic_params(*struct_name) {
                    let mut type_mappings = HashMap::new();
                    
                    // Create mappings from parameter names to concrete types
                    for (param_name, concrete_type) in generic_param_names.iter().zip(type_params.iter()) {
                        type_mappings.insert(*param_name, concrete_type.clone());
                    }
                    
                    // Store the type parameter mappings for this variable
                    self.context.set_var_type_mapping(name, type_mappings);
                    
                    eprintln!("DEBUG: Storing type mappings for variable '{}': {:?}", var_name_str, type_params);
                }
            }
        }
        
        self.context.set_var(name, final_type.clone());
        
        // Debug: Log what type was actually stored
        if var_name_str == "point" {
            eprintln!("DEBUG visit_val: Stored variable 'point' with type: {:?}", final_type);
        }
        
        // Restore previous type hint
        self.type_inference.type_hint = old_hint;
        
        Ok(TypeDecl::Unit)
    }

    /// Type check return statements
    pub fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if expr.is_none() {
            Ok(TypeDecl::Unit)
        } else {
            let e = expr.as_ref()
                .ok_or_else(|| TypeCheckError::generic_error("Expected expression in return"))?;
            let expr_obj = self.core.expr_pool.get(&e)
                .ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
            let return_type = expr_obj.clone().accept(self)?;
            Ok(return_type)
        }
    }

    /// Type check for loops
    pub fn visit_for(&mut self, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        
        let range_obj = self.core.expr_pool.get(&range)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid range expression reference"))?;
        let range_ty = range_obj.clone().accept(self)?;
        let ty = Some(range_ty);
        
        self.process_val_type(init, &ty, &Some(*range))?;
        
        let body_obj = self.core.expr_pool.get(&body)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference"))?;
        let res = body_obj.clone().accept(self);
        
        self.pop_context();
        res
    }

    /// Type check while loops
    pub fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Evaluate condition type first
        let cond_obj = self.core.expr_pool.get(&cond)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid condition expression reference in while"))?;
        let cond_type = cond_obj.clone().accept(self)?;
        
        // Verify condition is boolean
        if cond_type != TypeDecl::Bool {
            return Err(TypeCheckError::type_mismatch(TypeDecl::Bool, cond_type));
        }
        
        // Create new scope for while body
        self.push_context();
        let body_obj = self.core.expr_pool.get(&body)
            .ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference in while"))?;
        let res = body_obj.clone().accept(self);
        self.pop_context();
        res
    }

    /// Type check break statements
    pub fn visit_break(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    /// Type check continue statements
    pub fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }
}