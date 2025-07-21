use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;

#[derive(Debug, Clone)]
pub struct VariableValue {
    pub value: RcObject,
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct Environment {
    var: Vec<HashMap<DefaultSymbol, VariableValue>>,
}

#[derive(Eq, PartialEq)]
pub enum VariableSetType {
    Insert,
    Overwrite,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            var: vec![HashMap::new()],
        }
    }

    pub fn enter_block(&mut self) {
        self.var.push(HashMap::new());
    }

    pub fn exit_block(&mut self) {
        self.var.pop();
    }

    pub fn set_val(&mut self, name: DefaultSymbol, value: RcObject) {
        if let Some(last) = self.var.last_mut() {
            last.insert(name,
                        VariableValue{
                            mutable: false,
                            value
                        });
        }
    }

    pub fn set_var(&mut self, name: DefaultSymbol, value: RcObject, set_type: VariableSetType, string_interner: &DefaultStringInterner) -> Result<(), InterpreterError> {
        let current = self.var.iter_mut().rfind(|v| v.contains_key(&name));

        if current.is_none() || set_type == VariableSetType::Insert {
            // Insert new value
            let val = VariableValue{ mutable: true, value };
            if let Some(last) = self.var.last_mut() {
                last.insert(name, val);
            }
        } else {
            if let Some(current) = current {
                // Overwrite variable
                if let Some(entry) = current.get_mut(&name) {
                    if !entry.mutable {
                        let name = string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
                        return Err(InterpreterError::ImmutableAssignment(format!("Variable {} already defined as immutable (val)", name)));
                    }

                    entry.value = value;
                }
            }
        }

        Ok(())
    }

    pub fn get_val(&self, name: DefaultSymbol) -> Option<Rc<RefCell<Object>>> {
        for v in self.var.iter().rev() {
            if let Some(val) = v.get(&name) {
                return Some(val.value.clone());
            }
        }
        None
    }
}
