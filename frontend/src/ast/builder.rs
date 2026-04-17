use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::type_checker::SourceLocation;
use crate::type_decl::TypeDecl;
use super::{
    Expr, Stmt, ExprPool, StmtPool, LocationPool,
    ExprRef, StmtRef,
    Operator, UnaryOp, SliceInfo,
    BuiltinMethod, BuiltinFunction,
    StructField, Visibility, MethodFunction,
};

pub struct AstBuilder {
    pub expr_pool: ExprPool,
    pub stmt_pool: StmtPool,
    pub location_pool: LocationPool,
}

impl Default for AstBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AstBuilder {
    pub fn new() -> Self {
        AstBuilder {
            expr_pool: ExprPool::new(),
            stmt_pool: StmtPool::new(),
            location_pool: LocationPool::new(),
        }
    }

    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        AstBuilder {
            expr_pool: ExprPool::with_capacity(expr_cap),
            stmt_pool: StmtPool::with_capacity(stmt_cap),
            location_pool: LocationPool::with_capacity(expr_cap, stmt_cap),
        }
    }

    // Legacy methods for compatibility
    pub fn add_expr(&mut self, expr: Expr) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(None);
        expr_ref
    }

    pub fn add_stmt(&mut self, stmt: Stmt) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(None);
        stmt_ref
    }

    // New methods with location support
    pub fn add_expr_with_location(&mut self, expr: Expr, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn add_stmt_with_location(&mut self, stmt: Stmt, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(location);
        stmt_ref
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

    pub fn get_location_pool(&self) -> &LocationPool {
        &self.location_pool
    }

    pub fn get_location_pool_mut(&mut self) -> &mut LocationPool {
        &mut self.location_pool
    }

    pub fn extract_pools(self) -> (ExprPool, StmtPool, LocationPool) {
        (self.expr_pool, self.stmt_pool, self.location_pool)
    }

    // Expression builders
    pub fn uint64_expr(&mut self, value: u64, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::UInt64(value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn int64_expr(&mut self, value: i64, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Int64(value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn bool_true_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::True);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn bool_false_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::False);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn null_expr(&mut self, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Null);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn identifier_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Identifier(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn string_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::String(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn number_expr(&mut self, symbol: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Number(symbol));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn binary_expr(&mut self, op: Operator, lhs: ExprRef, rhs: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Binary(op, lhs, rhs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn unary_expr(&mut self, op: UnaryOp, operand: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Unary(op, operand));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn assign_expr(&mut self, lhs: ExprRef, rhs: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Assign(lhs, rhs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn if_elif_else_expr(&mut self, cond: ExprRef, if_block: ExprRef, elif_pairs: Vec<(ExprRef, ExprRef)>, else_block: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::IfElifElse(cond, if_block, elif_pairs, else_block));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn block_expr(&mut self, statements: Vec<StmtRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Block(statements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn call_expr(&mut self, fn_name: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let args_ref = self.expr_pool.add(Expr::ExprList(args));
        self.location_pool.add_expr_location(None); // args_ref location
        let expr_ref = self.expr_pool.add(Expr::Call(fn_name, args_ref));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn expr_list(&mut self, exprs: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ExprList(exprs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn array_literal_expr(&mut self, elements: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ArrayLiteral(elements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn slice_assign_expr(&mut self, object: ExprRef, start: Option<ExprRef>, end: Option<ExprRef>, value: ExprRef, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::SliceAssign(object, start, end, value));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn associated_function_call_expr(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::AssociatedFunctionCall(struct_name, function_name, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn slice_access_expr(&mut self, object: ExprRef, slice_info: SliceInfo, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::SliceAccess(object, slice_info));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn dict_literal_expr(&mut self, entries: Vec<(ExprRef, ExprRef)>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::DictLiteral(entries));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn tuple_literal_expr(&mut self, elements: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::TupleLiteral(elements));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn tuple_access_expr(&mut self, tuple: ExprRef, index: usize, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::TupleAccess(tuple, index));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn cast_expr(&mut self, expr: ExprRef, target_type: TypeDecl, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Cast(expr, target_type));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn field_access_expr(&mut self, object: ExprRef, field: DefaultSymbol, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::FieldAccess(object, field));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn method_call_expr(&mut self, object: ExprRef, method: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::MethodCall(object, method, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn struct_literal_expr(&mut self, type_name: DefaultSymbol, fields: Vec<(DefaultSymbol, ExprRef)>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::StructLiteral(type_name, fields));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn qualified_identifier_expr(&mut self, path: Vec<DefaultSymbol>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::QualifiedIdentifier(path));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn builtin_method_call_expr(&mut self, receiver: ExprRef, method: BuiltinMethod, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::BuiltinMethodCall(receiver, method, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn builtin_call_expr(&mut self, func: BuiltinFunction, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::BuiltinCall(func, args));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    // Statement builders
    pub fn expression_stmt(&mut self, expr: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Expression(expr));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn val_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Val(name, type_decl, value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn var_stmt(&mut self, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: Option<ExprRef>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Var(name, type_decl, value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn return_stmt(&mut self, value: Option<ExprRef>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Return(value));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn break_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Break);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn continue_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::Continue);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn for_stmt(&mut self, var: DefaultSymbol, start: ExprRef, end: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::For(var, start, end, block));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn while_stmt(&mut self, cond: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::While(cond, block));
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn struct_decl_stmt(&mut self, name: DefaultSymbol, generic_params: Vec<DefaultSymbol>, fields: Vec<StructField>, visibility: Visibility, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::StructDecl { name, generic_params, fields, visibility });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn impl_block_stmt(&mut self, target_type: DefaultSymbol, methods: Vec<Rc<MethodFunction>>, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::ImplBlock { target_type, methods });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
}
