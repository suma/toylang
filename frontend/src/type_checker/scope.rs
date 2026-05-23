use std::collections::HashMap;
use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::Function;
use crate::type_checker::TypeCheckerVisitor;

/// Scope-management helpers for `TypeCheckerVisitor`.
///
/// Thin wrappers around `TypeCheckContext` so that the visitor layer
/// (which holds a mutable borrow of the context) can expose a
/// consistent API without callers touching `context` directly.
impl<'a> TypeCheckerVisitor<'a> {
    /// Push a new variable scope (used when entering a block).
    pub fn push_context(&mut self) {
        self.context.push_scope();
    }

    /// Pop the innermost variable scope (used when exiting a block).
    pub fn pop_context(&mut self) {
        self.context.pop_scope();
    }

    /// Register a user-authored function under its bare name.
    pub fn add_function(&mut self, f: Rc<Function>) {
        self.context.set_fn(f.name, f.clone());
    }

    /// Register an imported / module-qualified function.
    pub fn add_function_with_module(
        &mut self,
        qualifier: Option<DefaultSymbol>,
        f: Rc<Function>,
    ) {
        self.context.set_fn_with_module(qualifier, f.name, f.clone());
    }

    /// Return a clone of the expression → type map built by
    /// inference.  Used by tests and by later compilation stages.
    pub fn get_expr_types(&self) -> HashMap<crate::ast::ExprRef, crate::type_decl::TypeDecl> {
        self.type_inference.expr_types.clone()
    }
}
