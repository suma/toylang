use std::collections::HashMap;
use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::AstVisitor;

#[derive(Debug)]
pub struct VarState {
    ty: TypeDecl,
    is_const: bool,
}
#[derive(Debug)]
pub struct TypeCheckContext {
    vars: Vec<HashMap<String, VarState>>,
    functions: HashMap<String, Rc<Function>>,
}

#[derive(Debug)]
pub struct TypeCheckError {
    msg: String,
}

pub struct TypeCheckerVisitor {
    pub stmt_pool: StmtPool,
    pub expr_pool: ExprPool,
    pub context: TypeCheckContext,
    pub call_depth: usize,
    pub is_checked_fn: HashMap<String, Option<TypeDecl>>, // None -> in progress, Some -> Done
}

impl std::fmt::Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl TypeCheckError {
    pub fn new(msg: String) -> Self {
        Self { msg }
    }
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::new()],
            functions: HashMap::new(),
        }
    }

    pub fn set_val(&mut self, name: &str, ty: TypeDecl) {
        let last = self.vars.last_mut().unwrap();
        last.insert(name.to_string(), VarState { ty, is_const: true });
    }

    pub fn set_var(&mut self, name: &str, ty: TypeDecl) {
        let last = self.vars.last_mut().unwrap();
        let exist = last.get(name);
        if let Some(exist) = exist {
            if exist.is_const {
                panic!("Cannot re-assign const variable: {:?}", name);
            }
            let ety = exist.ty.clone();
            if ety != ty {
                panic!("Cannot re-assign variable: {:?} with different type: {:?} != {:?}", name, ety, ty);
            }
            // it can overwrite
        } else {
            // or insert a new one
            last.insert(name.to_string(), VarState { ty, is_const: false });
        }
    }

    pub fn set_fn(&mut self, name: &str, f: Rc<Function>) {
        self.functions.insert(name.to_string(), f);
    }

    pub fn get_var(&self, name: &str) -> Option<TypeDecl> {
        for v in self.vars.iter().rev() {
            let v_val = v.get(name);
            if let Some(val) = v_val {
                return Some(val.ty.clone());
            }
        }
        None
    }

    pub fn get_fn(&self, name: &str) -> Option<Rc<Function>> {
        let name = name.to_string();
        if let Some(val) = self.functions.get(&name) {
            Some(val.clone())
        } else {
            None
        }
    }
}


impl TypeCheckerVisitor {
    pub fn new(stmt_pool: StmtPool, expr_pool: ExprPool) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            context: TypeCheckContext::new(),
            call_depth: 0,
            is_checked_fn: HashMap::new(),
        }
    }

    pub fn push_context(&mut self) {
        self.context.vars.push(HashMap::new());
    }

    pub fn pop_context(&mut self) {
        self.context.vars.pop();
    }

    pub fn add_function(&mut self, f: Rc<Function>) {
        self.context.set_fn(f.name.as_str(), f.clone());
    }

    fn process_val_type(&mut self, name: &String, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let expr_ty = match expr {
            Some(e) => {
                let ty = self.visit_expr(e)?;
                if ty == TypeDecl::Unit {
                    return Err(TypeCheckError::new(format!("Type mismatch: expected <expression>, but got {:?}", ty)));
                }
                Some(ty)
            }
            None => None,
        };

        match (type_decl, expr_ty.as_ref()) {
            (Some(TypeDecl::Unknown), Some(ty)) => {
                self.context.set_var(name.as_str(), ty.clone());
            }
            (Some(decl), Some(ty)) => {
                if decl != ty {
                    return Err(TypeCheckError::new(format!("Type mismatch: expected {:?}, but got {:?}", decl, ty)));
                }
                self.context.set_var(name.as_str(), ty.clone());
            }
            _ => (),
        }

        Ok(TypeDecl::Unit)
    }

    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code.clone();

        // Is already checked
        let func_name = func.name.clone();
        match self.is_checked_fn.get(func_name.as_str()) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // Now checking...
        self.is_checked_fn.insert(func_name.clone(), None);

        self.call_depth += 1;

        let statements = match self.stmt_pool.get(s.to_index()).unwrap() {
            Stmt::Expression(e) => {
                match self.expr_pool.0.get(e.to_index()).unwrap() {
                    Expr::Block(statements) => {
                        statements.clone()  // TODO: I want to avoid clone
                    }
                    _ => {
                        panic!("type_check: expected block but {:?}", self.expr_pool.get(s.to_index()).unwrap());
                    }
                }
            }
            _ => panic!("type_check: expected block but {:?}", self.expr_pool.get(s.to_index()).unwrap()),
        };

        self.push_context();
        // Define variable of argument for this `func`
        func.parameter.iter().for_each(|(name, type_decl)| {
            self.context.set_var(name.as_str(), type_decl.clone());
        });

        for stmt in statements {
            let res = self.stmt_pool.get(stmt.to_index()).unwrap().clone().accept(self);
            if res.is_err() {
                return res;
            } else {
                last = res?;
            }
        }
        self.pop_context();
        self.call_depth -= 1;

        self.is_checked_fn.insert(func_name, Some(last.clone()));
        Ok(last)
    }
}
pub trait Acceptable {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError>;
}

