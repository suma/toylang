#[derive (Clone, Copy, Debug, PartialEq)]
pub struct ExprRef(pub u32);
pub struct ExprPool(pub Vec<Expr>);

#[derive(Debug, PartialEq)]
pub struct Stmt {
}

#[derive(Debug, PartialEq, Clone)]
pub struct Node {
    start: usize,
    end: usize,
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

impl Node {
    pub fn new(start: usize, end: usize) -> Self {
        Node {
            start,
            end,
        }
    }
}

pub struct Program {
    pub node: Node,
    pub import: Vec<String>,
    pub function: Vec<Function>,
    //pub expression: Vec<ExprRef>,

    pub expression: ExprPool,
}

impl Program {

    pub fn get(&self, i: u32) -> Option<&crate::ast::Expr> {
        self.expression.0.get(i as usize)
    }

    pub fn get_block(&self, i: u32) -> Option<Vec<&crate::ast::Expr>> {
        let mut expression_block: Vec<&crate::ast::Expr> = vec![];
        match self.get(i) {
            Some(e) => match e {
                crate::ast::Expr::Block(expressions) => {
                    expressions.iter().for_each(|x| expression_block.push(&self.get(x.0).unwrap()));
                }
                _ => return Option::None,
            }
            _ => return Option::None,
        }
        Some(expression_block)
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
    pub return_type: Option<Type>,
    pub code: ExprRef,
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