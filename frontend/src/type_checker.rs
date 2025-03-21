use std::collections::HashMap;
use std::rc::Rc;
use crate::ast::*;
use crate::type_decl::*;

#[derive(Debug)]
pub struct TypeCheckContext {
    vars: HashMap<String, TypeDecl>,
    functions: HashMap<String, Rc<Function>>,
    super_context: Option<Box<TypeCheckContext>>,
}

#[derive(Debug)]
pub struct TypeCheckError {
    msg: String,
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
            vars: HashMap::new(),
            functions: HashMap::new(),
            super_context: None,
        }
    }

    pub fn set_var(&mut self, name: &str, ty: TypeDecl) {
        self.vars.insert(name.to_string(), ty);
    }

    pub fn set_fn(&mut self, name: &str, f: Rc<Function>) {
        self.functions.insert(name.to_string(), f);
    }

    pub fn get_var(&self, name: &str) -> Option<TypeDecl> {
        let name = name.to_string();
        if let Some(val) = self.vars.get(&name) {
            Some(val.clone())
        } else if let Some(box super_context) = &self.super_context {
            super_context.get_var(&name)
        } else {
            None
        }
    }

    pub fn get_fn(&self, name: &str) -> Option<Rc<Function>> {
        let name = name.to_string();
        if let Some(val) = self.functions.get(&name) {
            Some(val.clone())
        } else if let Some(box super_context) = &self.super_context {
            super_context.get_fn(&name)
        } else {
            None
        }
    }
}

fn process_val_type(ast: &ExprPool, ctx: &mut TypeCheckContext, name: &String, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
    let mut expr_ty: Option<TypeDecl> = None;
    if expr.is_some() {
        let ty = type_check(ast, expr.unwrap(), ctx)?;
        if ty != TypeDecl::Unit {
            expr_ty = Some(ty);
        } else {
            return Err(TypeCheckError::new(format!("Type mismatch: expected <expression>, but got {:?}", ty)));
        }
    }
    match type_decl {
        Some(TypeDecl::Unknown) => {
            ctx.set_var(name.as_str(), expr_ty.clone().unwrap());
        }
        Some(type_decl) => {
            if expr_ty.is_some() && *type_decl != expr_ty.clone().unwrap() {
                return Err(TypeCheckError::new(format!("Type mismatch: expected {:?}, but got {:?}", type_decl, expr_ty.unwrap())));
            }
            ctx.set_var(name.as_str(), expr_ty.clone().unwrap());
        }
        _ => (),
    }
    Ok(TypeDecl::Unit)
}

