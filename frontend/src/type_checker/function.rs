use std::collections::HashMap;
use string_interner::DefaultSymbol;
use crate::ast::StmtRef;
use crate::type_decl::TypeDecl;

#[derive(Debug)]
pub struct FunctionCheckingState {
    pub call_depth: usize,
    /// Return type per function *name*, for the call sites that only
    /// have a name to look one up by (`type_check_forward_ref`).
    pub is_checked_fn: HashMap<DefaultSymbol, Option<TypeDecl>>,
    /// Whether a *body* has been walked, keyed by the body itself.
    ///
    /// Separate from `is_checked_fn` because a name is not unique:
    /// `core/std/base64.t` and `core/std/hex.t` both define a free
    /// `encode`, and a name-keyed guard let the first one answer for
    /// the second. The second body was then never walked — and the
    /// type checker does not only *check* bodies, it **rewrites**
    /// them (the `?` desugar, `Display`'s `to_str`, CHAR-LITERAL-NUM
    /// narrowing, the SIMD result-type stamp), so every one of those
    /// silently did nothing there and the backends were handed the
    /// raw AST. `None` marks a body currently being walked, which is
    /// how recursion still terminates.
    pub checked_bodies: HashMap<StmtRef, Option<TypeDecl>>,
}

impl Default for FunctionCheckingState {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionCheckingState {
    pub fn new() -> Self {
        Self {
            call_depth: 0,
            is_checked_fn: HashMap::new(),
            checked_bodies: HashMap::new(),
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
        self.checked_bodies.clear();
    }
}