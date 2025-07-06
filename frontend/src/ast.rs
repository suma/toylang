use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::type_checker::{Acceptable, TypeCheckError};
use crate::type_decl::TypeDecl;
use crate::visitor::AstVisitor;

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

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Expr> {
        self.0.get_mut(i)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn accept_expr(&self, expr_ref: &ExprRef, visitor: &mut dyn AstVisitor)
                       -> Result<TypeDecl, TypeCheckError> {
        match self.get(expr_ref.to_index()) {
            Some(expr) => expr.clone().accept(visitor),
            None => Err(TypeCheckError::new(format!("Expression not found at index: {:?}", expr_ref.to_index()))),
        }
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

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Stmt> {
        self.0.get_mut(i)
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
    pub string_interner: DefaultStringInterner,
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
    pub name: DefaultSymbol,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
}

pub type Parameter = (DefaultSymbol, TypeDecl);
pub type ParameterList = Vec<Parameter>;

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub type_decl: TypeDecl,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Stmt {
    Expression(ExprRef),
    Val(DefaultSymbol, Option<TypeDecl>, ExprRef),
    Var(DefaultSymbol, Option<TypeDecl>, Option<ExprRef>),
    Return(Option<ExprRef>),
    Break,
    Continue,
    For(DefaultSymbol, ExprRef, ExprRef, ExprRef), // str, start, end, block
    While(ExprRef, ExprRef), // cond, block
    StructDecl {
        name: String,
        fields: Vec<StructField>,
    },
}

#[derive(Debug, PartialEq, Clone)]
pub enum Expr {
    Assign(ExprRef, ExprRef),   // lhs = rhs
    IfElifElse(ExprRef, ExprRef, Vec<(ExprRef, ExprRef)>, ExprRef), // if_cond, if_block, elif_pairs, else_block
    Binary(Operator, ExprRef, ExprRef),
    Block(Vec<StmtRef>),
    True,
    False,
    Int64(i64),
    UInt64(u64),
    Number(DefaultSymbol),
    Identifier(DefaultSymbol),
    Null,
    ExprList(Vec<ExprRef>),
    Call(DefaultSymbol, ExprRef), // apply, function call, etc
    String(DefaultSymbol),
    ArrayLiteral(Vec<ExprRef>),  // [1, 2, 3, 4, 5]
    ArrayAccess(ExprRef, ExprRef),  // a[0]
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
