//! CLOSURE-CAPTURE E3: which closures may share their captured
//! bindings with the scope that owns them.
//!
//! A closure that cannot outlive its captures can hold them by
//! reference: reads see the current value and writes reach the outer
//! binding. One that can outlive them must take a copy, and writing
//! to a copy is `E0021`.
//!
//! Without lifetimes the safe answer is a syntactic one. A closure
//! qualifies when it is bound by `val NAME = fn(...)` in the body of
//! the function that owns the captures, and every mention of `NAME`
//! in that function is the callee of a direct call at the same
//! nesting. Anything else — passing it to a function, returning it,
//! storing it in a struct, calling it from inside another closure —
//! puts the closure somewhere the frame may already have left, so it
//! keeps the copy. That is deliberately blunt: being wrong in this
//! direction costs a diagnostic, being wrong in the other costs a
//! read of a dead frame.
//!
//! The answer is written onto the closure node itself
//! (`Expr::Closure::captures_by_ref`) rather than into a side table,
//! because all five engines need it and each of them already has its
//! own copy of the *capture scan* — the duplication that let them
//! disagree about writes to begin with.

use std::collections::{HashMap, HashSet};

use string_interner::DefaultSymbol;

use crate::ast::{Expr, ExprPool, ExprRef, Stmt, StmtPool, StmtRef};

/// Decide the capture mode of every closure bound in `body`, and
/// record it on the closure nodes.
///
/// `body` is the statement list of one function. Nested functions do
/// not exist in this language, so a closure's captures belong either
/// to this function or to an enclosing closure in it.
/// Returns the *body* refs of the closures it marked, which is how
/// the type checker keys a closure while it is checking one (the
/// visitor is handed the body, not the closure node). Both answers
/// come from this one walk so the two consumers cannot drift.
pub fn mark_by_ref_closures(
    expr_pool: &mut ExprPool,
    stmt_pool: &StmtPool,
    body: &[StmtRef],
) -> HashSet<ExprRef> {
    let mut scan = Scan::new(expr_pool, stmt_pool);
    for stmt in body {
        scan.walk_stmt(*stmt);
    }
    let by_ref = scan.settle();
    let mut bodies = HashSet::with_capacity(by_ref.len());
    for expr_ref in by_ref {
        let Some(Expr::Closure { params, return_type, body, .. }) = expr_pool.get(&expr_ref) else {
            continue;
        };
        bodies.insert(body);
        expr_pool.update(
            &expr_ref,
            Expr::Closure { params, return_type, body, captures_by_ref: true },
        );
    }
    bodies
}

struct Scan<'a> {
    expr_pool: &'a ExprPool,
    stmt_pool: &'a StmtPool,
    /// `val NAME = fn(...)` bound directly in this function's body.
    candidates: HashMap<DefaultSymbol, ExprRef>,
    /// Names disqualified: mentioned as a value, bound more than
    /// once, or called from inside another closure.
    rejected: HashSet<DefaultSymbol>,
    /// How many closure bodies deep the walk currently is.
    depth: u32,
}

impl<'a> Scan<'a> {
    fn new(expr_pool: &'a ExprPool, stmt_pool: &'a StmtPool) -> Self {
        Self {
            expr_pool,
            stmt_pool,
            candidates: HashMap::new(),
            rejected: HashSet::new(),
            depth: 0,
        }
    }

    fn settle(self) -> Vec<ExprRef> {
        self.candidates
            .into_iter()
            .filter(|(name, _)| !self.rejected.contains(name))
            .map(|(_, expr_ref)| expr_ref)
            .collect()
    }

