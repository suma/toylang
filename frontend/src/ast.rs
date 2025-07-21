use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::type_checker::{Acceptable, TypeCheckError, SourceLocation};
use crate::type_decl::TypeDecl;
use crate::visitor::AstVisitor;

#[derive (Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

impl Node {
    pub fn to_source_location(&self, line: u32, column: u32) -> SourceLocation {
        SourceLocation {
            line,
            column,
            offset: self.start as u32,
        }
    }
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

pub struct AstBuilder {
    pub expr_pool: ExprPool,
    pub stmt_pool: StmtPool,
}

impl AstBuilder {
    pub fn new() -> Self {
        AstBuilder {
            expr_pool: ExprPool::new(),
            stmt_pool: StmtPool::new(),
        }
    }

    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        AstBuilder {
            expr_pool: ExprPool::with_capacity(expr_cap),
            stmt_pool: StmtPool::with_capacity(stmt_cap),
        }
    }

    // Legacy methods for compatibility
    pub fn add_expr(&mut self, expr: Expr) -> ExprRef {
        self.expr_pool.add(expr)
    }

    pub fn add_stmt(&mut self, stmt: Stmt) -> StmtRef {
        self.stmt_pool.add(stmt)
    }

    pub fn get_expr_pool(&self) -> &ExprPool {
        &self.expr_pool
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        &self.stmt_pool
    }

    pub fn get_expr_pool_mut(&mut self) -> &mut ExprPool {
        &mut self.expr_pool
    }

    pub fn get_stmt_pool_mut(&mut self) -> &mut StmtPool {
        &mut self.stmt_pool
    }

    pub fn extract_pools(self) -> (ExprPool, StmtPool) {
        (self.expr_pool, self.stmt_pool)
    }

    // New Builder Pattern API
    
    // Expression builders
    pub fn uint64_expr(&mut self, value: u64) -> ExprRef {
        self.expr_pool.add(Expr::UInt64(value))
    }
    
    pub fn int64_expr(&mut self, value: i64) -> ExprRef {
        self.expr_pool.add(Expr::Int64(value))
    }
    
    pub fn bool_true_expr(&mut self) -> ExprRef {
        self.expr_pool.add(Expr::True)
    }
    
    pub fn bool_false_expr(&mut self) -> ExprRef {
        self.expr_pool.add(Expr::False)
    }
    
    pub fn null_expr(&mut self) -> ExprRef {
        self.expr_pool.add(Expr::Null)
    }
    
    pub fn identifier_expr(&mut self, symbol: DefaultSymbol) -> ExprRef {
        self.expr_pool.add(Expr::Identifier(symbol))
    }
    
    pub fn string_expr(&mut self, symbol: DefaultSymbol) -> ExprRef {
        self.expr_pool.add(Expr::String(symbol))
    }
    
    pub fn number_expr(&mut self, symbol: DefaultSymbol) -> ExprRef {
        self.expr_pool.add(Expr::Number(symbol))
    }
    
    pub fn binary_expr(&mut self, op: Operator, lhs: ExprRef, rhs: ExprRef) -> ExprRef {
        self.expr_pool.add(Expr::Binary(op, lhs, rhs))
    }
    
    pub fn assign_expr(&mut self, lhs: ExprRef, rhs: ExprRef) -> ExprRef {
        self.expr_pool.add(Expr::Assign(lhs, rhs))
    }
    
    pub fn if_elif_else_expr(&mut self, cond: ExprRef, if_block: ExprRef, elif_pairs: Vec<(ExprRef, ExprRef)>, else_block: ExprRef) -> ExprRef {
        self.expr_pool.add(Expr::IfElifElse(cond, if_block, elif_pairs, else_block))
    }
    
    pub fn block_expr(&mut self, statements: Vec<StmtRef>) -> ExprRef {
        self.expr_pool.add(Expr::Block(statements))
    }
    
