use std::collections::{HashMap, HashSet, VecDeque};
use string_interner::DefaultSymbol;
use crate::ast::ExprRef;
use crate::type_decl::TypeDecl;

/// Information about a generic function or struct instantiation
#[derive(Debug, Clone, PartialEq)]
pub struct GenericInstantiation {
    /// The original generic function or struct name
    pub original_name: DefaultSymbol,
    /// Generated name for the instantiated function/struct (e.g., "identity_i64", "List_i64")
    pub instantiated_name: DefaultSymbol,
    /// Type substitutions for this instantiation
    pub type_substitutions: HashMap<DefaultSymbol, TypeDecl>,
    /// Type of instantiation (function or struct)
    pub kind: InstantiationKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstantiationKind {
    Function,
    Struct,
}

/// Type constraint for unification-based type inference
#[derive(Debug, Clone, PartialEq)]
pub struct TypeConstraint {
    pub left: TypeDecl,
    pub right: TypeDecl,
    pub context: ConstraintContext,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintContext {
    /// From a function call argument
    FunctionCall {
        function_name: DefaultSymbol,
        arg_index: usize,
    },
    /// From a variable assignment
    Assignment {
        variable_name: DefaultSymbol,
    },
    /// From a return statement
    Return {
        function_name: DefaultSymbol,
    },
    /// From a field access
    FieldAccess {
        struct_name: DefaultSymbol,
        field_name: DefaultSymbol,
    },
    /// Generic context
    Generic,
}

#[derive(Debug)]
pub struct TypeInferenceState {
    pub type_hint: Option<TypeDecl>,
    pub number_usage_context: Vec<(ExprRef, TypeDecl)>,
    pub variable_expr_mapping: HashMap<DefaultSymbol, ExprRef>,
    pub recursion_depth: u32,
    pub max_recursion_depth: u32,
    /// Comprehensive mapping of all expression references to their types
    pub expr_types: HashMap<ExprRef, TypeDecl>,
    /// Generic type parameter to concrete type mappings stack for nested scopes
    pub generic_substitutions_stack: Vec<HashMap<DefaultSymbol, TypeDecl>>,
    /// Collected generic instantiations that need to be processed in the instantiation pass
    pub pending_instantiations: Vec<GenericInstantiation>,
    /// Set to track unique instantiations and avoid duplicates
    pub instantiation_signatures: HashSet<String>,
    /// Type constraints to be solved during inference
    pub constraints: Vec<TypeConstraint>,
    /// Partial type solutions being built up
    pub partial_solutions: HashMap<DefaultSymbol, TypeDecl>,
}

impl TypeInferenceState {
    pub fn new() -> Self {
        Self {
            type_hint: None,
            number_usage_context: Vec::new(),
            variable_expr_mapping: HashMap::new(),
            recursion_depth: 0,
            max_recursion_depth: 50, // Further increased for complex nested structs
            expr_types: HashMap::new(),
            generic_substitutions_stack: Vec::new(),
            pending_instantiations: Vec::new(),
            instantiation_signatures: HashSet::new(),
            constraints: Vec::new(),
            partial_solutions: HashMap::new(),
        }
    }

    pub fn set_type_hint(&mut self, hint: Option<TypeDecl>) {
        self.type_hint = hint;
    }

    pub fn get_type_hint(&self) -> Option<TypeDecl> {
        self.type_hint.clone()
    }

    pub fn increment_recursion_depth(&mut self) -> Result<(), crate::type_checker::TypeCheckError> {
        if self.recursion_depth >= self.max_recursion_depth {
            return Err(crate::type_checker::TypeCheckError::generic_error(
                "Maximum recursion depth reached in type inference"
            ));
        }
        self.recursion_depth += 1;
        Ok(())
    }

    pub fn decrement_recursion_depth(&mut self) {
        if self.recursion_depth > 0 {
            self.recursion_depth -= 1;
        }
    }

    pub fn add_number_context(&mut self, expr_ref: ExprRef, type_decl: TypeDecl) {
        self.number_usage_context.push((expr_ref, type_decl.clone()));
        // Also add to comprehensive expr_types mapping
        self.expr_types.insert(expr_ref, type_decl);
    }
    
    /// Record the type of any expression
    pub fn set_expr_type(&mut self, expr_ref: ExprRef, type_decl: TypeDecl) {
        self.expr_types.insert(expr_ref, type_decl);
    }

