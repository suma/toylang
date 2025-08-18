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
}