pub fn type_check(ast: &ExprPool, e: ExprRef, ctx: &mut TypeCheckContext) -> Result<TypeDecl, TypeCheckError> {
    let is_block_empty = |blk: ExprRef| -> bool {
        match ast.0.get(blk.0 as usize).unwrap() {
            Expr::Block(expressions) => {
                expressions.is_empty()
            }
            _ => false,
        }
    };
    Ok(match ast.0.get(e.0 as usize).unwrap_or(&Expr::Null) {
        Expr::True | Expr::False => TypeDecl::Bool,
        Expr::IfElse(_cond, blk1, blk2) => {
            let blk1_ty = if is_block_empty(*blk1) {
                TypeDecl::Unit
            } else {
                check_block(ast, *blk1, ctx)?
            };
            let blk2_empty = is_block_empty(*blk2);
            if blk2_empty {
                blk1_ty // ignore to infer empty of blk2
            } else {
                let blk2_ty = check_block(ast, *blk2, ctx)?;
                // eprintln!("If blk type {:?} {:?}", blk1_ty, blk2_ty);
                if blk1_ty == blk2_ty {
                    blk1_ty
                } else {
                    TypeDecl::Unit
                }
            }
        }
        Expr::Binary(op, lhs, rhs) => {
            let lhs_ty = type_check(ast, *lhs, ctx)?;
            let rhs_ty = type_check(ast, *rhs, ctx)?;
            if lhs_ty != rhs_ty {
                return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
            }
            match op {
                Operator::IAdd if lhs_ty == TypeDecl::String && rhs_ty == TypeDecl::String => {
                    TypeDecl::String
                }
                Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul |
                    Operator::LE | Operator::LT | Operator::GE | Operator::GT => {
                    if lhs_ty == TypeDecl::UInt64 {
                        if rhs_ty != TypeDecl::UInt64 {
                            return Err(TypeCheckError::new(format!("Type mismatch: lhs expected UInt64, but rhs got {:?}", rhs_ty)));
                        }
                        TypeDecl::UInt64
                    } else if lhs_ty == TypeDecl::Int64 {
                        if rhs_ty != TypeDecl::Int64 {
                            return Err(TypeCheckError::new(format!("Type mismatch: lhs expected Int64, but rhs got {:?}", rhs_ty)));
                        }
                        TypeDecl::Int64
                    } else {
                        return Err(TypeCheckError::new(format!("Type mismatch: lhs expected Int64 or UInt64, but rhs got {:?}", rhs_ty)));
                    }
                }
                Operator::LogicalAnd | Operator::LogicalOr => {
                    if lhs_ty == TypeDecl::Bool && rhs_ty == TypeDecl::Bool {
                        TypeDecl::Bool
                    } else {
                        return Err(TypeCheckError::new(format!("Type mismatch: lhs expected Bool, but rhs got {:?}", rhs_ty)));
                    }
                }
                _ => return Err(TypeCheckError::new(format!("Type mismatch: expected {:?}, but got {:?}", op, lhs_ty))),
            }

        }
        Expr::Block(_expressions) => {
            check_block(ast, e, ctx)?
        }
        Expr::Int64(_) => TypeDecl::Int64,
        Expr::UInt64(_) => TypeDecl::UInt64,
        Expr::String(_) => TypeDecl::String,
        Expr::Var(name, type_decl, expr) => {
            process_val_type(ast, ctx, name, type_decl, expr)?
        }
        Expr::Val(name, type_decl, expr) => {
            process_val_type(ast, ctx, name, type_decl, expr)?
        }
        Expr::Identifier(name) => {
            if let Some(val_type) = ctx.get_var(&name) {
                val_type.clone()
            } else if let Some(fun) = ctx.get_fn(name.as_str()) {
                if let Some(ret_type) = &fun.return_type {
                    ret_type.clone()
                } else {
                    TypeDecl::Unknown
                }
            } else {
                return Err(TypeCheckError::new(format!("Identifier {:?} not found", name)));
            }
        }
        Expr::Null => TypeDecl::Any,
        Expr::ExprList(_) => TypeDecl::Unit,
        Expr::Call(fn_name, _) => {
            if let Some(fun) = ctx.get_fn(fn_name.as_str()) {
                if let Some(ret_type) = &fun.return_type {
                    ret_type.clone()
                } else {
                    TypeDecl::Unknown
                }
            } else {
                return Err(TypeCheckError::new(format!("Function {:?} not found", fn_name)));
            }
        }
        Expr::Return(expr) => {
            if expr.is_none() {
                TypeDecl::Unit
            } else {
                let e = expr.unwrap();
                type_check(ast, e, ctx)?
            }
        }
        Expr::Break => TypeDecl::Unit,
        Expr::Continue => TypeDecl::Unit,
        Expr::Assign(lhs, rhs) => {
            let lhs_ty = type_check(ast, *lhs, ctx)?;
            let rhs_ty = type_check(ast, *rhs, ctx)?;
            if lhs_ty != rhs_ty {
                return Err(TypeCheckError::new(format!("Type mismatch: lhs expected {:?}, but rhs got {:?}", lhs_ty, rhs_ty)));
            }
            lhs_ty
        }
    })
}
pub fn check_block(ast: &ExprPool, e: ExprRef, ctx: &mut TypeCheckContext) -> Result<TypeDecl, TypeCheckError> {
    match ast.0.get(e.0 as usize).unwrap_or(&Expr::Null) {
        Expr::Block(expressions) => {
            if expressions.is_empty() {
                return Ok(TypeDecl::Unit);
            }
            let mut return_types = vec![];
            // This code assumes Block(expression) don't make nested function
            // so `return` expression always return for this context.
            for e in expressions {
                let expr = ast.0.get(e.0 as usize).unwrap();
                let def_ty = match expr {
                    Expr::Return(ret_ty) if ret_ty.is_none() => {
                        TypeDecl::Unit
                    }
                    Expr::Return(ret_ty) if ret_ty.is_some() => {
                        type_check(ast, ret_ty.unwrap(), ctx)?
                    }
                    _ => type_check(ast, *e, ctx)?,
                };
                return_types.push(def_ty);
            }
            return_types.iter().all(|ty| *ty == return_types[0]);
            Ok(return_types[0].clone())
        }
        _ => panic!("check_block: expected block but {:?}", ast.0.get(e.0 as usize).unwrap()),
    }
}