    pub fn map_variable(&mut self, name: DefaultSymbol, expr_ref: ExprRef) {
        self.variable_expr_mapping.insert(name, expr_ref);
    }

    pub fn get_variable_expr(&self, name: DefaultSymbol) -> Option<ExprRef> {
        self.variable_expr_mapping.get(&name).copied()
    }

    pub fn clear(&mut self) {
        self.type_hint = None;
        self.number_usage_context.clear();
        self.variable_expr_mapping.clear();
        self.generic_substitutions_stack.clear();
        self.pending_instantiations.clear();
        self.instantiation_signatures.clear();
        self.constraints.clear();
        self.partial_solutions.clear();
    }
    
    /// Push a new generic scope with the given type parameter mappings
    pub fn push_generic_scope(&mut self, mappings: HashMap<DefaultSymbol, TypeDecl>) {
        self.generic_substitutions_stack.push(mappings);
    }
    
    /// Pop the current generic scope
    pub fn pop_generic_scope(&mut self) -> Option<HashMap<DefaultSymbol, TypeDecl>> {
        self.generic_substitutions_stack.pop()
    }
    
    /// Look up a generic type parameter in the current scope stack (innermost first)
    pub fn lookup_generic_type(&self, param: DefaultSymbol) -> Option<TypeDecl> {
        for scope in self.generic_substitutions_stack.iter().rev() {
            if let Some(type_decl) = scope.get(&param) {
                return Some(type_decl.clone());
            }
        }
        None
    }
    
    /// Add or update a generic type mapping in the current (top) scope
    pub fn set_generic_type(&mut self, param: DefaultSymbol, type_decl: TypeDecl) {
        if let Some(current_scope) = self.generic_substitutions_stack.last_mut() {
            current_scope.insert(param, type_decl);
        } else {
            // If no scope exists, create one
            let mut new_scope = HashMap::new();
            new_scope.insert(param, type_decl);
            self.generic_substitutions_stack.push(new_scope);
        }
    }
    
    /// Record a generic instantiation for later processing
    pub fn record_instantiation(&mut self, instantiation: GenericInstantiation) {
        // Create a unique signature to avoid duplicate instantiations
        let signature = self.create_instantiation_signature(&instantiation);
        
        if !self.instantiation_signatures.contains(&signature) {
            self.instantiation_signatures.insert(signature);
            self.pending_instantiations.push(instantiation);
        }
    }
    
    /// Create a unique signature for an instantiation
    fn create_instantiation_signature(&self, instantiation: &GenericInstantiation) -> String {
        let mut sig = format!("{:?}:", instantiation.kind);
        sig.push_str(&format!("{:?}", instantiation.original_name));
        
        // Sort type substitutions for consistent signatures
        let mut sorted_subs: Vec<_> = instantiation.type_substitutions.iter().collect();
        sorted_subs.sort_by_key(|(k, _)| *k);
        
        for (param, type_decl) in sorted_subs {
            sig.push_str(&format!("_{:?}_{:?}", param, type_decl));
        }
        
        sig
    }
    
    /// Get all pending instantiations
    pub fn get_pending_instantiations(&self) -> &[GenericInstantiation] {
        &self.pending_instantiations
    }
    
    /// Add a type constraint for later resolution
    pub fn add_constraint(&mut self, left: TypeDecl, right: TypeDecl, context: ConstraintContext) {
        let constraint = TypeConstraint { left, right, context };
        self.constraints.push(constraint);
    }
    
    /// Solve all constraints using unification algorithm
    pub fn solve_constraints(&mut self) -> Result<HashMap<DefaultSymbol, TypeDecl>, String> {
        let mut solution = self.partial_solutions.clone();
        let mut work_queue: VecDeque<TypeConstraint> = self.constraints.iter().cloned().collect();
        
        while let Some(constraint) = work_queue.pop_front() {
            match self.unify_types(&constraint.left, &constraint.right, &mut solution) {
                Ok(new_constraints) => {
                    // Add any new constraints generated during unification
                    for new_constraint in new_constraints {
                        work_queue.push_back(new_constraint);
                    }
                }
                Err(e) => {
                    return Err(format!("Constraint solving failed: {}", e));
                }
            }
        }
        
        Ok(solution)
    }
    
