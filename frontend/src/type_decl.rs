use string_interner::DefaultSymbol;

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub enum TypeDecl {
    Unknown,
    Unit,
    Int64,
    UInt64,
    Bool,
    Identifier(DefaultSymbol),
    Null,  // null
    String,
    Number,  // Type-unspecified numeric literal for type inference
    Array(Vec<TypeDecl>, usize),  // element types and fixed size
    Struct(DefaultSymbol),  // struct type
    Dict(Box<TypeDecl>, Box<TypeDecl>),  // Dict<K, V> - key type and value type
    Self_,  // Self type within impl blocks
    Ptr,  // Raw pointer type for heap memory
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
}