    pub fn call_expr(&mut self, fn_name: DefaultSymbol, args: Vec<ExprRef>) -> ExprRef {
        let args_ref = self.expr_pool.add(Expr::ExprList(args));
        self.expr_pool.add(Expr::Call(fn_name, args_ref))
    }
    
    pub fn expr_list(&mut self, exprs: Vec<ExprRef>) -> ExprRef {
        self.expr_pool.add(Expr::ExprList(exprs))
    }
    
    pub fn array_literal_expr(&mut self, elements: Vec<ExprRef>) -> ExprRef {
        self.expr_pool.add(Expr::ArrayLiteral(elements))
    }
    
    pub fn array_access_expr(&mut self, array: ExprRef, index: ExprRef) -> ExprRef {
        self.expr_pool.add(Expr::ArrayAccess(array, index))
    }
    
    pub fn field_access_expr(&mut self, object: ExprRef, field: DefaultSymbol) -> ExprRef {
        self.expr_pool.add(Expr::FieldAccess(object, field))
    }
    
    pub fn method_call_expr(&mut self, object: ExprRef, method: DefaultSymbol, args: Vec<ExprRef>) -> ExprRef {
        self.expr_pool.add(Expr::MethodCall(object, method, args))
    }
    
    pub fn struct_literal_expr(&mut self, type_name: DefaultSymbol, fields: Vec<(DefaultSymbol, ExprRef)>) -> ExprRef {
        self.expr_pool.add(Expr::StructLiteral(type_name, fields))
    }

    // Statement builders
    pub fn expression_stmt(&mut self, expr: ExprRef) -> StmtRef {
        self.stmt_pool.add(Stmt::Expression(expr))
    }
    
    pub fn val_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: ExprRef) -> StmtRef {
        self.stmt_pool.add(Stmt::Val(name, type_decl, value))
    }
    
    pub fn var_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: Option<ExprRef>) -> StmtRef {
        self.stmt_pool.add(Stmt::Var(name, type_decl, value))
    }
    
    pub fn return_stmt(&mut self, value: Option<ExprRef>) -> StmtRef {
        self.stmt_pool.add(Stmt::Return(value))
    }
    
    pub fn break_stmt(&mut self) -> StmtRef {
        self.stmt_pool.add(Stmt::Break)
    }
    
    pub fn continue_stmt(&mut self) -> StmtRef {
        self.stmt_pool.add(Stmt::Continue)
    }
    
    pub fn for_stmt(&mut self, var: DefaultSymbol, start: ExprRef, end: ExprRef, block: ExprRef) -> StmtRef {
        self.stmt_pool.add(Stmt::For(var, start, end, block))
    }
    
    pub fn while_stmt(&mut self, cond: ExprRef, block: ExprRef) -> StmtRef {
        self.stmt_pool.add(Stmt::While(cond, block))
    }
    
    pub fn struct_decl_stmt(&mut self, name: String, fields: Vec<StructField>) -> StmtRef {
        self.stmt_pool.add(Stmt::StructDecl { name, fields })
    }
    
    pub fn impl_block_stmt(&mut self, target_type: String, methods: Vec<Rc<MethodFunction>>) -> StmtRef {
        self.stmt_pool.add(Stmt::ImplBlock { target_type, methods })
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

#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    pub target_type: String,
    pub methods: Vec<Rc<MethodFunction>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodFunction {
    pub node: Node,
    pub name: DefaultSymbol,
    pub parameter: ParameterList,
    pub return_type: Option<TypeDecl>,
    pub code: StmtRef,
    pub has_self_param: bool, // true if first parameter is &self
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
    ImplBlock {
        target_type: String,
        methods: Vec<Rc<MethodFunction>>,
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
    FieldAccess(ExprRef, DefaultSymbol),  // obj.field
    MethodCall(ExprRef, DefaultSymbol, Vec<ExprRef>),  // obj.method(args)
    StructLiteral(DefaultSymbol, Vec<(DefaultSymbol, ExprRef)>),  // Point { x: 10, y: 20 }
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