impl Acceptable for Expr {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Expr::Binary(op, lhs, rhs) => visitor.visit_binary(op, lhs, rhs),
            Expr::Block(statements) => visitor.visit_block(statements),
            Expr::IfElse(cond, then_block, else_block) => visitor.visit_if_else(cond, then_block, else_block),
            Expr::Assign(lhs, rhs) => visitor.visit_assign(lhs, rhs),
            Expr::Identifier(name) => visitor.visit_identifier(name),
            Expr::Call(fn_name, args) => visitor.visit_call(fn_name, args),
            Expr::Int64(val) => visitor.visit_int64_literal(val),
            Expr::UInt64(val) => visitor.visit_uint64_literal(val),
            Expr::Number(val) => visitor.visit_number_literal(val),
            Expr::String(val) => visitor.visit_string_literal(val),
            Expr::True | Expr::False => visitor.visit_boolean_literal(self),
            Expr::Null => visitor.visit_null_literal(),
            Expr::ExprList(items) => visitor.visit_expr_list(items),
        }
    }
}

impl Acceptable for Stmt {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Stmt::Expression(expr) => visitor.visit_expression_stmt(expr),
            Stmt::Var(name, type_decl, expr) => visitor.visit_var(name, type_decl, expr),
            Stmt::Val(name, type_decl, expr) => visitor.visit_val(name, type_decl, expr),
            Stmt::Return(expr) => visitor.visit_return(expr),
            Stmt::For(init, cond, step, body) => visitor.visit_for(init, cond, step, body),
            Stmt::While(cond, body) => visitor.visit_while(cond, body),
            Stmt::Break => visitor.visit_break(),
            Stmt::Continue => visitor.visit_continue()
        }
    }
}

