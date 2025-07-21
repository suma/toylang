use string_interner::DefaultStringInterner;
use crate::ast::*;

#[derive(Debug)]
pub struct CoreReferences<'a, 'b, 'c, 'd> {
    pub stmt_pool: &'a StmtPool,
    pub expr_pool: &'b mut ExprPool,
    pub string_interner: &'c DefaultStringInterner,
    pub location_pool: &'d LocationPool,
}

impl<'a, 'b, 'c, 'd> CoreReferences<'a, 'b, 'c, 'd> {
    pub fn new(
        stmt_pool: &'a StmtPool,
        expr_pool: &'b mut ExprPool,
        string_interner: &'c DefaultStringInterner,
        location_pool: &'d LocationPool,
    ) -> Self {
        Self {
            stmt_pool,
            expr_pool,
            string_interner,
            location_pool,
        }
    }
}