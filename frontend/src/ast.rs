#[derive(Debug, PartialEq)]
pub enum Expr {
    Binary(Box<BinaryExpr>),
    Int64(i64),
    UInt64(u64),
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
