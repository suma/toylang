use string_interner::DefaultSymbol;
use crate::ast::{Expr, ExprRef, Operator, StmtRef};
use crate::type_checker::TypeCheckError;
use crate::type_decl::TypeDecl;

pub trait AstVisitor {
    fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError>;

    // Expr variants
    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_if_else(&mut self, cond: &ExprRef, then_block: &ExprRef, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_call(&mut self, fn_name: DefaultSymbol, args: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_int64_literal(&mut self, value: &i64) -> Result<TypeDecl, TypeCheckError>;
    fn visit_uint64_literal(&mut self, value: &u64) -> Result<TypeDecl, TypeCheckError>;
    fn visit_number_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_string_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError>;
    fn visit_boolean_literal(&mut self, value: &Expr) -> Result<TypeDecl, TypeCheckError>;
    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError>;
    fn visit_expr_list(&mut self, items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError>;

    // Stmt variants
    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError>;
    fn visit_for(&mut self, init: DefaultSymbol, cond: &ExprRef, step: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError>;
    fn visit_break(&mut self, ) -> Result<TypeDecl, TypeCheckError>;
    fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError>;
}