use std::collections::HashMap;
use string_interner::DefaultSymbol;
use crate::type_decl::TypeDecl;

#[derive(Debug)]
pub struct FunctionCheckingState {
    pub call_depth: usize,
    pub is_checked_fn: HashMap<DefaultSymbol, Option<TypeDecl>>,
}

impl FunctionCheckingState {
    pub fn new() -> Self {
        Self {
            call_depth: 0,
            is_checked_fn: HashMap::new(),
        }
    }

    pub fn enter_function(&mut self) {
        self.call_depth += 1;
    }

    pub fn exit_function(&mut self) {
        if self.call_depth > 0 {
            self.call_depth -= 1;
        }
    }

    pub fn mark_function_checked(&mut self, name: DefaultSymbol, return_type: Option<TypeDecl>) {
        self.is_checked_fn.insert(name, return_type);
    }

    pub fn is_function_checked(&self, name: DefaultSymbol) -> bool {
        self.is_checked_fn.contains_key(&name)
    }

    pub fn get_function_return_type(&self, name: DefaultSymbol) -> Option<TypeDecl> {
        self.is_checked_fn.get(&name).and_then(|t| t.clone())
    }

    pub fn get_call_depth(&self) -> usize {
        self.call_depth
    }

    pub fn clear(&mut self) {
        self.call_depth = 0;
        self.is_checked_fn.clear();
    }
}