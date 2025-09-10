use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub enum TypeDecl {
    Unknown,
    Unit,
    Int64,
    UInt64,
    Bool,
    Identifier(DefaultSymbol),
    String,
    Number,  // Type-unspecified numeric literal for type inference
    Array(Vec<TypeDecl>, usize),  // element types and fixed size
    Struct(DefaultSymbol, Vec<TypeDecl>),  // struct type with type parameters
    Dict(Box<TypeDecl>, Box<TypeDecl>),  // Dict<K, V> - key type and value type
    Self_,  // Self type within impl blocks
    Ptr,  // Raw pointer type for heap memory
    Tuple(Vec<TypeDecl>),  // Tuple type - ordered collection of heterogeneous types
    Generic(DefaultSymbol),  // Generic type parameter (e.g., T, U, V)
}

impl TypeDecl {
    /// Check if two types are equivalent for function argument checking.
    /// This considers Identifier(symbol) and Struct(symbol) as equivalent when they have the same symbol.
    pub fn is_equivalent(&self, other: &TypeDecl) -> bool {
        if self == other {
            return true;
        }
        
        match (self, other) {
            // Identifier and Struct with same symbol are equivalent (ignore type parameters for compatibility)
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2, _)) |
            (TypeDecl::Struct(s1, _), TypeDecl::Identifier(s2)) => s1 == s2,
            // Two structs are equivalent if they have the same name and compatible type parameters
            (TypeDecl::Struct(s1, params1), TypeDecl::Struct(s2, params2)) => {
                s1 == s2 && params1.len() == params2.len() &&
                params1.iter().zip(params2.iter()).all(|(p1, p2)| p1.is_equivalent(p2))
            },
            // Generic types are compatible with any type during inference
            (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => true,
            // Unknown types are compatible with any type
            (TypeDecl::Unknown, _) | (_, TypeDecl::Unknown) => true,
            _ => false,
        }
    }
    
    /// Substitute generic type parameters with concrete types
    pub fn substitute_generics(&self, substitutions: &std::collections::HashMap<DefaultSymbol, TypeDecl>) -> TypeDecl {
        match self {
            TypeDecl::Generic(param) => {
                // If we have a substitution for this generic parameter, use it
                substitutions.get(param).cloned().unwrap_or_else(|| self.clone())
            },
            TypeDecl::Array(element_types, size) => {
                // Recursively substitute in array element types
                let new_elements = element_types.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Array(new_elements, *size)
            },
            TypeDecl::Dict(key_type, value_type) => {
                // Recursively substitute in dictionary key and value types
                let new_key = Box::new(key_type.substitute_generics(substitutions));
                let new_value = Box::new(value_type.substitute_generics(substitutions));
                TypeDecl::Dict(new_key, new_value)
            },
            TypeDecl::Tuple(element_types) => {
                // Recursively substitute in tuple element types
                let new_elements = element_types.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Tuple(new_elements)
            },
            TypeDecl::Struct(name, type_params) => {
                // Recursively substitute in struct type parameters
                let new_params = type_params.iter()
                    .map(|t| t.substitute_generics(substitutions))
                    .collect();
                TypeDecl::Struct(*name, new_params)
            },
            // For all other types, no substitution needed
            _ => self.clone(),
        }
    }
}