    /// Unify two types, returning new constraints if needed
    fn unify_types(&self, left: &TypeDecl, right: &TypeDecl, solution: &mut HashMap<DefaultSymbol, TypeDecl>) -> Result<Vec<TypeConstraint>, String> {
        match (left, right) {
            // Generic parameter unification
            (TypeDecl::Generic(param), concrete_type) | (concrete_type, TypeDecl::Generic(param)) => {
                if let Some(existing) = solution.get(param) {
                    if existing != concrete_type {
                        return Err(format!(
                            "Conflicting type constraint: {:?} already bound to {:?}, cannot bind to {:?}",
                            param, existing, concrete_type
                        ));
                    }
                } else {
                    solution.insert(*param, concrete_type.clone());
                }
                Ok(Vec::new())
            }
            
            // Structural unification
            (TypeDecl::Array(left_elem, left_size), TypeDecl::Array(right_elem, right_size)) => {
                if left_size != right_size || left_elem.len() != right_elem.len() {
                    return Err("Array structure mismatch".to_string());
                }
                let mut new_constraints = Vec::new();
                for (l_elem, r_elem) in left_elem.iter().zip(right_elem.iter()) {
                    new_constraints.push(TypeConstraint {
                        left: l_elem.clone(),
                        right: r_elem.clone(),
                        context: ConstraintContext::Generic,
                    });
                }
                Ok(new_constraints)
            }
            
            (TypeDecl::Tuple(left_elems), TypeDecl::Tuple(right_elems)) => {
                if left_elems.len() != right_elems.len() {
                    return Err("Tuple arity mismatch".to_string());
                }
                let mut new_constraints = Vec::new();
                for (l_elem, r_elem) in left_elems.iter().zip(right_elems.iter()) {
                    new_constraints.push(TypeConstraint {
                        left: l_elem.clone(),
                        right: r_elem.clone(),
                        context: ConstraintContext::Generic,
                    });
                }
                Ok(new_constraints)
            }
            
            (TypeDecl::Dict(left_key, left_val), TypeDecl::Dict(right_key, right_val)) => {
                Ok(vec![
                    TypeConstraint {
                        left: (**left_key).clone(),
                        right: (**right_key).clone(),
                        context: ConstraintContext::Generic,
                    },
                    TypeConstraint {
                        left: (**left_val).clone(),
                        right: (**right_val).clone(),
                        context: ConstraintContext::Generic,
                    },
                ])
            }
            
            // Identical types unify trivially
            (left_type, right_type) if left_type == right_type => Ok(Vec::new()),
            
            // Number type inference
            (TypeDecl::Number, concrete_type) | (concrete_type, TypeDecl::Number) => {
                // Number can unify with any numeric type
                match concrete_type {
                    TypeDecl::UInt64 | TypeDecl::Int64 => Ok(Vec::new()),
                    _ => Err("Number type can only unify with numeric types".to_string())
                }
            }
            
            // Type mismatch
            _ => Err(format!("Cannot unify {:?} with {:?}", left, right)),
        }
    }
    
    /// Apply a solution to substitute generic parameters in a type
    pub fn apply_solution(&self, type_decl: &TypeDecl, solution: &HashMap<DefaultSymbol, TypeDecl>) -> TypeDecl {
        match type_decl {
            TypeDecl::Generic(param) => {
                solution.get(param).cloned().unwrap_or_else(|| type_decl.clone())
            }
            TypeDecl::Array(elements, size) => {
                let substituted_elements: Vec<_> = elements
                    .iter()
                    .map(|elem| self.apply_solution(elem, solution))
                    .collect();
                TypeDecl::Array(substituted_elements, *size)
            }
            TypeDecl::Tuple(elements) => {
                let substituted_elements: Vec<_> = elements
                    .iter()
                    .map(|elem| self.apply_solution(elem, solution))
                    .collect();
                TypeDecl::Tuple(substituted_elements)
            }
            TypeDecl::Dict(key_type, value_type) => {
                let substituted_key = self.apply_solution(key_type, solution);
                let substituted_value = self.apply_solution(value_type, solution);
                TypeDecl::Dict(Box::new(substituted_key), Box::new(substituted_value))
            }
            _ => type_decl.clone(),
        }
    }
    
