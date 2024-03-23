#[derive (Clone, Copy, Debug, PartialEq)]
pub struct ExprRef(pub u32);
pub struct ExprPool(pub Vec<Expr>);

#[derive(Debug, PartialEq)]
pub enum Inst {
    Import(String),
    // Name, Parameters, Return Type, Instructions
    Function(String, ParameterList, Option<Type>, ExprRef),
    Expression(ExprRef),
}

pub type Parameter = (String, Type);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, PartialEq, Clone)]
pub enum Expr {
    IfElse(ExprRef, ExprRef, ExprRef),
    Binary(Operator, ExprRef, ExprRef),
    Block(Vec<ExprRef>),
    Int64(i64),
    UInt64(u64),
    Int(String),
    Val(String, Option<Type>, Option<ExprRef>),
    Identifier(String),
    Null,
    Call(String, ExprRef) // apply, function call, etc
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

#[derive(Debug)]
pub struct BinaryExpr {
    pub op: Operator,
    pub lhs: ExprRef,
    pub rhs: ExprRef,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Type {
    Unknown,
    Int64,
    UInt64,
    Identifier(String),
    Unit,
    Bool,
}