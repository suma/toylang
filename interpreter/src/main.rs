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

#[derive(Debug)]
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
        }
        None
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Object {
    Int64(i64),
    UInt64(u64),
    String(String),
    //Array: Vec<Object>,
    //Function: Rc<Function>,
}

impl Object {
    pub fn get_type(&self) -> TypeDecl {
        match self {
            Object::UInt64(_) => TypeDecl::UInt64,
            Object::Int64(_) => TypeDecl::Int64,
            Object::String(_) => TypeDecl::String,
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
                        _ => panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr),
                    }
                }
                Operator::ISub => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l - r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l - r),
                        _ => panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr),
                    }
                }
                Operator::IMul => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l * r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l * r),
                        _ => panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr),
                    }
                }
                Operator::IDiv => {
                    match (lhs, rhs) {
                        (Object::Int64(l), Object::Int64(r)) => Object::Int64(l / r),
                        (Object::UInt64(l), Object::UInt64(r)) => Object::UInt64(l / r),
                        _ => panic!("evaluate: Bad types for binary operation due to different type: {:?}", expr),
                    }
                }
                // TODO: implement LogicalAnd, LogicalOr
                _ => panic!("evaluate: not implemented {:?}", op),
            };
            Ok(res)
        }
        Some(Expr::Int64(_)) | Some(Expr::UInt64(_)) | Some(Expr::String(_)) => {
            Ok(convert_object(expr))
        }
        Some(Expr::Return(e)) => {
            Ok(evaluate(&e.unwrap(), ast, ctx)?)
        }
        Some(Expr::Identifier(s)) => {
            Ok(ctx.get_val(s.as_ref()).unwrap().clone())
        }

        _ => panic!("evaluate: Not implemented yet {:?}", expr),
    }
}

fn evaluate_main(function: Rc<Function>, ast: &ExprPool, ctx: &mut Environment) -> Result<Object, String> {
    let code = ast.get(function.code.0 as usize);
    let mut last: Option<Object> = None;
    match code {
        Some(Expr::Block(expressions)) => {
            expressions.iter().for_each(|e| {
                let expr = ast.get(e.0 as usize);
                match expr {
                    // Type has already checked
                    Some(Expr::Val(name, _, e)) => {
                        let e = ast.get(e.0 as usize);
                        let value = convert_object(e);
                        ctx.set_val(name.as_ref(), &value);
                    }
                    Some(Expr::Var(name, _, e)) => {
                        //let e = ast.get(e.unwrap().0 as usize);
                        let value = evaluate(&e.unwrap(), ast, ctx).unwrap();
                        // TODO: var can be set without assigned expression so `e` can be None
                        ctx.set_var(name.as_ref(), &value);
                    }
                    Some(Expr::Int64(_)) | Some(Expr::UInt64(_)) | Some(Expr::String(_)) => {
                        last = Some(convert_object(expr));
                    }
                    Some(Expr::Return(e)) => {
                        last = Some(evaluate(&e.unwrap(), ast, ctx).unwrap());
                        return;
                    }
                    Some(Expr::Identifier(s)) => {
                        last = Some(ctx.get_val(s.as_ref()).unwrap().clone());
                    }

                    // TODO: implement here
                    None | Some(_) => panic!("evaluate: Not handled yet {:?}", expr),
                }
            });

            // Result
            let res = last.unwrap();
            Ok(res)
        }
        Some(_) | None => Err("Invalid code".to_string()),
    }
}

fn convert_object(e: Option<&Expr>) -> Object {
    match e {
        Some(Expr::Int64(v)) => Object::Int64(*v),
        Some(Expr::UInt64(v)) => Object::UInt64(*v),
        Some(Expr::String(v)) => Object::String(v.clone()),
        _ => panic!("Not handled yet {:?}", e),
    }
}