    /// Clear all constraints but keep partial solutions
    pub fn clear_constraints(&mut self) {
        self.constraints.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use string_interner::DefaultStringInterner;

    fn create_test_symbol(interner: &mut DefaultStringInterner, name: &str) -> DefaultSymbol {
        interner.get_or_intern(name)
    }

    #[test]
    fn test_constraint_solving_basic() {
        let mut inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        
        // Add constraint: T = u64
        inference.add_constraint(
            TypeDecl::Generic(t_param),
            TypeDecl::UInt64,
            ConstraintContext::Generic
        );
        
        let solution = inference.solve_constraints().expect("Should solve successfully");
        assert_eq!(solution.get(&t_param), Some(&TypeDecl::UInt64));
    }

    #[test]
    fn test_constraint_solving_array() {
        let mut inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        
        // Add constraint: Array<T, 2> = Array<i64, 2>
        inference.add_constraint(
            TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 2),
            TypeDecl::Array(vec![TypeDecl::Int64], 2),
            ConstraintContext::Generic
        );
        
        let solution = inference.solve_constraints().expect("Should solve successfully");
        assert_eq!(solution.get(&t_param), Some(&TypeDecl::Int64));
    }

    #[test]
    fn test_constraint_solving_conflict() {
        let mut inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        
        // Add conflicting constraints: T = u64 and T = i64
        inference.add_constraint(
            TypeDecl::Generic(t_param),
            TypeDecl::UInt64,
            ConstraintContext::Generic
        );
        inference.add_constraint(
            TypeDecl::Generic(t_param),
            TypeDecl::Int64,
            ConstraintContext::Generic
        );
        
        let result = inference.solve_constraints();
        assert!(result.is_err(), "Should fail due to conflicting constraints");
    }

    #[test]
    fn test_constraint_solving_tuple() {
        let mut inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        let u_param = create_test_symbol(&mut interner, "U");
        
        // Add constraint: (T, U) = (u64, bool)
        inference.add_constraint(
            TypeDecl::Tuple(vec![TypeDecl::Generic(t_param), TypeDecl::Generic(u_param)]),
            TypeDecl::Tuple(vec![TypeDecl::UInt64, TypeDecl::Bool]),
            ConstraintContext::Generic
        );
        
        let solution = inference.solve_constraints().expect("Should solve successfully");
        assert_eq!(solution.get(&t_param), Some(&TypeDecl::UInt64));
        assert_eq!(solution.get(&u_param), Some(&TypeDecl::Bool));
    }

    #[test]
    fn test_apply_solution() {
        let inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        let mut solution = HashMap::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        solution.insert(t_param, TypeDecl::UInt64);
        
        // Test basic generic substitution
        let generic_type = TypeDecl::Generic(t_param);
        let result = inference.apply_solution(&generic_type, &solution);
        assert_eq!(result, TypeDecl::UInt64);
        
        // Test array substitution
        let array_type = TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 3);
        let result = inference.apply_solution(&array_type, &solution);
        assert_eq!(result, TypeDecl::Array(vec![TypeDecl::UInt64], 3));
        
        // Test tuple substitution
        let tuple_type = TypeDecl::Tuple(vec![TypeDecl::Generic(t_param), TypeDecl::Bool]);
        let result = inference.apply_solution(&tuple_type, &solution);
        assert_eq!(result, TypeDecl::Tuple(vec![TypeDecl::UInt64, TypeDecl::Bool]));
    }

    #[test]
    fn test_number_type_unification() {
        let mut inference = TypeInferenceState::new();
        
        // Number should unify with numeric types
        inference.add_constraint(
            TypeDecl::Number,
            TypeDecl::UInt64,
            ConstraintContext::Generic
        );
        
        let solution = inference.solve_constraints().expect("Should solve successfully");
        // Number doesn't create bindings itself, just validates compatibility
        assert!(solution.is_empty());
    }

    #[test]
    fn test_function_call_context() {
        let mut inference = TypeInferenceState::new();
        let mut interner = DefaultStringInterner::new();
        
        let t_param = create_test_symbol(&mut interner, "T");
        let fn_name = create_test_symbol(&mut interner, "test_fn");
        
        // Add constraint from function call context
        inference.add_constraint(
            TypeDecl::Generic(t_param),
            TypeDecl::String,
            ConstraintContext::FunctionCall {
                function_name: fn_name,
                arg_index: 0,
            }
        );
        
        let solution = inference.solve_constraints().expect("Should solve successfully");
        assert_eq!(solution.get(&t_param), Some(&TypeDecl::String));
    }
}