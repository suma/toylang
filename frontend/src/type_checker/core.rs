use string_interner::DefaultStringInterner;
use crate::ast::*;
use crate::module_resolver::ModuleResolver;

#[derive(Debug)]
pub struct CoreReferences<'a> {
    pub stmt_pool: &'a StmtPool,
    pub expr_pool: &'a mut ExprPool,
    pub string_interner: &'a DefaultStringInterner,
    pub location_pool: &'a LocationPool,
    pub module_resolver: Option<&'a mut ModuleResolver>,
}

impl<'a> CoreReferences<'a> {
    pub fn new(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a mut ExprPool,
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
    
    pub fn from_program(program: &'a mut Program) -> Self {
        Self {
            stmt_pool: &program.statement,
            expr_pool: &mut program.expression,
            string_interner: &program.string_interner,
            location_pool: &program.location_pool,
            module_resolver: None,
        }
    }
    
    pub fn with_module_resolver(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a mut ExprPool,
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool,
        module_resolver: &'a mut ModuleResolver,
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