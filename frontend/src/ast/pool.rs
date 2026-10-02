use crate::type_checker::{AcceptableExpr, TypeCheckError, SourceLocation};
use crate::type_decl::TypeDecl;
use crate::visitor::ExprVisitor;
use super::{Expr, Stmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExprRef(pub u32);

impl ExprRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

// `Eq` / `Hash` so a statement can be a set key — `File::
// transferred_bindings` records ownership transfer per `val` / `var`
// statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StmtRef(pub u32);

impl StmtRef {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExprType {
    Assign = 0,
    IfElifElse = 1,
    Binary = 2,
    Unary = 3,
    Block = 4,
    True = 5,
    False = 6,
    Int64 = 7,
    UInt64 = 8,
    Number = 9,
    Identifier = 10,
    Null = 11,
    ExprList = 12,
    Call = 13,
    String = 14,
    ArrayLiteral = 15,
    FieldAccess = 16,
    MethodCall = 17,
    StructLiteral = 18,
    QualifiedIdentifier = 19,
    BuiltinMethodCall = 20,
    BuiltinCall = 21,
    SliceAccess = 22,
    SliceAssign = 23,
    AssociatedFunctionCall = 24,
    DictLiteral = 25,
    TupleLiteral = 26,
    TupleAccess = 27,
    Cast = 28,
    With = 29,
    Match = 30,
    Range = 31,
    Float64 = 32,
    Float32 = 43,
    /// `'a'` — a char literal. Stored like `UInt32` (the code point
    /// in `uint64_val`) but discriminated, because the type checker
    /// treats the two differently: a char literal may take a
    /// different integer type when the position asks for one and the
    /// value fits, while `42u32` may not.
    CharLiteral = 44,
    // NUM-W narrow integer literal discriminants. Storage
    // piggybacks on the existing `int64_val` / `uint64_val`
    // arrays (the lexer already validates the value fits at the
    // narrow width); the discriminant here is what subsequent
    // passes use to recover the exact type.
    Int8 = 33,
    Int16 = 34,
    Int32 = 35,
    UInt8 = 36,
    UInt16 = 37,
    UInt32 = 38,
    /// `fn(params) -> Ret { body }` — closure / lambda literal. Phase 1.
    Closure = 39,
    /// `expr?` — postfix early-return operator. The parser emits this
    /// node; the type checker rewrites it in-place to a `Match` so
    /// backends never observe it.
    Try = 40,
    /// `P { x: 1i64, ..base }` — struct update syntax. Like `Try`, the
    /// parser emits it and the type checker rewrites it in place, so
    /// backends never observe it.
    StructUpdate = 41,
    /// `a ?? b` — null-coalesce. Like `Try`, the parser emits it and
    /// the type checker rewrites it in place to a lazy `match`, so
    /// backends never observe it.
    NullCoalesce = 42,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StmtType {
    Expression = 0,
    Val = 1,
    Var = 2,
    Return = 3,
    Break = 4,
    Continue = 5,
    For = 6,
    While = 7,
    StructDecl = 8,
    ImplBlock = 9,
    EnumDecl = 10,
    TraitDecl = 11,
    TypeAlias = 12,
}

/// Every expression of a program, addressed by [`ExprRef`].
///
/// Stored as the `Expr` values themselves. Until AST-BORROW this was one
/// column per field (`lhs`, `rhs`, `expr_list`, ...), and `get` assembled
/// an owned `Expr` on every call -- cloning its child lists -- because
/// there was no `Expr` in memory to lend. [`Self::get_ref`] lends one; a
/// pass that only reads should use it.
#[derive(Debug, PartialEq, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExprPool {
    exprs: Vec<Expr>,
}

impl ExprPool {
    pub fn new() -> ExprPool {
        ExprPool { exprs: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> ExprPool {
        ExprPool { exprs: Vec::with_capacity(cap) }
    }

    pub fn add(&mut self, expr: Expr) -> ExprRef {
        let index = self.exprs.len();
        self.exprs.push(expr);
        ExprRef(index as u32)
    }

    /// The expression at `expr_ref`, owned. Clones its child lists; a
    /// caller that only reads should take [`Self::get_ref`].
    pub fn get(&self, expr_ref: &ExprRef) -> Option<Expr> {
        self.get_ref(expr_ref).cloned()
    }

    /// The expression at `expr_ref`, borrowed from the pool.
    pub fn get_ref(&self, expr_ref: &ExprRef) -> Option<&Expr> {
        self.exprs.get(expr_ref.to_index())
    }

    pub fn len(&self) -> usize {
        self.exprs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exprs.is_empty()
    }

    /// Replace the expression at `expr_ref`. Out-of-range refs are
    /// ignored.
    pub fn update(&mut self, expr_ref: &ExprRef, expr: Expr) {
        if let Some(slot) = self.exprs.get_mut(expr_ref.to_index()) {
            *slot = expr;
        }
    }

    pub fn accept_expr(&self, expr_ref: &ExprRef, visitor: &mut dyn ExprVisitor)
                       -> Result<TypeDecl, TypeCheckError> {
        match self.get(expr_ref) {
            Some(mut expr) => expr.accept_expr(visitor),
            None => Err(TypeCheckError::new(format!("Expression not found: {:?}", expr_ref))),
        }
    }
}

/// Every statement of a program, addressed by [`StmtRef`]. Stored as the
/// `Stmt` values themselves, like [`ExprPool`].
#[derive(Debug, PartialEq, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StmtPool {
    stmts: Vec<Stmt>,
}

impl StmtPool {
    pub fn new() -> StmtPool {
        StmtPool { stmts: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> StmtPool {
        StmtPool { stmts: Vec::with_capacity(cap) }
    }

    pub fn add(&mut self, stmt: Stmt) -> StmtRef {
        let index = self.stmts.len();
        self.stmts.push(stmt);
        StmtRef(index as u32)
    }

    /// Replace the statement at `stmt_ref`. Exists primarily so the
    /// module integration pass can install placeholder slots up front
    /// and fill them with the real remapped statements once every
    /// `StmtRef` / `ExprRef` has been redirected. Out-of-range refs are
    /// ignored, matching `ExprPool::update`.
    pub fn update(&mut self, stmt_ref: &StmtRef, stmt: Stmt) {
        if let Some(slot) = self.stmts.get_mut(stmt_ref.to_index()) {
            *slot = stmt;
        }
    }

    /// The statements of one kind, in pool order. A pass that wants
    /// only the trait or impl declarations picks them out here instead
    /// of building every statement in the program to look at its tag.
    pub fn refs_of(&self, kind: StmtType) -> impl Iterator<Item = StmtRef> + '_ {
        self.stmts
            .iter()
            .enumerate()
            .filter(move |(_, s)| stmt_kind(s) == kind)
            .map(|(i, _)| StmtRef(i as u32))
    }

    /// The statement at `stmt_ref`, owned. Clones its contents (an impl
    /// block's method list, a struct's fields); a caller that only
    /// reads should take [`Self::get_ref`].
    pub fn get(&self, stmt_ref: &StmtRef) -> Option<Stmt> {
        self.get_ref(stmt_ref).cloned()
    }

    /// The statement at `stmt_ref`, borrowed from the pool.
    pub fn get_ref(&self, stmt_ref: &StmtRef) -> Option<&Stmt> {
        self.stmts.get(stmt_ref.to_index())
    }

    pub fn len(&self) -> usize {
        self.stmts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stmts.is_empty()
    }
}

/// The [`StmtType`] tag of a statement.
pub fn stmt_kind(stmt: &Stmt) -> StmtType {
    match stmt {
        Stmt::Expression(_) => StmtType::Expression,
        Stmt::Val(..) => StmtType::Val,
        Stmt::Var(..) => StmtType::Var,
        Stmt::Return(_) => StmtType::Return,
        Stmt::Break(_) => StmtType::Break,
        Stmt::Continue(_) => StmtType::Continue,
        Stmt::For(..) => StmtType::For,
        Stmt::While(..) => StmtType::While,
        Stmt::StructDecl { .. } => StmtType::StructDecl,
        Stmt::ImplBlock { .. } => StmtType::ImplBlock,
        Stmt::EnumDecl { .. } => StmtType::EnumDecl,
        Stmt::TraitDecl { .. } => StmtType::TraitDecl,
        Stmt::TypeAlias { .. } => StmtType::TypeAlias,
    }
}


/// Location information storage for AST nodes
#[derive(Debug, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocationPool {
    pub expr_locations: Vec<Option<SourceLocation>>,
    pub stmt_locations: Vec<Option<SourceLocation>>,
}

impl Default for LocationPool {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationPool {
    pub fn new() -> Self {
        Self {
            expr_locations: Vec::new(),
            stmt_locations: Vec::new(),
        }
    }

    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        Self {
            expr_locations: Vec::with_capacity(expr_cap),
            stmt_locations: Vec::with_capacity(stmt_cap),
        }
    }

    pub fn add_expr_location(&mut self, location: Option<SourceLocation>) {
        self.expr_locations.push(location);
    }

    pub fn add_stmt_location(&mut self, location: Option<SourceLocation>) {
        self.stmt_locations.push(location);
    }

    pub fn get_expr_location(&self, expr_ref: &ExprRef) -> Option<&SourceLocation> {
        self.expr_locations.get(expr_ref.to_index())?.as_ref()
    }

    pub fn get_stmt_location(&self, stmt_ref: &StmtRef) -> Option<&SourceLocation> {
        self.stmt_locations.get(stmt_ref.to_index())?.as_ref()
    }

    pub fn set_expr_location(&mut self, expr_ref: &ExprRef, location: SourceLocation) {
        if let Some(loc) = self.expr_locations.get_mut(expr_ref.to_index()) {
            *loc = Some(location);
        }
    }

}
