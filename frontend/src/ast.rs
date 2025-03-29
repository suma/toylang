use std::rc::Rc;
use crate::type_decl::TypeDecl;

#[derive (Clone, Copy, Debug, PartialEq)]
pub struct ExprRef(pub u32);
#[derive(Debug, PartialEq, Clone)]
pub struct ExprPool(pub Vec<Expr>);

#[derive (Clone, Copy, Debug, PartialEq)]
pub struct StmtRef(pub u32);
#[derive(Debug, PartialEq, Clone)]
pub struct StmtPool(pub Vec<Stmt>);

#[derive(Debug, PartialEq, Clone)]
pub struct Node {
    pub start: usize,
    pub end: usize,
}

impl ExprRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

impl StmtRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

impl ExprPool {
    pub fn new() -> ExprPool {
        ExprPool(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> ExprPool {
        ExprPool(Vec::with_capacity(cap))
    }

    pub fn push(&mut self, expr: Expr) {
        self.0.push(expr);
    }

    pub fn add(&mut self, expr: Expr) -> ExprRef {
        let len = self.0.len();
        self.0.push(expr);
        ExprRef(len as u32)
    }

    pub fn get(&self, i: usize) -> Option<&Expr> {
        self.0.get(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl StmtPool {
    pub fn new() -> StmtPool {
        StmtPool(Vec::new())
    }
    pub fn with_capacity(cap: usize) -> StmtPool {
        StmtPool(Vec::with_capacity(cap))
    }

    pub fn push(&mut self, stmt: Stmt) {
        self.0.push(stmt);
    }

    pub fn add(&mut self, stmt: Stmt) -> StmtRef {
        let len = self.0.len();
        self.0.push(stmt);
        StmtRef(len as u32)
    }

    pub fn get(&self, i: usize) -> Option<&Stmt> {
        self.0.get(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Node {
    pub fn new(start: usize, end: usize) -> Self {
        Node {
            start,
            end,
        }
    }
}

#[derive(Debug)]
pub struct Program {
    pub node: Node,
    pub import: Vec<String>,
    pub function: Vec<Rc<Function>>,

    pub statement: StmtPool,
    pub expression: ExprPool,
}

impl Program {
    pub fn get(&self, i: u32) -> Option<&crate::ast::Expr> {
        self.expression.0.get(i as usize)
    }

    pub fn len(&self) -> usize {
        self.expression.0.len()
    }

}

#[derive(Debug, PartialEq, Clone)]
pub struct Function {
    pub node: Node,
    pub name: String,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
}

pub type Parameter = (String, TypeDecl);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, PartialEq, Clone)]
pub enum Stmt {
    Expression(ExprRef),
    Val(String, Option<TypeDecl>, ExprRef),
    Var(String, Option<TypeDecl>, Option<ExprRef>),
    Return(Option<ExprRef>),
    Break,
    Continue,
    For(String, ExprRef, ExprRef, ExprRef), // str, start, end, block
    While(ExprRef, ExprRef), // cond, block
}

#[derive(Debug, PartialEq, Clone)]
pub enum Expr {
    Assign(ExprRef, ExprRef),   // lhs = rhs
    IfElse(ExprRef, ExprRef, ExprRef),
    Binary(Operator, ExprRef, ExprRef),
    Block(Vec<StmtRef>),
    True,
    False,
    Int64(i64),
    UInt64(u64),
    Identifier(String),
    Null,
    ExprList(Vec<ExprRef>),
    Call(String, ExprRef), // apply, function call, etc
    String(String),
}

impl Expr {
    pub fn is_block(&self) -> bool {
        match self {
            Expr::Block(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
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
