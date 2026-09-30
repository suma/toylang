use crate::type_decl::TypeDecl;
use crate::visitor::{ExprVisitor, StmtVisitor, DeclVisitor};
use super::TypeCheckError;

/// Visitor dispatch trait for expressions.
pub trait AcceptableExpr {
    fn accept_expr(&mut self, visitor: &mut dyn ExprVisitor) -> Result<TypeDecl, TypeCheckError>;
}

/// Visitor dispatch trait for statements.
pub trait AcceptableStmt {
    fn accept_stmt(&mut self, visitor: &mut dyn StmtVisitor) -> Result<TypeDecl, TypeCheckError>;
}

/// Visitor dispatch trait for declarations.
pub trait AcceptableDecl {
    fn accept_decl(&mut self, visitor: &mut dyn DeclVisitor) -> Result<TypeDecl, TypeCheckError>;
}

/// Backward-compatible alias: `Acceptable` = all three sub-traits.
/// New code should prefer the per-category traits.
pub trait Acceptable: AcceptableExpr + AcceptableStmt + AcceptableDecl {}
impl<T> Acceptable for T where T: AcceptableExpr + AcceptableStmt + AcceptableDecl {}
