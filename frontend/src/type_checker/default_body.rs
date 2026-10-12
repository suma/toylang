//! TRAIT-DEFAULT-BODY-T: a copy of a generic trait's default body, in
//! one impl's terms.
//!
//! A default body is written against the trait's own parameters (`T`
//! in `trait Iterator<T>`). The impl that inherits it names them its
//! own way: `impl<T> Iterator<T> for VecIter<T>` keeps `T`, but
//! `impl<T, U> Iterator<U> for MapIter<T, U>` means `U` by it, and
//! `impl Iterator<u8> for StringIter` means `u8`. The inherited
//! signature is restated per impl (`synthesize_default_method`); a body
//! that *spells* `T` (`val x: T`, `T::default()`, `Vec<T>`,
//! `__builtin_sizeof::<T>()`) needs the same, and it cannot share one
//! tree between impls that disagree on what `T` is -- every backend
//! reads the annotation off the node.
//!
//! So such a body is copied, once per impl that needs it, with every
//! type in it restated. A body that never spells a trait parameter,
//! or an impl whose arguments are the parameters themselves, keeps the
//! shared tree: the copy is only made where it changes something.
//!
//! The copy carries each node's location. It does not carry the
//! parser's side tables (`call_paths`, `pattern_sites`): a written
//! `module::f()` inside the copy is not re-checked against its path,
//! and `toy query` answers from the original.

use rustc_hash::FxHashMap;
use string_interner::DefaultSymbol;

use crate::ast::{
    BuiltinFunction, Expr, ExprPool, ExprRef, LocationPool, MatchArm, Pattern, SliceInfo, Stmt,
    StmtPool, StmtRef,
};
use crate::type_decl::TypeDecl;

/// The body `body` restated under `subst` (trait parameter -> the
/// impl's argument), or `None` when no node in it spells a parameter
/// `subst` changes.
pub(crate) fn restate_default_body(
    stmts: &mut StmtPool,
    exprs: &mut ExprPool,
    locations: Option<&mut LocationPool>,
    body: StmtRef,
    params: &[DefaultSymbol],
    subst: &FxHashMap<DefaultSymbol, TypeDecl>,
) -> Option<StmtRef> {
    // Only the parameters the impl actually renames.
    let subst: FxHashMap<DefaultSymbol, TypeDecl> = subst
        .iter()
        .filter(|(p, t)| !matches!(t, TypeDecl::Generic(g) | TypeDecl::Identifier(g) if g == *p))
        .map(|(p, t)| (*p, t.clone()))
        .collect();
    if subst.is_empty() {
        return None;
    }
    let mut probe = Copier { stmts, exprs, locations: None, params, subst: &subst, dry: true, changed: false };
    probe.stmt(body);
    if !probe.changed {
        return None;
    }
    let mut copier = Copier { stmts, exprs, locations, params, subst: &subst, dry: false, changed: false };
    Some(copier.stmt(body))
}

/// Walks a subtree children-first. `dry` only looks (`changed` says
/// whether a copy would differ); otherwise each node is re-added with
/// its children's new refs and its types restated.
struct Copier<'a> {
    stmts: &'a mut StmtPool,
    exprs: &'a mut ExprPool,
    locations: Option<&'a mut LocationPool>,
    params: &'a [DefaultSymbol],
    subst: &'a FxHashMap<DefaultSymbol, TypeDecl>,
    dry: bool,
    changed: bool,
}

