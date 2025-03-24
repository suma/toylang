#![feature(box_patterns)]

use std::collections::HashMap;
use std::rc::Rc;
use frontend;
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::type_decl::TypeDecl;

fn main() {
    let args = std::env::args().collect::<Vec<String>>();
    if args.len() != 2 {
        println!("Usage: {} <file>", args[0]);
        return;
    }
    let file = std::fs::read_to_string(args[1].clone()).expect("Failed to read file");
    let mut parser = frontend::Parser::new(file.as_str());
    let program = parser.parse_program();
    if program.is_err() {
        println!("parser_program failed {:?}", program.unwrap_err());
        return;
    }

    let mut program = program.unwrap();
    let mut ctx = TypeCheckContext::new();
    let mut kill_switch = false;
    let mut main: Option<Rc<Function>> = None;
    program.function.iter().for_each(|func| {
        let r = type_check(&program.expression, func.code, &mut ctx);
        if r.is_err() {
            eprintln!("type_check failed in {}: {}", func.name, r.unwrap_err());
            kill_switch = true;
        }
        if func.name == "main" && func.parameter.is_empty() {
            main = Some(func.clone());
        }
    });
    if !kill_switch && main.is_some() {
        let mut env = Environment::new();
        let res = evaluate_main(main.unwrap(), &mut program.expression, &mut env);
        println!("Result: {:?}", res);
        return;
    } else {
        println!("Program didn't run");
    }
}

#[derive(Debug, Clone)]
pub struct Environment {
    // mutable = true, immutable = false
    var: HashMap<String, (bool, Object)>,
    super_context: Option<Box<Environment>>,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            var: HashMap::new(),
            super_context: None,
        }
    }

    pub fn new_block(&self) -> Self {
        Self {
            var: HashMap::new(),
            super_context: Some(Box::new(self.clone())),
        }
    }

    pub fn set_val(&mut self, name: &str, value: &Object) {
        if self.var.contains_key(name) {
            panic!("Variable {} already defined (val)", name);
        }
        let value = value.clone();
        self.var.insert(name.to_string(), (false, value));
    }

    pub fn set_var(&mut self, name: &str, value: &Object) {
        if self.var.contains_key(name) {
            panic!("Variable {} already defined (var)", name);
        }
        let exist = self.var.get(name);
        if exist.is_some() {
            // Check type of variable
            let exist = exist.unwrap();
            let ty = value.get_type();
            if exist.1.get_type() != ty {
                panic!("Variable {} already defined with different type (var)", name);
            }
        }
        let value = value.clone();
        self.var.insert(name.to_string(), (true, value));
    }

    pub fn get_val(&self, name: &str) -> Option<&Object> {
        let v_val = self.var.get(name);
        if v_val.is_some() {
            return Some(&v_val.unwrap().1);
        } else if self.super_context.is_some() {
            if let Some(v) = self.super_context.as_ref() {
                return v.get_val(name);
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Object {
    Bool(bool),
    Int64(i64),
    UInt64(u64),
    String(String),
    //Array: Vec<Object>,
    //Function: Rc<Function>,
    Null,
    Unit,
}

impl Object {
    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::Unit => TypeDecl::Unit,
            Object::Null => TypeDecl::Any,
            Object::Bool(_) => TypeDecl::Bool,
            Object::UInt64(_) => TypeDecl::UInt64,
            Object::Int64(_) => TypeDecl::Int64,
            Object::String(_) => TypeDecl::String,
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            Object::Null => true,
            _ => false,
        }
    }

    pub fn is_unit(&self) -> bool {
        match self {
            Object::Unit => true,
            _ => false,
        }
    }
}

fn evaluate(e: &ExprRef, ast: &ExprPool, ctx: &mut Environment) -> Result<Object, String> {
    let expr = ast.get(e.0 as usize);
    match expr {
        Some(Expr::Binary(op, lhs, rhs)) => {
            let bool_op = vec![Operator::EQ, Operator::NE, Operator::LT, Operator::LE, Operator::GT, Operator::GE];
            if bool_op.contains(op) {
                panic!("Not handled yet logical operation: {:?}", expr);
            }
            let lhs = evaluate(lhs, ast, ctx)?;
            let rhs = evaluate(rhs, ast, ctx)?;
            let lhs_ty = lhs.get_type();
            let rhs_ty = rhs.get_type();
            if lhs_ty != rhs_ty {
                panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr);
            }
            let res = match op { // Int64, UInt64 only now
                Operator::IAdd => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l + r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l + r),
                        _ => panic!("evaluate: Bad types for binary '+' operation due to different type: {:?}", expr),
                    }
                }
                Operator::ISub => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l - r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l - r),
                        _ => panic!("evaluate: Bad types for binary '-' operation due to different type: {:?}", expr),
                    }
                }
                Operator::IMul => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l * r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l * r),
                        _ => panic!("evaluate: Bad types for binary '*' operation due to different type: {:?}", expr),
                    }
                }
                Operator::IDiv => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l / r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l / r),
                        _ => panic!("evaluate: Bad types for binary '/' operation due to different type: {:?}", expr),
                    }
                }
                Operator::EQ => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l == r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l == r),
                        (Object::String(l), Object::String(r)) => Object::Bool(l == r),
                        _ => panic!("evaluate: Bad types for binary '==' operation due to different type: {:?}", expr),
                    }
                }
                Operator::NE => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l != r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l != r),
                        (Object::String(l), Object::String(r)) => Object::Bool(l != r),
                        _ => panic!("evaluate: Bad types for binary '!=' operation due to different type: {:?}", expr),
                    }
                }
                Operator::GE => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l >= r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l >= r),
                        _ => panic!("evaluate: Bad types for binary '>=' operation due to different type: {:?}", expr),
                    }
                }
                Operator::GT => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l > r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l > r),
                        _ => panic!("evaluate: Bad types for binary '>' operation due to different type: {:?}", expr),
                    }
                }
                Operator::LE => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l <= r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l <= r),
                        _ => panic!("evaluate: Bad types for binary '<=' operation due to different type: {:?}", expr),
                    }
                }
                Operator::LT => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Bool(l < r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::Bool(l < r),
                        _ => panic!("evaluate: Bad types for binary '<' operation due to different type: {:?}", expr),
                    }
                }
                Operator::LogicalAnd => {
                    match (lhs, rhs) {
                        (Object::Bool(l), Object::Bool(r)) => Object::Bool(l && r),
                        _ => panic!("evaluate: Bad types for binary '&&' operation due to different type: {:?}", expr),
                    }
                }
                Operator::LogicalOr => {
                    match (lhs, rhs) {
                        (Object::Bool(l), Object::Bool(r)) => Object::Bool(l || r),
                        _ => panic!("evaluate: Bad types for binary '||' operation due to different type: {:?}", expr),
                    }
                }
            };
            Ok(res)
        }
        Some(Expr::Int64(_)) | Some(Expr::UInt64(_)) | Some(Expr::String(_)) | Some(Expr::True) | Some(Expr::False) => {
            Ok(convert_object(expr))
        }
        Some(Expr::Return(e)) => {
            Ok(evaluate(&e.unwrap(), ast, ctx)?)
        }
        Some(Expr::Identifier(s)) => {
            Ok(ctx.get_val(s.as_ref()).unwrap().clone())
        }
        Some(Expr::IfElse(cond, then, _else)) => {
            let cond = evaluate(cond, ast, ctx)?;
            if cond.get_type() != TypeDecl::Bool {
                panic!("evaluate: Bad types for if-else due to different type: {:?}", expr);
            }
            assert!(ast.get(then.0 as usize).unwrap().is_block(), "evaluate: then is not block");
            assert!(ast.get(_else.0 as usize).unwrap().is_block(), "evaluate: else is not block");
            let mut ctx = ctx.new_block();
            if let Object::Bool(true) = cond {
                let then = evaluate_block(then, ast, &mut ctx)?;
                Ok(then)
            } else {
                let _else = evaluate_block(_else, ast, &mut ctx)?;
                Ok(_else)
            }
        }

        Some(Expr::Block(_)) => {
            let mut ctx = ctx.new_block();
            Ok(evaluate_block(e, ast, &mut ctx)?)
        }

        _ => panic!("evaluate: Not handled yet {:?}", expr),
    }
}

