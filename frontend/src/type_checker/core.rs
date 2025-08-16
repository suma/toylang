use string_interner::DefaultStringInterner;
use crate::ast::*;
use crate::module_resolver::ModuleResolver;

#[derive(Debug)]
pub struct CoreReferences<'a, 'b, 'c> {
    pub stmt_pool: &'a StmtPool,
    pub expr_pool: &'b mut ExprPool,
    pub string_interner: &'a DefaultStringInterner,
    pub location_pool: &'a LocationPool,
    pub module_resolver: Option<&'c mut ModuleResolver>,
}

impl<'a, 'b, 'c> CoreReferences<'a, 'b, 'c> {
    pub fn new(
        stmt_pool: &'a StmtPool,
        expr_pool: &'b mut ExprPool,
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool,
    ) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            location_pool,
            module_resolver: None,
        }
    }
    
    pub fn with_module_resolver(
        stmt_pool: &'a StmtPool,
        expr_pool: &'b mut ExprPool,
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool,
        module_resolver: &'c mut ModuleResolver,
    ) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            location_pool,
            module_resolver: Some(module_resolver),
        }
    }
}