impl Copier<'_> {
    fn ty(&mut self, t: &TypeDecl) -> TypeDecl {
        let out = t.name_params_generic(self.params).substitute_generics(self.subst);
        if !self.changed && out != t.name_params_generic(self.params) {
            self.changed = true;
        }
        out
    }

    /// A type parameter used as a path (`T::default()`): the name of
    /// what it stands for, when that is a named type.
    fn type_name(&mut self, s: DefaultSymbol) -> DefaultSymbol {
        let Some(to) = self.subst.get(&s) else {
            return s;
        };
        let named = match to {
            TypeDecl::Generic(n) | TypeDecl::Identifier(n) => Some(*n),
            TypeDecl::Struct(n, _) | TypeDecl::Enum(n, _) => Some(*n),
            _ => None,
        };
        match named {
            Some(n) => {
                self.changed |= n != s;
                n
            }
            None => s,
        }
    }

    fn exprs_of(&mut self, rs: &[ExprRef]) -> Vec<ExprRef> {
        rs.iter().map(|r| self.expr(*r)).collect()
    }

    fn opt(&mut self, r: Option<ExprRef>) -> Option<ExprRef> {
        r.map(|r| self.expr(r))
    }

    fn pattern(&mut self, p: &Pattern) -> Pattern {
        match p {
            Pattern::EnumVariant(e, v, subs) => {
                let e = self.type_name(*e);
                Pattern::EnumVariant(e, *v, subs.iter().map(|s| self.pattern(s)).collect())
            }
            Pattern::Literal(r) => Pattern::Literal(self.expr(*r)),
            Pattern::Tuple(subs) => Pattern::Tuple(subs.iter().map(|s| self.pattern(s)).collect()),
            Pattern::Struct(n, fields, rest) => Pattern::Struct(
                *n,
                fields.iter().map(|(f, s)| (*f, self.pattern(s))).collect(),
                *rest,
            ),
            Pattern::Range(a, b) => Pattern::Range(self.expr(*a), self.expr(*b)),
            Pattern::Binding(n, inner) => Pattern::Binding(*n, Box::new(self.pattern(inner))),
            Pattern::Name(_) | Pattern::Wildcard => p.clone(),
        }
    }

    fn builtin(&mut self, f: &BuiltinFunction) -> BuiltinFunction {
        match f {
            BuiltinFunction::PtrReadTyped(t) => BuiltinFunction::PtrReadTyped(self.ty(t)),
            BuiltinFunction::PtrRefTyped(t) => BuiltinFunction::PtrRefTyped(self.ty(t)),
            BuiltinFunction::SizeOfType(t) => BuiltinFunction::SizeOfType(self.ty(t)),
            other => other.clone(),
        }
    }

    fn expr(&mut self, r: ExprRef) -> ExprRef {
        let Some(e) = self.exprs.get(&r) else {
            return r;
        };
        let new = match &e {
            Expr::Assign(a, b) => Expr::Assign(self.expr(*a), self.expr(*b)),
            Expr::IfElifElse(c, t, elifs, el) => {
                let c = self.expr(*c);
                let t = self.expr(*t);
                let elifs = elifs.iter().map(|(a, b)| (self.expr(*a), self.expr(*b))).collect();
                Expr::IfElifElse(c, t, elifs, self.expr(*el))
            }
            Expr::Binary(op, a, b) => Expr::Binary(op.clone(), self.expr(*a), self.expr(*b)),
            Expr::Unary(op, a) => Expr::Unary(op.clone(), self.expr(*a)),
            Expr::Block(ss) => Expr::Block(ss.iter().map(|s| self.stmt(*s)).collect()),
            Expr::ExprList(items) => Expr::ExprList(self.exprs_of(items)),
            Expr::Call(f, args) => Expr::Call(*f, self.expr(*args)),
            Expr::ArrayLiteral(items) => Expr::ArrayLiteral(self.exprs_of(items)),
            Expr::TupleLiteral(items) => Expr::TupleLiteral(self.exprs_of(items)),
            Expr::FieldAccess(o, f) => Expr::FieldAccess(self.expr(*o), *f),
            Expr::TupleAccess(o, i) => Expr::TupleAccess(self.expr(*o), *i),
            Expr::MethodCall(o, m, args) => {
                let o = self.expr(*o);
                Expr::MethodCall(o, *m, self.exprs_of(args))
            }
            Expr::StructLiteral(n, fields) => {
                Expr::StructLiteral(*n, fields.iter().map(|(f, v)| (*f, self.expr(*v))).collect())
            }
            Expr::BuiltinMethodCall(o, m, args) => {
                let o = self.expr(*o);
                Expr::BuiltinMethodCall(o, m.clone(), self.exprs_of(args))
            }
            Expr::BuiltinCall(f, args) => {
                let f = self.builtin(f);
                Expr::BuiltinCall(f, self.exprs_of(args))
            }
            Expr::SliceAccess(o, info) => {
                let o = self.expr(*o);
                let info = SliceInfo { start: self.opt(info.start), end: self.opt(info.end), ..info.clone() };
                Expr::SliceAccess(o, info)
            }
            Expr::SliceAssign(o, a, b, v) => {
                let o = self.expr(*o);
                let a = self.opt(*a);
                let b = self.opt(*b);
                Expr::SliceAssign(o, a, b, self.expr(*v))
            }
            Expr::AssociatedFunctionCall(t, f, args) => {
                let t = self.type_name(*t);
                Expr::AssociatedFunctionCall(t, *f, self.exprs_of(args))
            }
            Expr::DictLiteral(entries) => {
                Expr::DictLiteral(entries.iter().map(|(k, v)| (self.expr(*k), self.expr(*v))).collect())
            }
            Expr::Cast(a, t) => {
                let a = self.expr(*a);
                Expr::Cast(a, self.ty(t))
            }
            Expr::With(a, b) => Expr::With(self.expr(*a), self.expr(*b)),
            Expr::Match(s, arms) => {
                let s = self.expr(*s);
                let arms = arms
                    .iter()
                    .map(|arm| MatchArm {
                        pattern: self.pattern(&arm.pattern),
                        guard: self.opt(arm.guard),
                        body: self.expr(arm.body),
                    })
                    .collect();
                Expr::Match(s, arms)
            }
            Expr::Range(a, b) => Expr::Range(self.expr(*a), self.expr(*b)),
            Expr::Closure { params, return_type, body, captures_by_ref } => {
                let params = params.iter().map(|(n, t)| (*n, self.ty(t))).collect();
                let return_type = return_type.as_ref().map(|t| self.ty(t));
                Expr::Closure { params, return_type, body: self.expr(*body), captures_by_ref: *captures_by_ref }
            }
            Expr::Try { inner, .. } => {
                let inner = self.expr(*inner);
                match e.clone() {
                    Expr::Try { inner: _, scrutinee_binding, success_binding, error_binding, panic_msg, converted_binding, result_binding } => {
                        Expr::Try { inner, scrutinee_binding, success_binding, error_binding, panic_msg, converted_binding, result_binding }
                    }
                    _ => unreachable!(),
                }
            }
            Expr::NullCoalesce { lhs, rhs, scrutinee_binding, success_binding, error_binding } => {
                let lhs = self.expr(*lhs);
                Expr::NullCoalesce {
                    lhs,
                    rhs: self.expr(*rhs),
                    scrutinee_binding: *scrutinee_binding,
                    success_binding: *success_binding,
                    error_binding: *error_binding,
                }
            }
            Expr::StructUpdate { type_name, fields, base, base_binding } => {
                let fields = fields.iter().map(|(f, v)| (*f, self.expr(*v))).collect();
                Expr::StructUpdate { type_name: *type_name, fields, base: self.expr(*base), base_binding: *base_binding }
            }
            Expr::True
            | Expr::False
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::CharLiteral(_)
            | Expr::Float64(_)
            | Expr::Float32(_)
            | Expr::Number(_)
            | Expr::Identifier(_)
            | Expr::Null
            | Expr::String(_)
            | Expr::QualifiedIdentifier(_) => e.clone(),
        };
        if self.dry {
            return r;
        }
        let out = self.exprs.add(new);
        if let Some(locs) = self.locations.as_deref_mut() {
            let at = locs.get_expr_location(&r).copied();
            locs.put_expr_location(&out, at);
        }
        out
    }

    fn stmt(&mut self, r: StmtRef) -> StmtRef {
        let Some(s) = self.stmts.get(&r) else {
            return r;
        };
        let new = match &s {
            Stmt::Expression(e) => Stmt::Expression(self.expr(*e)),
            Stmt::Val(n, t, e) => {
                let t = t.as_ref().map(|t| self.ty(t));
                Stmt::Val(*n, t, self.expr(*e))
            }
            Stmt::Var(n, t, e) => {
                let t = t.as_ref().map(|t| self.ty(t));
                Stmt::Var(*n, t, self.opt(*e))
            }
            Stmt::Return(e) => Stmt::Return(self.opt(*e)),
            Stmt::For(l, v, a, b, body) => {
                let a = self.expr(*a);
                let b = self.expr(*b);
                Stmt::For(*l, *v, a, b, self.expr(*body))
            }
            Stmt::While(l, c, body) => {
                let c = self.expr(*c);
                Stmt::While(*l, c, self.expr(*body))
            }
            // Declarations do not occur in a method body, and `break` /
            // `continue` hold no tree.
            _ => s.clone(),
        };
        if self.dry {
            return r;
        }
        let out = self.stmts.add(new);
        if let Some(locs) = self.locations.as_deref_mut() {
            let at = locs.get_stmt_location(&r).copied();
            locs.put_stmt_location(&out, at);
        }
        out
    }
}
