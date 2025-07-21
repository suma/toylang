use std::collections::HashMap;
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::Function;
use crate::type_decl::TypeDecl;

#[derive(Debug)]
pub struct VarState {
    pub ty: TypeDecl,
}

#[derive(Debug)]
pub struct TypeCheckContext {
    pub vars: Vec<HashMap<DefaultSymbol, VarState>>,
    pub functions: HashMap<DefaultSymbol, Rc<Function>>,
}

impl TypeCheckContext {
    pub fn new() -> Self {
        Self {
            vars: vec![HashMap::new()],
            functions: HashMap::new(),
        }
    }

    pub fn set_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty });
    }

    pub fn set_mutable_var(&mut self, name: DefaultSymbol, ty: TypeDecl) {
        let last = self.vars.last_mut().expect("Variable stack should not be empty");
        last.insert(name, VarState { ty });
    }

    pub fn set_fn(&mut self, name: DefaultSymbol, f: Rc<Function>) {
        self.functions.insert(name, f);
    }

    pub fn get_var(&self, name: DefaultSymbol) -> Option<TypeDecl> {
        for v in self.vars.iter().rev() {
            let v_val = v.get(&name);
            if let Some(val) = v_val {
                return Some(val.ty.clone());
            }
        }
        None
    }

    pub fn get_fn(&self, name: DefaultSymbol) -> Option<Rc<Function>> {
        if let Some(val) = self.functions.get(&name) {
            Some(val.clone())
        } else {
            None
        }
    }

    pub fn update_var_type(&mut self, name: DefaultSymbol, new_ty: TypeDecl) -> bool {
        for v in self.vars.iter_mut().rev() {
            if let Some(var_state) = v.get_mut(&name) {
                var_state.ty = new_ty;
                return true;
            }
        }
        false
    }

    pub fn push_scope(&mut self) {
        self.vars.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.vars.pop();
    }
}