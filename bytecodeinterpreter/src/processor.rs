use std::collections::HashMap;
use crate::compiler::*;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Object {
    UInt64(u64),
    Int64(i64),
    Ident(u32),
    Null,
}

#[derive(Debug)]
pub struct Processor {
    program: Vec<BCode>,
    stack: Vec<Object>,
    var: HashMap<u32, Object>,
    val: HashMap<u32, Object>,
    pos: usize,
}

// Stack machine interpreter
impl Processor {
    pub fn new() -> Self {
        Processor {
            program: Vec::new(),
            stack: Vec::new(),
            var: HashMap::new(),
            val: HashMap::new(),
            pos: 0,
        }
    }

    pub fn append(&mut self, mut codes: Vec<BCode>) -> u64 {
        self.program.append(&mut codes);
        return self.evaluate();
    }

    pub fn evaluate(&mut self) -> u64 {
        let mut i = self.pos;
        let plen = self.program.len();
        loop {
            if i >= plen {
                break;
            }
            let code: &BCode = &self.program[i];
            match code {
                BCode::NOP => i += 1,
                BCode::PUSH_NULL => {
                    self.stack.push(Object::Null);
                    i += 1;
                }
                BCode::PUSH_INT(int) => {
                    self.stack.push(Object::Int64(*int));
                    i += 1;
                }
                BCode::PUSH_UINT(u) => {
                    self.stack.push(Object::UInt64(*u));
                    i += 1;
                }
                BCode::PUSH_CONST(id) => {
                    let top = self.stack.pop().unwrap();
                    self.val.insert(*id, top);
                    i += 1;
                }
                BCode::LOAD_IDENT(id) => {
                    let value = self.stack.pop().unwrap();
                    self.var.insert(*id, value);
                    i += 1;
                }
                BCode::LOAD_CONST(id) => {
                    let value = self.stack.pop().unwrap();
                    self.val.insert(*id, value);
                    i += 1;
                }
                BCode::LOAD_IDENT_VAR(id) => {
                    let v = self.var.get(&id);
                    match v {
                        Some(v) => self.stack.push(*v),
                        _ => panic!("LOAD IDENT var"),
                    };
                    i += 1;
                }
                BCode::LOAD_IDENT_CONST(id) => {
                    let v = self.val.get(&id);
                    match v {
                        Some(v) => self.stack.push(*v),
                        _ => panic!("LOAD IDENT val"),
                    };
                    i += 1;
                }

                BCode::PRINT0 => {
                    let top = self.stack.pop();
                    match top {
                        Some(Object::UInt64(u)) => println!("{} (u64)", u),
                        Some(Object::Int64(int)) => println!("{} (i64)", int),
                        Some(Object::Ident(id)) => {
                            // TODO: identify id for const(val) or variable
                            let val = self.val.get(&id);
                            match val {
                                Some(Object::UInt64(u)) => println!("val {} (u64)", u),
                                Some(Object::Int64(int)) => println!("val {} (i64)", int),
                                Some(Object::Null) => println!("Null"),
                                x => println!("{:?} const", x),
                            }
                        }
                        x => todo!("PRINT (not implemented yet) : {:?}", x),
                    }
                    i += 1;
                }

                BCode::BINARY_ADD => {
                    let lhs = self.stack.pop();
                    let rhs = self.stack.pop();
                    if lhs.is_none() || rhs.is_none() {
                        panic!("BINARY_ADD: Stack is empty")
                    }
                    match (lhs.unwrap(), rhs.unwrap()) {
                        (Object::UInt64(lhs), Object::UInt64(rhs)) => {
                            self.stack.push(Object::UInt64(lhs + rhs));
                            i += 1;
                        }
                        (Object::Int64(lhs), Object::Int64(rhs)) => {
                            self.stack.push(Object::Int64(lhs + rhs));
                            i += 1;
                        }
                        _ => panic!("Binary ADD operator found non integer object"),
                    }
                }
                x => {
                    panic!("not implemented yet: {:?}", x)
                }
                //BCode::BINARY_SUB => {}
                //BCode::BINARY_MUL => {}
                //BCode::BINARY_DIV => {}
            }
        }

        self.pos = i;
        return 0;
    }
}