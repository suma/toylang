use std::collections::HashMap;
use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;

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

pub struct TypeChecker {
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
    fn new(msg: String) -> Self {
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
            // or insert new one
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

impl TypeChecker {
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
                let ty = self.type_check_expr(e)?;
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

    pub fn type_check_expr(&mut self, e: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();

        let is_block_empty = |blk: ExprRef| -> bool {
            match self.expr_pool.get(blk.to_index()).unwrap() {
                Expr::Block(expressions) => {
                    expressions.is_empty()
                }
                _ => false,
            }
        };

        let res = match self.expr_pool.get(e.to_index()).unwrap_or(&Expr::Null) {
            Expr::True | Expr::False => Ok(TypeDecl::Bool),
            Expr::IfElse(_cond, blk1, blk2) => {
                let blk1 = blk1.clone();
                let blk2 = blk2.clone();
                if is_block_empty(blk1) || is_block_empty(blk2) {
                    self.pop_context();
                    return Ok(TypeDecl::Unit); // ignore to infer empty of blk
                }

                let blk1_ty = self.check_block(&blk1)?;
                let blk2_ty = self.check_block(&blk2)?;
                if blk1_ty != blk2_ty {
                    Ok(TypeDecl::Unit)
                } else {
                    Ok(blk1_ty)
                }
            }
            Expr::Binary(op, lhs, rhs) => {
                let op = op.clone();
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                let lhs_ty = self.type_check_expr(&lhs)?;
                let rhs_ty = self.type_check_expr(&rhs)?;
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

            Expr::Block(_expressions) => {
                let res = self.check_block(e);
                res
            }
            Expr::Int64(_) => Ok(TypeDecl::Int64),
            Expr::UInt64(_) => Ok(TypeDecl::UInt64),
            Expr::String(_) => Ok(TypeDecl::String),

            Expr::Identifier(name) => {
                if let Some(val_type) = self.context.get_var(&name) {
                    Ok(val_type.clone())
                } else if let Some(fun) = self.context.get_fn(name.as_str()) {
                    Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
                } else {
                    return Err(TypeCheckError::new(format!("Identifier {:?} not found", name)));
                }
            }

            Expr::Null => Ok(TypeDecl::Any),
            Expr::ExprList(_) => Ok(TypeDecl::Unit),

            Expr::Call(fn_name, _) => {
                self.context.vars.push(HashMap::new());
                if let Some(fun) = self.context.get_fn(fn_name.as_str()) {
                    let status = self.is_checked_fn.get(fn_name.as_str());
                    if status.is_none() || status.clone().unwrap().is_none() {
                        // not checked yet
                        let fun = self.context.get_fn(fn_name).unwrap();
                        self.type_check(fun.clone())?;
                    }

                    Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
                } else {
                    Err(TypeCheckError::new(format!("Function {:?} not found", fn_name)))
                }
            }

            Expr::Assign(lhs, rhs) => {
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                let lhs_ty = self.type_check_expr(&lhs)?;
                let rhs_ty = self.type_check_expr(&rhs)?;
                if lhs_ty != rhs_ty {
                    return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
                }
                Ok(lhs_ty)
            }
        };

        self.pop_context();
        res
    }

    pub fn type_check_stmt(&mut self, s: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        let to_stmt = |e: &StmtRef| -> &Stmt { self.stmt_pool.get(e.to_index()).unwrap_or(&Stmt::Break) };

        Ok(match to_stmt(s) {
            Stmt::Expression(e) => {
                let e = e.clone();
                self.type_check_expr(&e)?
            }
            Stmt::Var(name, type_decl, expr) => {
                let name = name.clone();
                let type_decl = type_decl.clone();
                let expr = expr.clone();
                self.process_val_type(&name, &type_decl, &expr)?
            }
            Stmt::Val(name, type_decl, expr) => {
                let expr = Some(expr.clone());
                let name = name.clone();
                let type_decl = type_decl.clone();
                self.process_val_type(&name, &type_decl, &expr)?
            }
            Stmt::Return(expr) => {
                if expr.is_none() {
                    TypeDecl::Unit
                } else {
                    let e = expr.unwrap();
                    self.type_check_expr(&e)?
                }
            }
            Stmt::For(_, _, _, _) => TypeDecl::Unit,
            Stmt::While(_, _) => TypeDecl::Unit,
            Stmt::Break => TypeDecl::Unit,
            Stmt::Continue => TypeDecl::Unit,
        })
    }

    pub fn check_block(&mut self, e: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let statements = match self.expr_pool.get(e.to_index()).unwrap_or(&Expr::Null) {
            Expr::Block(statements) => {
                if statements.is_empty() {
                    return Ok(TypeDecl::Unit);
                }
                statements.clone()
            }
            _ => panic!("check_block: expected block but {:?}", self.expr_pool.get(e.to_index()).unwrap()),
        };

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
                        let ty = self.type_check_expr(&e)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                let ret_expr = self.expr_pool.get(e.to_index()).unwrap_or(&Expr::Null);
                                Err(TypeCheckError::new(format!("Type mismatch(return): expected {:?}, but got {:?} : {:?}", last, ret_expr, s)))?
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => self.type_check_stmt(&s),
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

    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code.clone();

        // Is already checked
        let func_name = func.name.clone();
        let res_ty = self.is_checked_fn.get(func_name.as_str());
        if let Some(res_ty) = res_ty {
            if let Some(res_ty) = res_ty {
                return Ok(res_ty.clone());
            } else if let None = res_ty {
                return Ok(TypeDecl::Unknown);
            }
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
            let res = self.type_check_stmt(&stmt);
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
