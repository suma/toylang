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
pub struct ModuleEnvironment {
    pub name: Vec<DefaultSymbol>,  // Module path: [math, basic]
    pub variables: HashMap<DefaultSymbol, VariableValue>,
    pub functions: HashMap<DefaultSymbol, frontend::ast::Function>,
}

impl ModuleEnvironment {
    pub fn new(name: Vec<DefaultSymbol>) -> Self {
        Self {
            name,
            variables: HashMap::new(),
            functions: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Environment {
    var: Vec<HashMap<DefaultSymbol, VariableValue>>,
    pub modules: HashMap<Vec<DefaultSymbol>, ModuleEnvironment>,  // Module registry
    pub current_module: Option<Vec<DefaultSymbol>>,               // Current module path
}

#[derive(Eq, PartialEq)]
pub enum VariableSetType {
    Insert,
    Overwrite,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        Self {
            var: vec![HashMap::new()],
            modules: HashMap::new(),
            current_module: None,
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
        } else if let Some(current) = current {
            // Overwrite variable
            if let Some(entry) = current.get_mut(&name) {
                if !entry.mutable {
                    let name = string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
                    return Err(InterpreterError::ImmutableAssignment(format!("Variable {name} already defined as immutable (val)")));
                }

                entry.value = value;
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

    // Module management methods
    pub fn register_module(&mut self, module_path: Vec<DefaultSymbol>) {
        let module_env = ModuleEnvironment::new(module_path.clone());
        self.modules.insert(module_path, module_env);
    }

    pub fn set_current_module(&mut self, module_path: Option<Vec<DefaultSymbol>>) {
        self.current_module = module_path;
    }

    pub fn get_current_module(&self) -> Option<&Vec<DefaultSymbol>> {
        self.current_module.as_ref()
    }

    pub fn get_module(&self, module_path: &[DefaultSymbol]) -> Option<&ModuleEnvironment> {
        self.modules.get(module_path)
    }

    pub fn get_module_mut(&mut self, module_path: &[DefaultSymbol]) -> Option<&mut ModuleEnvironment> {
        self.modules.get_mut(module_path)
    }

    /// Resolve qualified name (e.g., math.add) to find function or variable
    pub fn resolve_qualified_name(&self, module_path: &[DefaultSymbol], name: DefaultSymbol) -> Option<&VariableValue> {
        if let Some(module) = self.get_module(module_path) {
            module.variables.get(&name)
        } else {
            None
        }
    }

    /// Set variable in a specific module
    pub fn set_module_variable(&mut self, module_path: &[DefaultSymbol], name: DefaultSymbol, value: VariableValue) -> Result<(), InterpreterError> {
        if let Some(module) = self.get_module_mut(module_path) {
            module.variables.insert(name, value);
            Ok(())
        } else {
            Err(InterpreterError::InternalError(format!("Module {:?} not found", module_path)))
        }
    }
}
