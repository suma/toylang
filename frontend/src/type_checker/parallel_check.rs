//! CONCURRENCY A1: what a `parallel for` body may not do.
//!
//! The modifier says the iterations may run in any order, and later
//! in parallel. Two things stop that from being a free choice, and
//! both are refused here rather than left to the reader:
//!
//! * **Output.** Interleaved `println`s are not the same output, and
//!   the lanes would stop agreeing the moment one of them actually
//!   ran the iterations at once. Collect and print outside the loop.
//! * **`with allocator = ...`.** The region check
//!   (`design-docs/REGIONS.md`) reasons about one control flow; a
//!   scoped allocator shared by several is outside what it can say.
//!   The body runs on the default allocator.
//!
//! The reachability walk is [`super::effects`], the same one
//! `never_allocates` and `const fn` use, so "reaches" means the same
//! thing in all three and an opaque hop (`extern`, a closure, `dyn`)
//! is reported rather than assumed innocent.
//!
//! What is *not* checked is whether the iterations are independent —
//! that is the caller's promise, written in `requires` if it is
//! written at all (CONCURRENCY.md section 5, point 3). Refusing to
//! guess here is the same decision `Span` made about aliasing.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{Expr, ExprRef, File, Stmt, StmtRef};
use crate::type_checker::effects::{render_path, Effect, EffectTable};
use crate::type_checker::error::TypeCheckError;
use crate::type_decl::TypeDecl;

pub fn check_parallel_loops(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    if program.parallel_loops.is_empty() {
        return Vec::new();
    }
    let mut table = EffectTable::new(program, interner, expr_types);
    let mut errors = Vec::new();

    // Sorted so two loops in one file are reported in a stable order
    // whatever the map iterates in.
    let mut loops: Vec<(StmtRef, crate::type_checker::SourceLocation)> =
        program.parallel_loops.iter().map(|(s, l)| (*s, *l)).collect();
    loops.sort_by_key(|(s, _)| s.0);

    for (stmt_ref, at) in loops {
        let Some(Stmt::For(_, _, _, _, body)) = program.statement.get(&stmt_ref) else {
            continue;
        };
        let effects = table.of_expr(&body);
        if let Some(witness) = effects.witness(Effect::Io) {
            let path = render_path("the loop body", &witness.path);
            errors.push(
                TypeCheckError::parallel_body("prints".to_string(), path).with_location(at),
            );
        }
        if let Some(with_expr) = find_with(program, &body) {
            let mut error = TypeCheckError::parallel_body(
                "switches the allocator".to_string(),
                "the loop body".to_string(),
            )
            .with_location(at);
            if let Some(loc) = program.location_pool.get_expr_location(&with_expr) {
                error = error.with_location(*loc);
            }
            errors.push(error);
        }
    }

    errors
}

/// The first `with allocator = ...` written inside this expression.
///
/// Syntactic, not reachability: a `with` in a *function* the body
/// calls is that function's own scope, entered and left inside one
/// iteration, and the region check already has it. What cannot be
/// allowed is the body opening one across iterations.
fn find_with(program: &File, expr_ref: &ExprRef) -> Option<ExprRef> {
    let mut work: Vec<ExprRef> = vec![*expr_ref];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = work.pop() {
        if !seen.insert(current.0) {
            continue;
        }
        let Some(expr) = program.expression.get(&current) else {
            continue;
        };
        match expr {
            Expr::With(..) => return Some(current),
            Expr::Block(stmts) => {
                for s in &stmts {
                    push_stmt(program, *s, &mut work);
                }
            }
            Expr::IfElifElse(cond, then_block, elifs, else_block) => {
                work.push(cond);
                work.push(then_block);
                for (c, b) in &elifs {
                    work.push(*c);
                    work.push(*b);
                }
                work.push(else_block);
            }
            Expr::Match(scrutinee, arms) => {
                work.push(scrutinee);
                for arm in &arms {
                    if let Some(g) = arm.guard {
                        work.push(g);
                    }
                    work.push(arm.body);
                }
            }
            other => {
                for child in child_exprs(&other) {
                    work.push(child);
                }
            }
        }
    }
    None
}

fn push_stmt(program: &File, stmt_ref: StmtRef, work: &mut Vec<ExprRef>) {
    let Some(stmt) = program.statement.get(&stmt_ref) else {
        return;
    };
    match stmt {
        Stmt::Val(_, _, e) | Stmt::Expression(e) => work.push(e),
        Stmt::Var(_, _, Some(e)) => work.push(e),
        Stmt::Return(Some(e)) => work.push(e),
        Stmt::While(_, cond, body) => {
            work.push(cond);
            work.push(body);
        }
        Stmt::For(_, _, start, end, body) => {
            work.push(start);
            work.push(end);
            work.push(body);
        }
        _ => {}
    }
}

/// Every sub-expression of a node, for the generic arms above.
fn child_exprs(expr: &Expr) -> Vec<ExprRef> {
    match expr {
        Expr::Binary(_, a, b) | Expr::Assign(a, b) | Expr::Range(a, b) => vec![*a, *b],
        Expr::Unary(_, a) | Expr::Cast(a, _) | Expr::Try { inner: a, .. } => vec![*a],
        Expr::FieldAccess(a, _) | Expr::TupleAccess(a, _) | Expr::SliceAccess(a, _) => vec![*a],
        Expr::NullCoalesce { lhs, rhs, .. } => vec![*lhs, *rhs],
        Expr::ExprList(items) | Expr::TupleLiteral(items) | Expr::ArrayLiteral(items) => {
            items.clone()
        }
        Expr::Call(_, args) => vec![*args],
        Expr::MethodCall(receiver, _, args) => {
            let mut out = vec![*receiver];
            out.extend(args.iter().copied());
            out
        }
        Expr::BuiltinMethodCall(receiver, _, args) => {
            let mut out = vec![*receiver];
            out.extend(args.iter().copied());
            out
        }
        Expr::AssociatedFunctionCall(_, _, args) | Expr::BuiltinCall(_, args) => args.clone(),
        Expr::StructLiteral(_, fields) => fields.iter().map(|(_, v)| *v).collect(),
        Expr::StructUpdate { fields, base, .. } => {
            let mut out: Vec<ExprRef> = fields.iter().map(|(_, v)| *v).collect();
            out.push(*base);
            out
        }
        Expr::DictLiteral(entries) => {
            let mut out = Vec::with_capacity(entries.len() * 2);
            for (k, v) in entries {
                out.push(*k);
                out.push(*v);
            }
            out
        }
        Expr::SliceAssign(object, start, end, value) => {
            let mut out = vec![*object, *value];
            if let Some(s) = start {
                out.push(*s);
            }
            if let Some(e) = end {
                out.push(*e);
            }
            out
        }
        _ => Vec::new(),
    }
}