    fn walk_stmt(&mut self, stmt_ref: StmtRef) {
        let Some(stmt) = self.stmt_pool.get(&stmt_ref) else {
            return;
        };
        match stmt {
            Stmt::Val(name, _, value) => {
                self.record_binding(name, value);
                self.walk_expr(value);
            }
            Stmt::Var(name, _, value) => {
                if let Some(value) = value {
                    // A `var` holding a closure can be reassigned, and
                    // the assignment mentions the name as a target, so
                    // it disqualifies itself through `Assign`. Record
                    // it the same way and let that happen.
                    self.record_binding(name, value);
                    self.walk_expr(value);
                }
            }
            Stmt::Expression(e) => self.walk_expr(e),
            Stmt::Return(e) => {
                if let Some(e) = e {
                    self.walk_expr(e);
                }
            }
            Stmt::While(_, cond, block) => {
                self.walk_expr(cond);
                self.walk_expr(block);
            }
            Stmt::For(_, _, start, end, block) => {
                self.walk_expr(start);
                self.walk_expr(end);
                self.walk_expr(block);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            // Declarations carry their own bodies, which are checked
            // as their own functions.
            _ => {}
        }
    }

    /// A `val NAME = fn(...)` at this function's own nesting is a
    /// candidate; a second binding of the same name gives it up (the
    /// walk would not know which one a call reaches).
    fn record_binding(&mut self, name: DefaultSymbol, value: ExprRef) {
        if !matches!(self.expr_pool.get(&value), Some(Expr::Closure { .. })) {
            return;
        }
        if self.depth > 0 || self.candidates.insert(name, value).is_some() {
            self.rejected.insert(name);
        }
    }

    fn walk_expr(&mut self, expr_ref: ExprRef) {
        let Some(expr) = self.expr_pool.get(&expr_ref) else {
            return;
        };
        match expr {
            // The only mention that keeps a closure eligible is being
            // the callee of a direct call, and a callee is a symbol
            // rather than an identifier expression — so every
            // `Identifier` naming a candidate is a use as a value.
            Expr::Identifier(name) => {
                self.rejected.insert(name);
            }
            // A direct call at this nesting is fine. From inside
            // another closure it is not: that closure may itself be
            // called after the frame is gone.
            Expr::Call(name, args) => {
                if self.depth > 0 {
                    self.rejected.insert(name);
                }
                self.walk_expr(args);
            }
            Expr::Closure { body, .. } => {
                self.depth += 1;
                self.walk_expr(body);
                self.depth -= 1;
            }
            Expr::Block(stmts) => {
                for s in &stmts {
                    self.walk_stmt(*s);
                }
            }
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                self.walk_expr(cond);
                self.walk_expr(then_block);
                for (c, b) in &elifs {
                    self.walk_expr(*c);
                    self.walk_expr(*b);
                }
                self.walk_expr(else_block);
            }
            Expr::Match(scrutinee, arms) => {
                self.walk_expr(scrutinee);
                for arm in &arms {
                    if let Some(guard) = arm.guard {
                        self.walk_expr(guard);
                    }
                    self.walk_expr(arm.body);
                }
            }
            Expr::Assign(lhs, rhs) | Expr::Binary(_, lhs, rhs) | Expr::Range(lhs, rhs) => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            Expr::StructLiteral(_, fields) => {
                for (_, value) in &fields {
                    self.walk_expr(*value);
                }
            }
            Expr::StructUpdate { fields, base, .. } => {
                for (_, value) in &fields {
                    self.walk_expr(*value);
                }
                self.walk_expr(base);
            }
            Expr::TupleLiteral(items)
            | Expr::ArrayLiteral(items)
            | Expr::ExprList(items)
            | Expr::BuiltinCall(_, items) => {
                for e in &items {
                    self.walk_expr(*e);
                }
            }
            Expr::DictLiteral(entries) => {
                for (k, v) in &entries {
                    self.walk_expr(*k);
                    self.walk_expr(*v);
                }
            }
            Expr::MethodCall(receiver, _, args) | Expr::BuiltinMethodCall(receiver, _, args) => {
                self.walk_expr(receiver);
                for a in &args {
                    self.walk_expr(*a);
                }
            }
            Expr::AssociatedFunctionCall(_, _, args) => {
                for a in &args {
                    self.walk_expr(*a);
                }
            }
            Expr::Unary(_, operand) => self.walk_expr(operand),
            Expr::FieldAccess(obj, _) | Expr::TupleAccess(obj, _) | Expr::SliceAccess(obj, _) => {
                self.walk_expr(obj)
            }
            Expr::Cast(inner, _) | Expr::Try { inner, .. } => self.walk_expr(inner),
            // `a ?? b` — both operands are uses as a value (the
            // desugar moves them into a `val` + `match`).
            Expr::NullCoalesce { lhs, rhs, .. } => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            Expr::SliceAssign(object, start, end, value) => {
                self.walk_expr(object);
                for bound in [start, end].into_iter().flatten() {
                    self.walk_expr(bound);
                }
                self.walk_expr(value);
            }
            Expr::With(allocator, body) => {
                self.walk_expr(allocator);
                self.walk_expr(body);
            }
            Expr::QualifiedIdentifier(_)
            | Expr::Int64(_)
            | Expr::UInt64(_)
            | Expr::Int8(_)
            | Expr::Int16(_)
            | Expr::Int32(_)
            | Expr::UInt8(_)
            | Expr::UInt16(_)
            | Expr::UInt32(_)
            | Expr::Float64(_)
            | Expr::Float32(_)
            | Expr::Number(_)
            | Expr::String(_)
            | Expr::True
            | Expr::False
            | Expr::Null => {}
        }
    }
}
