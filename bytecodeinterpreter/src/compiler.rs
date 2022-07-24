use std::collections::HashMap;
use frontend;
use frontend::ast::*;

pub enum Code {
    Op(BCode),
    UInt64(u64),
    Int64(i64),
    String(Box<String>),
}

#[allow(non_camel_case_types)]
#[derive(Debug, PartialEq)]
pub enum BCode {
    OP_NOP,
    OP_PUSH_NULL,
    OP_PUSH_INT(i64),
    OP_PUSH_UINT(u64),
    OP_PUSH_IDENT(u32),  // push(variable['ident'])
    OP_PUSH_CONST(u32),  // push(value['ident'])

    OP_LOAD_IDENT(u32),  // stack.push(variable[x])  variable or const val

    OP_ADD,
    OP_SUB,
    OP_MUL,
    OP_DIV,
}

pub struct Compiler {
    codes: Vec<BCode>,
    names: HashMap<String, u32>,
}

// byte code compiler
impl Compiler {
    pub fn new() -> Self {
        Compiler {
            codes: Vec::new(),
            names: HashMap::new(),
        }
    }

    // TODO: Change 2-pass or more pass compiler

    pub fn compile(&mut self, expr: &Expr) -> Vec<BCode> {
        match expr {
            Expr::IfElse(_, _, _) => (),
            Expr::Binary(bop) => {
                let mut codes = Vec::new();
                let mut lhs = self.compile(&bop.lhs);
                codes.append(&mut lhs);
                let mut rhs = self.compile(&bop.rhs);
                codes.append(&mut rhs);

                match bop.op {
                    Operator::IAdd => codes.push(BCode::OP_ADD),
                    Operator::ISub => codes.push(BCode::OP_SUB),
                    Operator::IMul => codes.push(BCode::OP_MUL),
                    Operator::IDiv => codes.push(BCode::OP_DIV),
                    _ => panic!("not implemented yet (Binary Operator)"),
                }
                return codes;
            }
            Expr::Int64(i) => return vec![BCode::OP_PUSH_INT(*i)],
            Expr::UInt64(u) => return vec![BCode::OP_PUSH_UINT(*u)],
            Expr::Int(_) => return vec![BCode::OP_PUSH_INT(0xDEADBEEF)], // TODO: implement
            Expr::Identifier(name) => {
                let id = self.names.get(name);
                if id.is_none() {
                    panic!("error, variable/constant name is invalid: `{}`", name);
                }
                let id = id.unwrap() as &u32;
                return vec![BCode::OP_LOAD_IDENT(*id)];
            }
            Expr::Call(_, _) => (),
            Expr::Null => return vec![BCode::OP_PUSH_NULL],
            Expr::Val(name, _ty, expr) => {
                match expr {
                    Some(expr) => {
                        let id = self.names.get(name);
                        if id.is_some() {
                            panic!("already defined constant `{}`", name)
                        }
                        let id = self.names.len() as u32;
                        self.names.insert(name.clone(), id);

                        let mut inst: Vec<BCode> = vec![BCode::OP_PUSH_CONST(id)];
                        let mut val = self.compile(expr);
                        inst.append(&mut val);
                        return inst;
                    }
                    _ => panic!("value is not set: {}", name), // error
                }
            }
        }
        return vec![];    // TODO
    }
}
