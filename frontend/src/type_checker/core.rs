use string_interner::DefaultStringInterner;
use crate::ast::*;
use crate::module_resolver::ModuleResolver;

#[derive(Debug)]
pub struct CoreReferences<'a> {
    /// `&mut` so the type checker can allocate fresh `Stmt`s during
    /// AST rewriting (e.g. the `?` operator desugars to a `match`
    /// whose error arm contains a synthetic `Stmt::Return` + dead
    /// `Stmt::Expression(panic)`). Read-only callers can also use
    /// the `&mut` ref — it widens, not narrows, what's allowed.
    pub stmt_pool: &'a mut StmtPool,
    pub expr_pool: &'a mut ExprPool,
    pub string_interner: &'a DefaultStringInterner,
    pub location_pool: &'a LocationPool,
    pub module_resolver: Option<&'a mut ModuleResolver>,
}

impl<'a> CoreReferences<'a> {
    pub fn new(
        stmt_pool: &'a mut StmtPool,
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

    pub fn from_program(program: &'a mut Program, string_interner: &'a DefaultStringInterner) -> Self {
        Self {
            stmt_pool: &mut program.statement,
            expr_pool: &mut program.expression,
            string_interner,
            location_pool: &program.location_pool,
            module_resolver: None,
        }
    }

    pub fn with_module_resolver(
        stmt_pool: &'a mut StmtPool,
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