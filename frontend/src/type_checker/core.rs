use string_interner::DefaultStringInterner;
use crate::ast::*;

#[derive(Debug)]
pub struct CoreReferences<'a, 'b> {
    pub stmt_pool: &'a StmtPool,
    pub expr_pool: &'b mut ExprPool,
    pub string_interner: &'a DefaultStringInterner,
    pub location_pool: &'a LocationPool,
}

impl<'a, 'b> CoreReferences<'a, 'b> {
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
        }
    }
}