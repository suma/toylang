use std::collections::HashMap;
use crate::ast::ExprRef;
use crate::type_decl::TypeDecl;

#[derive(Debug)]
pub struct PerformanceOptimization {
    pub type_cache: HashMap<ExprRef, TypeDecl>,
}

impl PerformanceOptimization {
    pub fn new() -> Self {
        Self {
            type_cache: HashMap::new(),
        }
    }

    pub fn cache_type(&mut self, expr_ref: ExprRef, type_decl: TypeDecl) {
        self.type_cache.insert(expr_ref, type_decl);
    }

    pub fn get_cached_type(&self, expr_ref: &ExprRef) -> Option<TypeDecl> {
        self.type_cache.get(expr_ref).cloned()
    }

    pub fn clear_cache(&mut self) {
        self.type_cache.clear();
    }

    pub fn cache_size(&self) -> usize {
        self.type_cache.len()
    }

    pub fn has_cached_type(&self, expr_ref: &ExprRef) -> bool {
        self.type_cache.contains_key(expr_ref)
    }
}