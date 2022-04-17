use std::collections::HashMap;
use frontend;
use frontend::ast::*;

pub struct Processor {
    environment: Environment,
}

pub struct Environment {
    pub context: HashMap<String, i64>,  // TODO: type of value
    // TODO: nested scope
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            context: HashMap::new(),
        }
    }
}
impl Processor {
    pub fn new() -> Self {
        Processor {
            environment: Environment::new(),
        }
    }

    pub fn evaluate(&mut self, expr: &Expr) -> i64 {
        match expr {
            Expr::IfElse(_, _, _) => (),
            Expr::Binary(bop) => {
                let lhs = self.evaluate(&bop.lhs);
                let rhs = self.evaluate(&bop.rhs);
                let res = match bop.op {
                    Operator::IAdd => lhs + rhs,
                    Operator::ISub => lhs - rhs,
                    Operator::IMul => lhs * rhs,
                    Operator::IDiv => lhs / rhs,
                    _ => panic!("not implemented yet (Binary Operator)"),
                };
                return res;
            }
            Expr::Int64(i) => return *i,
            Expr::UInt64(u) => return *u as i64,
            Expr::Int(i_str) => return 0,
            Expr::Identifier(name) => {
                match self.environment.context.get(name) {
                    Some(v) => return *v,
                    _ => return 0, // error
                }
            }
            Expr::Call(_, _) => (),
            Expr::Null => (),
            Expr::Val(name, _ty, expr) => {
                match expr {
                    Some(expr) => {
                        let eval = self.evaluate(expr);
                        self.environment.context.insert(name.to_string(), eval);
                        return 0;
                    }
                    _ => panic!("value is not set: {}", name), // error
                }
            }
        }
        return 0i64;    // TODO
    }
}
