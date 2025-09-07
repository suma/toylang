use std::collections::{HashMap, HashSet};
use string_interner::DefaultSymbol;
use crate::ast::ExprRef;
use crate::type_decl::TypeDecl;

/// Information about a generic function or struct instantiation
#[derive(Debug, Clone, PartialEq)]
pub struct GenericInstantiation {
    /// The original generic function or struct name
    pub original_name: DefaultSymbol,
    /// Type substitutions for this instantiation
    pub type_substitutions: HashMap<DefaultSymbol, TypeDecl>,
    /// Generated name for the instantiated function/struct (e.g., "identity_i64", "List_i64")
    pub instantiated_name: String,
    /// Type of instantiation (function or struct)
    pub kind: InstantiationKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InstantiationKind {
    Function,
    Struct,
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
}