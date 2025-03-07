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

pub fn type_check(ast: &ExprPool, e: ExprRef, ctx: &mut TypeCheckContext) -> Result<TypeDecl, TypeCheckError> {
    Ok(match ast.0.get(e.0 as usize).unwrap_or(&Expr::Null) {
        Expr::IfElse(_cond, _blk1, _blk2) => TypeDecl::Unit,
        Expr::Binary(_cond, _blk1, _blk2) => TypeDecl::Bool,
        Expr::Block(expressions) => {
            let mut ty = TypeDecl::Unit;
            for e in expressions {
                ty = type_check(ast, *e, ctx)?;
            }
            ty
        }
        Expr::Int64(_) => TypeDecl::Int64,
        Expr::UInt64(_) => TypeDecl::UInt64,
        Expr::String(_) => TypeDecl::String,
        Expr::Val(name, type_decl, _expr) => {
            if let Some(val_type) = type_decl {
                ctx.set_var(name.as_str(), val_type.clone());
                TypeDecl::Unit
            } else {
                return Err(TypeCheckError::new(format!("Type declaration {} not found", name)));
            }
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
    })
}