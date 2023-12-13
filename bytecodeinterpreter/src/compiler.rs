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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BCode {
    NOP,
    PUSH_NULL,
    PUSH_INT(i64),
    PUSH_UINT(u64),

    PUSH_CONST(u32),

    LOAD_IDENT(u32),  // push(variable['ident'])
    LOAD_CONST(u32),  // push(value['ident'])

    LOAD_IDENT_VAR(u32),  // stack.push(variable[x])  variable or const val
    LOAD_IDENT_CONST(u32),
    //STORE_GLOBAL(u32, ,
    //STORE_LOCAL_VAR,
    //STORE_LOCAL_CONST,

    BINARY_ADD,
    BINARY_SUB,
    BINARY_MUL,
    BINARY_DIV,

    PRINT0,
    PRINT,
}

pub enum SymbolType {
    Global,
    Argument,
    Local,
}

pub struct Symbol {
    kind: SymbolType,
    pos : u32,
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

    pub fn get_program(&mut self) -> &Vec<BCode> {
        return &self.codes;
    }

    pub fn compile_code(&mut self, expr: &Expr) {
        self.codes = self.compile(expr);
    }

    pub fn append(&mut self, expr: &Expr) {
        let mut codes = self.compile(expr);
        self.codes.append(&mut codes);
    }

    pub fn compile(&mut self, expr: &Expr) -> Vec<BCode> {
        let print_string0 = "print0".to_string();
        let print_string = "print".to_string();

        let codes: Vec<BCode> = match expr {
            Expr::IfElse(expr, thenBlock, elseBlock) => {
                let mut codes = self.compile(&expr);
                //let mut then_codes = self.compile(thenBlock);
                //let mut else_codes = self.compile(elseBlock);
                //codes.append(&mut then_codes);
                //codes.append(&mut else_codes);
                codes
            }
            Expr::Binary(bop) => {
                let mut codes = Vec::new();
                let mut lhs = self.compile(&bop.lhs);
                codes.append(&mut lhs);
                let mut rhs = self.compile(&bop.rhs);
                codes.append(&mut rhs);

                match bop.op {
                    Operator::IAdd => codes.push(BCode::BINARY_ADD),
                    Operator::ISub => codes.push(BCode::BINARY_SUB),
                    Operator::IMul => codes.push(BCode::BINARY_MUL),
                    Operator::IDiv => codes.push(BCode::BINARY_DIV),
                    // TODO: assign
                    _ => panic!("not implemented yet (Binary Operator)"),
                }
                codes
            }
            Expr::Int64(i) => vec![BCode::PUSH_INT(*i)],
            Expr::UInt64(u) => vec![BCode::PUSH_UINT(*u)],
            Expr::Int(i) => {
                // TODO: support multiple-precision integer
                let i = i.parse::<i64>().unwrap_or_else(|_| 0i64);
                vec![BCode::PUSH_INT(i)]
            }
            Expr::Identifier(name) => {
                let id = self.names.get(name);
                if id.is_none() {
                    panic!("error, variable/constant name is invalid: `{}`", name);
                }
                let id = id.unwrap() as &u32;
                vec![BCode::LOAD_IDENT_CONST(*id)]   // TODO(suma): Use env
            }
            Expr::Call(print_string0, _) => {
                vec![BCode::PRINT0]
            }
            Expr::Call(print_string, a) => {
                let mut codes: Vec<BCode> = vec![];
                for e in a {
                    let mut res = self.compile(&e);
                    codes.append(&mut res);
                }
                vec![BCode::PRINT]
            }
            Expr::Block(b) => {
                let mut codes: Vec<BCode> = vec![];
                for e in b {
                    let mut res: Vec<BCode> = self.compile(&e);
                    codes.append(&mut res);
                }
                codes
            }
            Expr::Null => vec![BCode::PUSH_NULL],
            Expr::Val(name, _ty, expr) => {
                match expr {
                    Some(expr) => {
                        let id = self.names.get(name);
                        if id.is_some() {
                            panic!("already defined constant `{}`", name)
                        }
                        let id = self.names.len() as u32;
                        self.names.insert(name.clone(), id);

                        let mut inst: Vec<BCode> = vec![BCode::PUSH_CONST(id)];
                        let mut val = self.compile(expr);
                        val.append(&mut inst);
                        val
                    }
                    _ => panic!("value is not set: {}", name), // error
                }
            }
        };

        return codes;
    }
    //self.codes.append(&mut codes);
}
