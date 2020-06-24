use frontend;
use frontend::ast::*;
use std::collections::HashMap;
use std::borrow::BorrowMut;
use frontend::ast::Type::UInt64;

pub struct Environment {
    context: HashMap<String, Type>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            context: HashMap::new(),
        }
    }
}

fn norm(t: &Type) -> &Type {
    match t {
        Type::Variable(tvar) => {
            let tvar = tvar.as_ref().borrow_mut();
            if tvar.ty == Type::Unknown {
                return t;
            } else {
                return norm(t);
            }
        }
        t => return t,
    }
}

fn copy_type(t: &Type) -> Type {
    // enum `Type` cannot be implemented `Copy`
    match t {
        Type::Variable(v) => Type::Variable(v.clone()),
        Type::Unknown => Type::Unknown,
        Type::Int64 => Type::Int64,
        Type::UInt64 => Type::UInt64,
        Type::Unit => Type::Unit,
        Type::Bool => Type::Bool
    }
}

fn unify(t1: &mut Type, t2: &mut Type) -> Result<(), String> {
    let t1 = norm(t1);
    let t2 = norm(t2);
    match (t1, t2) {
        (Type::Variable(tv1), Type::Variable(tv2)) => {
            let mut tv1 = tv1.as_ref().borrow_mut();
            let tv2 = tv2.as_ref().borrow_mut();
            if tv1.ty == Type::Unknown && tv2.ty == Type::Unknown && tv1.id != tv2.id {
                tv1.ty = copy_type(&t2);
            }
            Ok(())
        }
        (Type::Variable(tv1), _) => {
            let mut tv1 = tv1.as_ref().borrow_mut();
            if tv1.ty == Type::Unknown {
                tv1.ty = copy_type(&t2);
            }
            Ok(())
        }
        (_, Type::Variable(tv2)) => {
            let mut tv2 = tv2.as_ref().borrow_mut();
            if tv2.ty == Type::Unknown {
                tv2.ty = copy_type(&t1);
            }
            Ok(())
        }
        (Type::Int64, Type::Int64) => Ok(()),
        (Type::UInt64, Type::UInt64) => Ok(()),
        (Type::Bool, Type::Bool) => Ok(()),
        (lhs, rhs) => Err(format!("{:?} {:?} failed", lhs, rhs)),
    }
}

pub fn typing(expr: &mut Expr, env: &mut Environment) -> Result<Type, String> {
    match expr {
        Expr::Binary(x) => {
            let mut x = x.as_ref().borrow_mut();
            let mut t1 = typing(x.lhs.borrow_mut(), env)?;
            let mut t2 = typing(x.rhs.borrow_mut(), env)?;
            let mut ty_op = typing_op(x.op.clone());
            if ty_op == Type::Bool {
                if t1 != Type::Bool || t2 != Type::Bool {
                    return Err(format!("bool op but {:?} {:?}", t1, t2));
                } else {
                    return Ok(Type::Bool);
                }
            } else if ty_op == Type::Int64 {
                unify(&mut t1, &mut t2)?;

                // int64
                let int_res = unify(&mut ty_op, &mut t1);    // int64

                // uint64
                let mut ty_uint = Type::UInt64;
                let uint_res = unify(&mut ty_uint, &mut t1);    // int64

                // check
                if int_res.is_err() || uint_res.is_ok() {
                    // OK
                } else {
                    int_res?;
                    uint_res?;
                }
            } else {
                unify(&mut t1, &mut t2)?;
                unify(&mut ty_op, &mut t1)?;
            }
            Ok(t1)
        }
        Expr::Int64(_) => Ok(Type::Int64),
        Expr::UInt64(_) => Ok(Type::UInt64),
        /*
        Expr::Val(_, _, _) => {},
        Expr::Identifier(_) => {},
        Expr::Null => {},
        Expr::Call(_, _) => {},
         */
        _ => Err(format!("err")),
    }
}

pub fn typing_op(op: Operator) -> Type {
    match op {
        Operator::Assign => Type::Unit,
        Operator::IAdd => Type::Int64,
        Operator::ISub => Type::Int64,
        Operator::IMul => Type::Int64,
        Operator::IDiv => Type::Int64,
        Operator::EQ => Type::Bool,
        Operator::NE => Type::Bool,
        Operator::LT => Type::Bool,
        Operator::LE => Type::Bool,
        Operator::GT => Type::Bool,
        Operator::GE => Type::Bool,
        Operator::LogicalAnd => Type::Bool,
        Operator::LogicalOr => Type::Bool,
    }
}