fn evaluate_block(blk_expr: &ExprRef, ast: &ExprPool, ctx: &mut Environment) -> Result<Object, String> {
    let to_expr = |e: &ExprRef| -> &Expr { ast.get(e.0 as usize).unwrap_or(&Expr::Null) };
    assert!(to_expr(blk_expr).is_block(), "failed: block expected but got {:?}", to_expr(blk_expr));

    let mut last: Option<Object> = Some(Object::Unit);
    match to_expr(blk_expr) {
        Expr::Block(expressions) => {
            for e in expressions {
                let expr = to_expr(e);
                match expr {
                    Expr::Val(name, _, e) => {
                        let name = name.clone();
                        let value = &evaluate(e, ast, ctx)?;
                        ctx.set_val(name.as_ref(), value);
                        last = Some(Object::Unit);
                    }
                    Expr::Var(name, _, e) => {
                        let value = if e.is_none() {
                            Object::Null
                        } else {
                            evaluate(&e.unwrap(), ast, ctx)?
                        };
                        ctx.set_var(name.as_ref(), &value);
                    }
                    Expr::Return(e) => {
                        if e.is_none() {
                            return Ok(Object::Unit);
                        }
                        return Ok(evaluate(&e.unwrap(), ast, ctx)?);
                    }
                    // TODO: break, continue
                    Expr::Int64(_) | Expr::UInt64(_) | Expr::String(_) => {
                        last = Some(convert_object(Some(expr)));
                    }
                    Expr::Identifier(s) => {
                        let obj = ctx.get_val(s.as_ref());
                        if obj.is_none() || obj.clone().unwrap().is_null() {
                            panic!("evaluate_block: Identifier {} is null", s);
                        }
                        last = Some(obj.unwrap().clone());
                    }
                    Expr::Block(_) => {
                        let mut ctx = ctx.new_block();
                        last = Some(evaluate_block(blk_expr, ast, &mut ctx)?);
                    }
                    _ => {
                        last = Some(evaluate(e, ast, ctx)?);
                    }
                }
            }
        }
        _ => {
            last = Some(evaluate(blk_expr, ast, ctx)?);
            println!("evaluate_block: Not handled yet {:?}", to_expr(blk_expr))
        }
    }
    Ok(last.unwrap())
}

fn evaluate_main(function: Rc<Function>, ast: &ExprPool, ctx: &mut Environment) -> Result<Object, String> {
    let res = evaluate_block(&function.code, ast, ctx)?;
    if function.return_type.is_none() || function.return_type.clone().unwrap() == TypeDecl::Unit {
        Ok(Object::Unit)
    } else {
        Ok(res)
    }
}

fn convert_object(e: Option<&Expr>) -> Object {
    match e {
        Some(Expr::True) => Object::Bool(true),
        Some(Expr::False) => Object::Bool(false),
        Some(Expr::Int64(v)) => Object::Int64(*v),
        Some(Expr::UInt64(v)) => Object::UInt64(*v),
        Some(Expr::String(v)) => Object::String(v.clone()),
        _ => panic!("Not handled yet {:?}", e),
    }
}

