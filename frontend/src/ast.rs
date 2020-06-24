use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, PartialEq)]
pub enum Expr {
    Binary(Rc<RefCell<BinaryExpr>>),
    Int64(i64),
    UInt64(u64),
    Val(String, TVar, Option<Rc<RefCell<Expr>>>),
    Identifier(TVar),
    Null,
    Call(TVar, Vec<Expr> /* type of arguments */)    // apply, function call, etc
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
    Variable(Rc<RefCell<VarType>>),
    Unit,
    Bool,
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
