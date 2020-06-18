#[derive(Debug, PartialEq)]
pub enum Expr {
    Binary(Box<BinaryExpr>),
    Int64(i64),
    UInt64(u64),
    Identifier(TVar),
    Null,
    Call(TVar, Vec<Expr> /* type of arguments */)    // apply, function call, etc
}

#[derive(Debug, PartialEq)]
pub enum Operator {
    IAdd,
    ISub,
    IMul,
    IDiv,

    DoubleEqual,    // ==
    NotEqual,       // !=
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
    Variable(Box<VarType>),
}

#[derive(Debug, PartialEq)]
pub struct TVar {
    pub(crate) s: String,
    pub(crate) ty: Type,
}

#[derive(Debug, PartialEq)]
pub struct VarType {
    pub(crate) id: u64,
    pub(crate) ty: Type,
}
