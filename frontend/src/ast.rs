#[derive(Debug, PartialEq)]
pub enum Expr {
    IfElse(Box<Expr>, Vec<Expr>, Vec<Expr>),
    Binary(Box<BinaryExpr>),
    Int64(i64),
    UInt64(u64),
    Val(String, Option<Type>, Option<Box<Expr>>),
    Identifier(String),
    Null,
    Call(String, Vec<Expr> /* type of arguments */), // apply, function call, etc
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
    Assign, // =
    IAdd,
    ISub,
    IMul,
    IDiv,

    // Comparison operator
    EQ, // ==
    NE, // !=
    LT, // <
    LE, // <=
    GT, // >
    GE, // >=

    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, PartialEq)]
pub struct BinaryExpr {
    pub op: Operator,
    pub lhs: Expr,
    pub rhs: Expr,
}

#[derive(Debug, PartialEq)]
pub enum Type {
    Unknown,
    Int64,
    UInt64,
    Identifier(String),
    Unit,
    Bool,
}

/*
impl Clone for Type {
    fn clone(&self) -> Self {
        match self {
            Type::Variable(v) => Type::Variable(Box::new(VarType {
                id: v.id,
                ty: v.ty.clone(),
            })),
            Type::Unknown => Type::Unknown,
            Type::Int64 => Type::Int64,
            Type::UInt64 => Type::UInt64,
            Type::Unit => Type::Unit,
            Type::Bool => Type::Bool,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct TVar {
    pub s: String,
    pub ty: Type,
}

#[derive(Debug, PartialEq)]
pub struct VarType {
    pub id: u64,
    pub ty: Type,
    //pub(crate) ptr: bool,
}
*/
