use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};

/// Struct declaration type checking implementation
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check struct declarations
    pub fn visit_struct_decl_impl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        
        // Push generic parameters into scope for field type checking
        if !generic_params.is_empty() {
            let generic_substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = 
                generic_params.iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            self.type_inference.push_generic_scope(generic_substitutions);
        }
        
        // 1. Check for duplicate field names
        let mut field_names = std::collections::HashSet::new();
        for field in fields {
            if !field_names.insert(field.name.clone()) {
                if !generic_params.is_empty() {
                    self.type_inference.pop_generic_scope();
                }
                return Err(TypeCheckError::generic_error(&format!(
                    "Duplicate field '{}' in struct '{:?}'", field.name, name
                )));
            }
        }
        
        // 2. Validate field types
        for field in fields {
            match &field.type_decl {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String => {
                    // Basic types are valid
                },
                TypeDecl::Generic(_) => {
                    // Generic types are valid if they're in scope
                },
                TypeDecl::Identifier(struct_name) => {
                    // Check if referenced struct is already defined
                    if !self.context.struct_definitions.contains_key(struct_name) {
                        if !generic_params.is_empty() {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        match element_type {
                            TypeDecl::Identifier(struct_name) => {
                                if !self.context.struct_definitions.contains_key(struct_name) {
                                    if !generic_params.is_empty() {
                                        self.type_inference.pop_generic_scope();
                                    }
                                    return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                                }
                            },
                            TypeDecl::Generic(_) => {
                                // Generic array elements are valid
                            },
                            _ => {}
                        }
                    }
                },
                _ => {
                    if !generic_params.is_empty() {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("field type in struct '{:?}'", name), field.type_decl.clone()
                    ));
                }
            }
        }
        
        // 3. Register struct definition with visibility information
        let struct_symbol = name;
        let struct_def = crate::type_checker::context::StructDefinition {
            fields: fields.clone(),
            visibility: visibility.clone(),
        };
        
        // Store the struct definition for later type checking and access control
        self.context.struct_definitions.insert(struct_symbol, struct_def);
        
        // Register generic parameters if any
        if !generic_params.is_empty() {
            self.context.set_struct_generic_params(name, generic_params.clone());
        }
        
        // Pop generic scope after processing
        if !generic_params.is_empty() {
            self.type_inference.pop_generic_scope();
        }
        
        Ok(TypeDecl::Unit)
    }
}