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
//! * **A write to a name from outside the loop.** `acc = acc + i`
//!   reads what the previous iteration wrote, which is the one thing
//!   "any order" cannot survive. It is refused here rather than in
//!   the lowering so every lane says the same thing — the tree-walker
//!   would otherwise run an accumulator perfectly well and the
//!   compiled lanes would not.
//! * **`break` and `return`.** Both mean "stop the rest", and the
//!   rest is on other threads. `continue` is fine: it ends one
//!   iteration, which is a thing an iteration can do on its own.
//!
//! What is *not* checked is whether the iterations are independent —
//! that is the caller's promise, written in `requires` if it is
//! written at all (CONCURRENCY.md section 5, point 3). Refusing to
//! guess here is the same decision `Span` made about aliasing. Nor
//! is a *mutating method call* on an outer binding (`v.push(x)`):
//! knowing that `push` writes to `v` takes the callee's signature,
//! and the lowering catches it where it does know
//! (`compiler_lower::parallel`).

use std::collections::HashMap;

use string_interner::{DefaultStringInterner, DefaultSymbol};

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
        let Some(Stmt::For(_, var, _, _, body)) = program.statement.get(&stmt_ref) else {
            continue;
        };
        let effects = table.of_expr(&body);
        if let Some(witness) = effects.witness(Effect::Io) {
            let path = render_path("the loop body", &witness.path);
            errors.push(
                TypeCheckError::parallel_body(
                    "prints".to_string(),
                    path,
                    "Collect what each iteration produces, into a slot of its own, and \
                     print after the loop"
                        .to_string(),
                )
                .with_location(at),
            );
        }
        for finding in order_dependencies(program, interner, var, &body) {
            let mut error = TypeCheckError::parallel_body(finding.what, finding.path, finding.fix)
                .with_location(at);
            if let Some(loc) = finding
                .at
                .and_then(|e| program.location_pool.get_expr_location(&e))
            {
                error = error.with_location(*loc);
            }
            errors.push(error);
        }
        if let Some(with_expr) = find_with(program, &body) {
            let mut error = TypeCheckError::parallel_body(
                "switches the allocator".to_string(),
                "the loop body".to_string(),
                "Open the allocator outside the loop, or leave the body on the default one"
                    .to_string(),
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

/// One order-dependent thing a body does, ready to be reported.
struct Finding {
    what: String,
    path: String,
    fix: String,
    at: Option<ExprRef>,
}

/// Walk the body for the three order dependencies a reader can see
/// without knowing any callee's signature: a write to a name from
/// outside, a `break`, and a `return`.
///
/// "From outside" is decided by tracking what the body binds. A
/// block's bindings leave with the block, so the set is cloned on the
/// way in and dropped on the way out — `var sum = 0u64` written
/// *inside* the loop is one per iteration and may be assigned freely.
///
/// A nested loop's own `break` belongs to that loop, so only a
/// `break` at this loop's own level is reported.
fn order_dependencies(
    program: &File,
    interner: &DefaultStringInterner,
    loop_var: DefaultSymbol,
    body: &ExprRef,
) -> Vec<Finding> {
    let mut bound: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
    bound.insert(loop_var);
    let mut out = Vec::new();
    walk_order(program, interner, body, &mut bound, true, &mut out);
    out
}

fn walk_order(
    program: &File,
    interner: &DefaultStringInterner,
    expr_ref: &ExprRef,
    bound: &mut std::collections::HashSet<DefaultSymbol>,
    own_level: bool,
    out: &mut Vec<Finding>,
) {
    let Some(expr) = program.expression.get(expr_ref) else {
        return;
    };
    match expr {
        Expr::Assign(lhs, rhs) => {
            walk_order(program, interner, &rhs, bound, own_level, out);
            if let Some(name) = assigned_root(program, &lhs)
                && !bound.contains(&name)
            {
                let spelled = interner.resolve(name).unwrap_or("?");
                out.push(Finding {
                    what: format!("assigns to `{spelled}`, which lives outside the loop"),
                    path: "the loop body".to_string(),
                    fix: "Give each iteration a place of its own — a slot indexed by the \
                          loop variable, written through a window (`Span<T>`) — and combine \
                          them after the loop"
                        .to_string(),
                    // The assignment's own node is recorded at the
                    // end of the statement; the target names the
                    // spot a reader is looking for.
                    at: Some(lhs),
                });
            }
        }
        Expr::Block(stmts) => {
            let mut inner = bound.clone();
            for s in &stmts {
                walk_order_stmt(program, interner, *s, &mut inner, own_level, out);
            }
        }
        Expr::IfElifElse(cond, then_block, elifs, else_block) => {
            walk_order(program, interner, &cond, bound, own_level, out);
            walk_order(program, interner, &then_block, bound, own_level, out);
            for (c, b) in &elifs {
                walk_order(program, interner, c, bound, own_level, out);
                walk_order(program, interner, b, bound, own_level, out);
            }
            walk_order(program, interner, &else_block, bound, own_level, out);
        }
        Expr::Match(scrutinee, arms) => {
            walk_order(program, interner, &scrutinee, bound, own_level, out);
            for arm in &arms {
                // An arm's pattern binds names for its own body.
                let mut inner = bound.clone();
                pattern_names(&arm.pattern, &mut inner);
                if let Some(g) = arm.guard {
                    walk_order(program, interner, &g, &mut inner, own_level, out);
                }
                walk_order(program, interner, &arm.body, &mut inner, own_level, out);
            }
        }
        other => {
            for child in child_exprs(&other) {
                walk_order(program, interner, &child, bound, own_level, out);
            }
        }
    }
}

fn walk_order_stmt(
    program: &File,
    interner: &DefaultStringInterner,
    stmt_ref: StmtRef,
    bound: &mut std::collections::HashSet<DefaultSymbol>,
    own_level: bool,
    out: &mut Vec<Finding>,
) {
    let Some(stmt) = program.statement.get(&stmt_ref) else {
        return;
    };
    match stmt {
        Stmt::Val(name, _, e) => {
            walk_order(program, interner, &e, bound, own_level, out);
            bound.insert(name);
        }
        Stmt::Var(name, _, e) => {
            if let Some(e) = e {
                walk_order(program, interner, &e, bound, own_level, out);
            }
            bound.insert(name);
        }
        Stmt::Expression(e) => walk_order(program, interner, &e, bound, own_level, out),
        Stmt::Return(e) => {
            if let Some(e) = e {
                walk_order(program, interner, &e, bound, own_level, out);
            }
            out.push(Finding {
                what: "returns from inside the loop".to_string(),
                path: "the loop body".to_string(),
                fix: "A `return` here would leave the other iterations running. Record what \
                      the iteration found, and answer after the loop"
                    .to_string(),
                at: e,
            });
        }
        Stmt::Break(_) if own_level => out.push(Finding {
            what: "breaks out of the loop".to_string(),
            path: "the loop body".to_string(),
            fix: "Stopping early is a decision about iterations that may already have run, \
                  or be running. Let every iteration finish, and decide after the loop"
                .to_string(),
            at: None,
        }),
        Stmt::While(_, cond, body) => {
            walk_order(program, interner, &cond, bound, own_level, out);
            // Inside a nested loop, `break` is that loop's.
            walk_order(program, interner, &body, bound, false, out);
        }
        Stmt::For(_, var, start, end, body) => {
            walk_order(program, interner, &start, bound, own_level, out);
            walk_order(program, interner, &end, bound, own_level, out);
            let mut inner = bound.clone();
            inner.insert(var);
            walk_order(program, interner, &body, &mut inner, false, out);
        }
        _ => {}
    }
}

/// The name an assignment target ultimately writes: `x`, `x.f`,
/// `x.f[i]`, `x[i]` all write `x`.
fn assigned_root(program: &File, expr_ref: &ExprRef) -> Option<DefaultSymbol> {
    match program.expression.get(expr_ref)? {
        Expr::Identifier(name) => Some(name),
        Expr::FieldAccess(base, _) | Expr::TupleAccess(base, _) | Expr::SliceAccess(base, _) => {
            assigned_root(program, &base)
        }
        _ => None,
    }
}

/// Every name a pattern binds.
fn pattern_names(
    pattern: &crate::ast::Pattern,
    bound: &mut std::collections::HashSet<DefaultSymbol>,
) {
    use crate::ast::Pattern;
    match pattern {
        Pattern::Name(name) => {
            bound.insert(*name);
        }
        Pattern::Binding(name, inner) => {
            bound.insert(*name);
            pattern_names(inner, bound);
        }
        Pattern::EnumVariant(_, _, subs) | Pattern::Tuple(subs) => {
            for sub in subs {
                pattern_names(sub, bound);
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (_, sub) in fields {
                pattern_names(sub, bound);
            }
        }
        Pattern::Literal(_) | Pattern::Range(..) | Pattern::Wildcard => {}
    }
}
