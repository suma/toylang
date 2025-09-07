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
    Struct(DefaultSymbol),  // struct type
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
            // Identifier and Struct with same symbol are equivalent
            (TypeDecl::Identifier(s1), TypeDecl::Struct(s2)) |
            (TypeDecl::Struct(s1), TypeDecl::Identifier(s2)) => s1 == s2,
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
            // For all other types, no substitution needed
            _ => self.clone(),
        }
    }
}
