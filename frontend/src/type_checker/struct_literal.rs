use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};

/// Struct and implementation block type checking
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check struct declarations
    pub fn visit_struct_decl_new(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        
        // Push generic parameters into scope for field type checking
        if !generic_params.is_empty() {
            let generic_substitutions: HashMap<DefaultSymbol, TypeDecl> = 
                generic_params.iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            // TODO: Use proper generic scope management
        }
        
        // 1. Check for duplicate field names
        let mut field_names = HashSet::new();
        for field in fields {
            if !field_names.insert(field.name.clone()) {
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
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        match element_type {
                            TypeDecl::Identifier(struct_name) => {
                                if !self.context.struct_definitions.contains_key(struct_name) {
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
        
        Ok(TypeDecl::Unit)
    }

    /// Type check impl blocks (simplified version)
    pub fn visit_impl_block_new(&mut self, target_type: DefaultSymbol, _methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        // Check if the target struct exists
        if !self.context.struct_definitions.contains_key(&target_type) {
            return Err(TypeCheckError::not_found("Struct", &format!("{:?}", target_type)));
        }

        // TODO: Implement proper method type checking
        // For now, just validate the struct exists
        
        Ok(TypeDecl::Unit)
    }

    /// Type check field access (moved from expression.rs)
    pub fn visit_field_access_new(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        let obj_type = self.visit_expr(obj)?;
        
        match &obj_type {
            TypeDecl::Struct(struct_name) => {
                if let Some(struct_def) = self.context.get_struct_definition(*struct_name) {
                    for field_def in &struct_def.fields {
                        let field_name_str = self.core.string_interner.resolve(*field).unwrap_or("");
                        if field_def.name == field_name_str {
                            return Ok(field_def.type_decl.clone());
                        }
                    }
                    let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                    let struct_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<NOT_FOUND>");
                    Err(TypeCheckError::not_found(
                        "Field",
                        &format!("{} in struct '{}'", field_str, struct_str)
                    ))
                } else {
                    let struct_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<NOT_FOUND>");
                    Err(TypeCheckError::not_found("Struct definition", struct_str))
                }
            }
            TypeDecl::Generic(_generic_param) => {
                // Generic types don't have fields
                let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                Err(TypeCheckError::generic_error(&format!("Cannot access field '{}' on generic type parameter", field_str)))
            }
            _ => {
                let field_str = self.core.string_interner.resolve(*field).unwrap_or("<NOT_FOUND>");
                Err(TypeCheckError::type_mismatch_operation(
                    &format!("field access '{}'", field_str),
                    obj_type,
                    TypeDecl::Struct(*field)
                ))
            }
        }
    }

    /// Type check struct literals (simplified)
    pub fn visit_struct_literal_new(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        let struct_name = *struct_name;
        
        // Check if struct exists
        let struct_def = match self.context.get_struct_definition(struct_name) {
            Some(def) => def.clone(),
            None => {
                let struct_str = self.core.string_interner.resolve(struct_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::not_found("Struct", struct_str));
            }
        };
        
        // Basic field validation
        for (field_name, field_expr) in fields {
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("");
            
            // Find the expected type for this field
            let expected_type = struct_def.fields.iter()
                .find(|f| f.name == field_name_str)
                .map(|f| &f.type_decl);
            
            if let Some(expected_type) = expected_type {
                let field_type = self.visit_expr(field_expr)?;
                
                // Check type compatibility
                if field_type != *expected_type && field_type != TypeDecl::Unknown {
                    let struct_str = self.core.string_interner.resolve(struct_name).unwrap_or("<NOT_FOUND>");
                    return Err(TypeCheckError::generic_error(&format!(
                        "Type mismatch in struct '{}' field '{}': expected {:?}, found {:?}",
                        struct_str, field_name_str, expected_type, field_type
                    )));
                }
            } else {
                let struct_str = self.core.string_interner.resolve(struct_name).unwrap_or("<NOT_FOUND>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Unknown field '{}' in struct literal for '{}'", 
                    field_name_str, struct_str
                )));
            }
        }
        
        Ok(TypeDecl::Struct(struct_name))
    }
}