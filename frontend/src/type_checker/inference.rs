use std::collections::HashMap;
use string_interner::DefaultSymbol;
use crate::ast::ExprRef;
use crate::type_decl::TypeDecl;

#[derive(Debug)]
pub struct TypeInferenceState {
    pub type_hint: Option<TypeDecl>,
    pub number_usage_context: Vec<(ExprRef, TypeDecl)>,
    pub variable_expr_mapping: HashMap<DefaultSymbol, ExprRef>,
}

impl TypeInferenceState {
    pub fn new() -> Self {
        Self {
            type_hint: None,
            number_usage_context: Vec::new(),
            variable_expr_mapping: HashMap::new(),
        }
    }

    pub fn set_type_hint(&mut self, hint: Option<TypeDecl>) {
        self.type_hint = hint;
    }

    pub fn get_type_hint(&self) -> Option<TypeDecl> {
        self.type_hint.clone()
    }

    pub fn add_number_context(&mut self, expr_ref: ExprRef, type_decl: TypeDecl) {
        self.number_usage_context.push((expr_ref, type_decl));
    }

    pub fn map_variable(&mut self, name: DefaultSymbol, expr_ref: ExprRef) {
        self.variable_expr_mapping.insert(name, expr_ref);
    }

    pub fn get_variable_expr(&self, name: DefaultSymbol) -> Option<ExprRef> {
        self.variable_expr_mapping.get(&name).copied()
    }

    pub fn clear(&mut self) {
        self.type_hint = None;
        self.number_usage_context.clear();
        self.variable_expr_mapping.clear();
    }
}