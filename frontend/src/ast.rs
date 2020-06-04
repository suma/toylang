#[derive(Debug, PartialEq)]
pub enum Expr {
    Binary(Box<BinaryExpr>),
    Int64(i64),
    UInt64(u64),
}

#[derive(Debug, PartialEq)]
pub enum Operator {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, PartialEq)]
pub struct BinaryExpr {
    pub op: Operator,
    pub lhs: Expr,
    pub rhs: Expr,
}
