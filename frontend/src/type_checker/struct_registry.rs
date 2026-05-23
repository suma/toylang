use std::collections::HashMap;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::type_decl::TypeDecl;
use crate::type_checker::TypeCheckerVisitor;

/// Struct-definition and generic-substitution helpers for
/// `TypeCheckerVisitor`.
impl<'a> TypeCheckerVisitor<'a> {
    /// Build a flat map from field symbol → `"StructName.field_name"` for
    /// every registered struct.  Used by diagnostics.
    pub fn get_struct_var_mappings(
        &self,
        interner: &DefaultStringInterner,
    ) -> HashMap<DefaultSymbol, String> {
        let mut mappings = HashMap::new();
        for (struct_symbol, struct_def) in &self.context.struct_definitions {
            if let Some(struct_name) = interner.resolve(*struct_symbol) {
                for field in &struct_def.fields {
                    if let Some(field_symbol) = interner.get(&field.name) {
                        mappings.insert(field_symbol, format!("{}.{}", struct_name, field.name));
                    }
                }
            }
        }
        mappings
    }

    /// Create a mapping from a struct's generic parameter names to the
    /// concrete types supplied at a usage site.
    ///
    /// `Container<T>` + `[UInt64]` → `{T → UInt64}`.
    pub fn create_type_param_mapping(
        &self,
        struct_symbol: DefaultSymbol,
        type_params: &Vec<TypeDecl>,
    ) -> HashMap<DefaultSymbol, TypeDecl> {
        let mut mapping = HashMap::new();
        if let Some(generic_param_names) = self.context.get_struct_generic_params(struct_symbol) {
            for (param_name, concrete_type) in generic_param_names.iter().zip(type_params.iter()) {
                mapping.insert(*param_name, concrete_type.clone());
            }
        }
        mapping
    }

    /// Recursively replace `Generic(P)` occurrences in a type with the
    /// mapped concrete type.
    pub fn substitute_type_params(
        &self,
        type_decl: &TypeDecl,
        mapping: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> TypeDecl {
        match type_decl {
            TypeDecl::Generic(param_name) => {
                mapping.get(param_name).cloned().unwrap_or_else(|| type_decl.clone())
            }
            TypeDecl::Struct(name, type_params) => {
                let substituted_params: Vec<TypeDecl> = type_params
                    .iter()
                    .map(|param| self.substitute_type_params(param, mapping))
                    .collect();
                TypeDecl::Struct(*name, substituted_params)
            }
            TypeDecl::Array(element_types, size) => {
                let substituted_elements: Vec<TypeDecl> = element_types
                    .iter()
                    .map(|elem| self.substitute_type_params(elem, mapping))
                    .collect();
                TypeDecl::Array(substituted_elements, *size)
            }
            TypeDecl::Dict(key_type, value_type) => {
                let substituted_key = self.substitute_type_params(key_type, mapping);
                let substituted_value = self.substitute_type_params(value_type, mapping);
                TypeDecl::Dict(Box::new(substituted_key), Box::new(substituted_value))
            }
            TypeDecl::Tuple(element_types) => {
                let substituted_elements: Vec<TypeDecl> = element_types
                    .iter()
                    .map(|elem| self.substitute_type_params(elem, mapping))
                    .collect();
                TypeDecl::Tuple(substituted_elements)
            }
            _ => type_decl.clone(),
        }
    }
}
