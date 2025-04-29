use crate::ast::{Expr, ExprRef, Operator, StmtRef};
use crate::type_checker::TypeCheckError;

pub trait AstVisitor {
    type ResultType;
    fn visit_expr(&mut self, expr: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<Self::ResultType, TypeCheckError>;

    // Expr variants
    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_if_else(&mut self, cond: &ExprRef, then_block: &ExprRef, else_block: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_identifier(&mut self, name: &str) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_call(&mut self, fn_name: &str, args: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_int64_literal(&mut self, value: &i64) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_uint64_literal(&mut self, value: &u64) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_number_literal(&mut self, value: &str) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_string_literal(&mut self, value: &str) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_boolean_literal(&mut self, value: &Expr) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_null_literal(&mut self) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_expr_list(&mut self, items: &Vec<ExprRef>) -> Result<Self::ResultType, TypeCheckError>;

    // Stmt variants
    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_var(&mut self, name: &str, type_decl: &Option<Self::ResultType>, expr: &Option<ExprRef>) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_val(&mut self, name: &str, type_decl: &Option<Self::ResultType>, expr: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_for(&mut self, init: &String, cond: &ExprRef, step: &ExprRef, body: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_break(&mut self, ) -> Result<Self::ResultType, TypeCheckError>;
    fn visit_continue(&mut self) -> Result<Self::ResultType, TypeCheckError>;
}