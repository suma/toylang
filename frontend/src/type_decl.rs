#[derive(Debug, PartialEq, Clone)]
pub enum TypeDecl {
    Unknown,
    Unit,
    Int64,
    UInt64,
    Bool,
    Identifier(String),
    Any,  // null
}