impl AstVisitor for TypeCheckerVisitor {
    fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(expr.to_index()).unwrap().clone().accept(self)
    }

    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        self.stmt_pool.get(stmt.to_index()).unwrap_or(&Stmt::Break).clone().accept(self)
    }

    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = self.expr_pool.get(lhs.to_index()).unwrap().clone().accept(self)?;
        let rhs_ty = self.expr_pool.get(rhs.to_index()).unwrap().clone().accept(self)?;
        if lhs_ty != rhs_ty {
            return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
        }
        match op {
            Operator::IAdd if lhs_ty == TypeDecl::String && rhs_ty == TypeDecl::String => {
                Ok(TypeDecl::String)
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul |
            Operator::LE | Operator::LT | Operator::GE | Operator::GT => {
                if lhs_ty == TypeDecl::UInt64 {
                    if rhs_ty != TypeDecl::UInt64 {
                        return Err(TypeCheckError::new(format!("Type mismatch: lhs expected UInt64, but rhs got {:?}", rhs_ty)));
                    }
                    Ok(TypeDecl::UInt64)
                } else if lhs_ty == TypeDecl::Int64 {
                    if rhs_ty != TypeDecl::Int64 {
                        return Err(TypeCheckError::new(format!("Type mismatch: lhs expected Int64, but rhs got {:?}", rhs_ty)));
                    }
                    Ok(TypeDecl::Int64)
                } else {
                    return Err(TypeCheckError::new(format!("Type mismatch: lhs expected Int64 or UInt64, but rhs got {:?}", rhs_ty)));
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if lhs_ty == TypeDecl::Bool && rhs_ty == TypeDecl::Bool {
                    Ok(TypeDecl::Bool)
                } else {
                    Err(TypeCheckError::new(format!("Type mismatch(bool): lhs expected Bool, but rhs got {:?}", rhs_ty)))
                }
            }
            _ => Err(TypeCheckError::new(format!("Type mismatch: expected {:?}, but got {:?}", op, lhs_ty))),
        }
    }

    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements {
            let stmt = self.stmt_pool.get(s.to_index()).unwrap();
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let ty = self.expr_pool.get(e.to_index()).unwrap().clone().accept(self)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                let ret_expr = self.expr_pool.get(e.to_index()).unwrap();
                                Err(TypeCheckError::new(format!("Type mismatch(return): expected {:?}, but got {:?} : {:?}", last, ret_expr, s)))?
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => self.stmt_pool.get(s.to_index()).unwrap().clone().accept(self),
            };

            match stmt_type {
                Ok(def_ty) => last = Some(def_ty),
                Err(e) => return Err(e),
            }
        }

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::new(format!("Type of block mismatch: expected {:?}", last)))
        }
    }

    fn visit_if_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let is_block_empty = |blk: ExprRef| -> bool {
            match self.expr_pool.get(blk.to_index()).unwrap() {
                Expr::Block(expressions) => {
                    expressions.is_empty()
                }
                _ => false,
            }
        };
        let blk1 = then_block.clone();
        let blk2 = else_block.clone();
        if is_block_empty(blk1) || is_block_empty(blk2) {
            return Ok(TypeDecl::Unit); // ignore to infer empty of blk
        }

        let blk1_ty = self.expr_pool.get(blk1.to_index()).unwrap().clone().accept(self)?;
        let blk2_ty = self.expr_pool.get(blk2.to_index()).unwrap().clone().accept(self)?;
        if blk1_ty != blk2_ty {
            Ok(TypeDecl::Unit)
        } else {
            Ok(blk1_ty)
        }
    }

    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = self.expr_pool.get(lhs.to_index()).unwrap().clone().accept(self)?;
        let rhs_ty = self.expr_pool.get(rhs.to_index()).unwrap().clone().accept(self)?;
        if lhs_ty != rhs_ty {
            return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
        }
        Ok(lhs_ty)
    }

    fn visit_identifier(&mut self, name: &str) -> Result<TypeDecl, TypeCheckError> {
        if let Some(val_type) = self.context.get_var(&name) {
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            return Err(TypeCheckError::new(format!("Identifier {:?} not found", name)));
        }
    }

    fn visit_call(&mut self, fn_name: &str, _args: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        if let Some(fun) = self.context.get_fn(fn_name) {
            let status = self.is_checked_fn.get(fn_name);
            if status.is_none() || status.clone().unwrap().is_none() {
                // not checked yet
                let fun = self.context.get_fn(fn_name).unwrap();
                self.type_check(fun.clone())?;
            }

            self.pop_context();
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            self.pop_context();
            Err(TypeCheckError::new(format!("Function {:?} not found", fn_name)))
        }
    }

    fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    fn visit_number_literal(&mut self, _value: &str) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    fn visit_string_literal(&mut self, _value: &str) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Bool)

    }

    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Any)
    }

    fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(expr.to_index()).unwrap().clone().accept(self)
    }

    fn visit_var(&mut self, name: &str, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let name = name.to_string();
        let type_decl = type_decl.clone();
        let expr = expr.clone();
        self.process_val_type(&name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    fn visit_val(&mut self, name: &str, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr = Some(expr.clone());
        let name = name.to_string();
        let type_decl = type_decl.clone();
        self.process_val_type(&name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if expr.is_none() {
            Ok(TypeDecl::Unit)
        } else {
            let e = expr.unwrap();
            self.expr_pool.get(e.to_index()).unwrap().clone().accept(self)?;
            Ok(TypeDecl::Unit)
        }
    }

    fn visit_for(&mut self, _init: &String, _cond: &ExprRef, _step: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(body.to_index()).unwrap().clone().accept(self)
    }

    fn visit_while(&mut self, _cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.expr_pool.get(body.to_index()).unwrap().clone().accept(self)
    }

    fn visit_break(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }
}