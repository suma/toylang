//! CONTRACT-ELISION: what a function's `requires` clauses prove about
//! its parameters, and therefore which RUNTIME-TRAP guards its body
//! does not need.
//!
//! ```text
//! fn div(a: i64, b: i64) -> i64
//!     requires b != 0i64      // proves the guard below can never fire
//! { a / b }
//! ```
//!
//! Two properties make this sound, and both are load-bearing:
//!
//! 1. **Parameters are immutable.** The type checker refuses `n = ...`
//!    on a parameter ("binding is immutable"), so a fact established on
//!    entry holds for the whole body — including inside loops, where a
//!    fact that could be invalidated later would be unsound (the guard
//!    is emitted once, but the loop runs many times). Facts are
//!    therefore only ever drawn about parameter names, never about
//!    locals, fields, or anything reached through `self`.
//! 2. **The precondition is actually checked.** Elision is skipped
//!    entirely under `--release`, where `requires` clauses are not
//!    emitted: a contract nobody verifies cannot be used to justify
//!    removing a memory-safety check. That makes this the rare
//!    optimisation that is *on* in checked builds and *off* in
//!    unchecked ones, which is the direction that keeps a false
//!    contract from turning into an out-of-bounds read.
//!
//! A name re-bound by a `val` / `var` in the body drops its facts
//! (`shadowed`), since the guard site would then be reading the new
//! binding rather than the parameter.

use std::collections::{HashMap, HashSet};

use frontend::ast::{Expr, ExprRef, File, Operator, ParameterList, Stmt, StmtRef, UnaryOp};
use string_interner::{DefaultStringInterner, DefaultSymbol};

#[derive(Default, Clone)]
pub(super) struct ContractFacts {
    /// Parameters some clause proved to be non-zero.
    nonzero: HashSet<DefaultSymbol>,
    /// Ordered pairs `(a, b)` where some clause proved `a >= b`.
    at_least: HashSet<(DefaultSymbol, DefaultSymbol)>,
    /// Parameters some clause bounded from above: `x < N` records `N`,
    /// `x <= N` records `N + 1`. The value is the first index the
    /// parameter cannot take, which is exactly what an array bounds
    /// check compares against.
    below: HashMap<DefaultSymbol, u128>,
    /// Parameters some clause proved `>= 0` (signed). Drives the
    /// signed index elision, whose negative-adjustment path would
    /// otherwise stay live, and the signed `MIN / -1` guard, whose
    /// `lhs == MIN` half a non-negative lhs can never satisfy.
    nonneg: HashSet<DefaultSymbol>,
    /// Parameters some clause proved `!= -1` (signed). Kills the
    /// `rhs == -1` half of the `MIN / -1` guard outright.
    not_minus_one: HashSet<DefaultSymbol>,
}

impl ContractFacts {
    /// Read the facts out of `requires`. Only clauses that speak about
    /// a parameter by name contribute; everything else is ignored,
    /// which costs a guard that could have gone but never removes one
    /// that was needed.
    pub(super) fn from_requires(
        program: &File,
        interner: &DefaultStringInterner,
        requires: &[ExprRef],
        parameters: &ParameterList,
    ) -> Self {
        let params: HashSet<DefaultSymbol> = parameters.iter().map(|(sym, _)| *sym).collect();
        let mut facts = ContractFacts::default();
        if params.is_empty() {
            return facts;
        }
        let allow = Allow::Only(&params);
        for clause in requires {
            facts.collect(program, interner, clause, &allow, false);
        }
        // Transitive closure (CONTRACT-ELISION 残 (c)): the clauses
        // above are one level deep — `a >= b` elides the guard of
        // `a - b` only. Chaining them makes the facts reach further:
        //
        //   a >= b  and  b >= c    →  a >= c     (guard on `a - c`)
        //   a >= b  and  b >= 0    →  a >= 0     (signed index / MIN)
        //   a >= b  and  a < N     →  b < N      (bounds guard on
        //                                         `arr[b]`)
        facts.close();
        facts
    }

    /// Add what a branch condition states about the code it guards.
    ///
    /// `negated` reads the condition the other way round, which is how
    /// the `else` branch of `if b == 0u64 { ... }` learns that `b` is
    /// non-zero. `mutated` names everything the guarded code assigns:
    /// a fact about one of those would be true on entry and stale by
    /// the time the guard site reads it, and a wrongly elided guard is
    /// an unchecked division rather than a missed optimisation.
    pub(super) fn learn_condition(
        &mut self,
        program: &File,
        interner: &DefaultStringInterner,
        cond: &ExprRef,
        negated: bool,
        mutated: &HashSet<DefaultSymbol>,
    ) {
        self.collect(program, interner, cond, &Allow::Except(mutated), negated);
        self.close();
    }

    /// Add what `for var in start..end` states inside the loop body.
    ///
    /// The range is half-open in both spellings (`..` and `to` lower to
    /// the same `i < end` header), so a literal `end` is exactly the
    /// bound an index guard tests. The induction variable cannot be
    /// assigned — the type checker refuses it, the same immutability
    /// that makes a parameter's contract facts hold — so the only way
    /// it can change under the facts is a `val` / `var` in the body
    /// shadowing the name.
    pub(super) fn learn_range(
        &mut self,
        program: &File,
        interner: &DefaultStringInterner,
        var: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        mutated: &HashSet<DefaultSymbol>,
    ) {
        if mutated.contains(&var) {
            return;
        }
        if let Some(from) = integer_literal_value(program, interner, start) {
            if from >= 0 {
                self.nonneg.insert(var);
            }
            if from > 0 {
                self.nonzero.insert(var);
            }
        }
        if let Some(to) = integer_literal_value(program, interner, end)
            && let Ok(limit) = u128::try_from(to)
        {
            self.record_below(var, limit);
        }
        self.close();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nonzero.is_empty()
            && self.at_least.is_empty()
            && self.below.is_empty()
            && self.nonneg.is_empty()
            && self.not_minus_one.is_empty()
    }

    /// Whether a clause proved `sym < limit` — or better.
    pub(super) fn is_below(&self, sym: DefaultSymbol, limit: u128) -> bool {
        self.below.get(&sym).is_some_and(|bound| *bound <= limit)
    }

    /// Whether `sym` is a parameter a clause proved non-zero.
    pub(super) fn is_nonzero(&self, sym: DefaultSymbol) -> bool {
        self.nonzero.contains(&sym)
    }

    /// Whether a clause proved `a >= b`.
    pub(super) fn is_at_least(&self, a: DefaultSymbol, b: DefaultSymbol) -> bool {
        self.at_least.contains(&(a, b))
    }

    /// Whether a clause (or a chain of `>=` facts) proved `sym >= 0`.
    pub(super) fn is_nonneg(&self, sym: DefaultSymbol) -> bool {
        self.nonneg.contains(&sym)
    }

    /// Whether `sym` can be shown `!= -1`: either directly, or by
    /// being non-negative (which -1 is not).
    pub(super) fn is_not_minus_one(&self, sym: DefaultSymbol) -> bool {
        self.nonneg.contains(&sym) || self.not_minus_one.contains(&sym)
    }

    /// Forget everything known about `sym` — a `val` / `var` in the
    /// body re-bound the name, so a guard site reading it is no longer
    /// reading the parameter the contract spoke about.
    pub(super) fn shadowed(&mut self, sym: DefaultSymbol) {
        if self.is_empty() {
            return;
        }
        self.nonzero.remove(&sym);
        self.below.remove(&sym);
        self.nonneg.remove(&sym);
        self.not_minus_one.remove(&sym);
        self.at_least.retain(|(a, b)| *a != sym && *b != sym);
    }

    /// Keep the tightest bound seen; two clauses about the same
    /// parameter are both true. Returns whether the bound changed,
    /// which the transitive closure uses to detect progress.
    fn record_below(&mut self, sym: DefaultSymbol, limit: u128) -> bool {
        match self.below.entry(sym) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if limit < *e.get() {
                    e.insert(limit);
                    true
                } else {
                    false
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(limit);
                true
            }
        }
    }

    /// Extend the collected facts with everything they imply
    /// transitively. Fixpoint over the three fact sets, so the order
    /// clauses were written in does not matter.
    fn close(&mut self) {
        loop {
            let mut changed = false;
            // a >= b and b >= c → a >= c
            for (a, b) in self.at_least.iter().copied().collect::<Vec<_>>() {
                for (b2, c) in self.at_least.iter().copied().collect::<Vec<_>>() {
                    if b2 == b && self.at_least.insert((a, c)) {
                        changed = true;
                    }
                }
            }
            // a >= b and b >= 0 → a >= 0
            for (a, b) in self.at_least.iter().copied().collect::<Vec<_>>() {
                if self.nonneg.contains(&b) && self.nonneg.insert(a) {
                    changed = true;
                }
            }
            // a >= b and a < N → b < N
            for (a, b) in self.at_least.iter().copied().collect::<Vec<_>>() {
                if let Some(limit) = self.below.get(&a).copied()
                    && self.record_below(b, limit)
                {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn collect(
        &mut self,
        program: &File,
        interner: &DefaultStringInterner,
        clause: &ExprRef,
        allow: &Allow<'_>,
        negated: bool,
    ) {
        let Some(expr) = program.expression.get(clause) else {
            return;
        };
        match expr {
            // `!e` is the same reading with the sense flipped, which is
            // how an `else` branch learns from its `if`.
            Expr::Unary(UnaryOp::LogicalNot, inner) => {
                self.collect(program, interner, &inner, allow, !negated);
            }
            // `a && b` gives both halves; `||` gives neither, since
            // either side alone may be the one that held. Negated, De
            // Morgan swaps which is which: `!(a || b)` is `!a && !b`.
            Expr::Binary(Operator::LogicalAnd, lhs, rhs) if !negated => {
                self.collect(program, interner, &lhs, allow, false);
                self.collect(program, interner, &rhs, allow, false);
            }
            Expr::Binary(Operator::LogicalOr, lhs, rhs) if negated => {
                self.collect(program, interner, &lhs, allow, true);
                self.collect(program, interner, &rhs, allow, true);
            }
            Expr::Binary(Operator::LogicalAnd | Operator::LogicalOr, _, _) => {}
            Expr::Binary(op, lhs, rhs) => {
                let Some(op) = (if negated { negate(op) } else { Some(op) }) else {
                    return;
                };
                let left = ident_of(program, &lhs).filter(|s| allow.permits(*s));
                let right = ident_of(program, &rhs).filter(|s| allow.permits(*s));
                let left_zero = is_literal(program, interner, &lhs, 0);
                let right_zero = is_literal(program, interner, &rhs, 0);
                let right_one = is_literal(program, interner, &rhs, 1);
                match op {
                    // x != 0 / 0 != x
                    Operator::NE => {
                        if right_zero && let Some(sym) = left {
                            self.nonzero.insert(sym);
                        }
                        if left_zero && let Some(sym) = right {
                            self.nonzero.insert(sym);
                        }
                        // `x != -1` / `-1 != x` rules out the rhs half
                        // of the signed `MIN / -1` guard.
                        if is_literal(program, interner, &rhs, -1) && let Some(sym) = left {
                            self.not_minus_one.insert(sym);
                        }
                        if is_literal(program, interner, &lhs, -1) && let Some(sym) = right {
                            self.not_minus_one.insert(sym);
                        }
                    }
                    // x > 0  (unsigned and signed alike: not zero —
                    // and for signed, >= 1, so also non-negative)
                    Operator::GT => {
                        if right_zero && let Some(sym) = left {
                            self.nonzero.insert(sym);
                            self.nonneg.insert(sym);
                        }
                        // `x > -1` ⟺ `x >= 0` for integers
                        if is_literal(program, interner, &rhs, -1) && let Some(sym) = left {
                            self.nonneg.insert(sym);
                        }
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((a, b));
                        }
                    }
                    // x >= 1 proves non-zero for unsigned and signed
                    // alike (and >= 0); `x >= 0` proves non-negativity
                    // outright; `a >= b` is the subtraction fact.
                    Operator::GE => {
                        if right_one && let Some(sym) = left {
                            self.nonzero.insert(sym);
                            self.nonneg.insert(sym);
                        }
                        if right_zero && let Some(sym) = left {
                            self.nonneg.insert(sym);
                        }
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((a, b));
                        }
                    }
                    // Mirror images of the two above, plus the upper
                    // bound an array index needs: `i < 8u64` says the
                    // index cannot reach 8, which is the same test the
                    // bounds guard would emit.
                    Operator::LT => {
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((b, a));
                        }
                        if let (Some(sym), Some(limit)) =
                            (left, integer_literal_value(program, interner, &rhs))
                            && let Ok(limit) = u128::try_from(limit)
                        {
                            self.record_below(sym, limit);
                        }
                        // `-1 < x` ⟺ `x >= 0`
                        if is_literal(program, interner, &lhs, -1) && let Some(sym) = right {
                            self.nonneg.insert(sym);
                        }
                        // `0 < x` — the mirror of `x > 0`
                        if is_literal(program, interner, &lhs, 0) && let Some(sym) = right {
                            self.nonzero.insert(sym);
                            self.nonneg.insert(sym);
                        }
                    }
                    Operator::LE => {
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((b, a));
                        }
                        if let (Some(sym), Some(limit)) =
                            (left, integer_literal_value(program, interner, &rhs))
                            && let Ok(limit) = u128::try_from(limit)
                        {
                            self.record_below(sym, limit + 1);
                        }
                        // `0 <= x` ⟺ `x >= 0`; `1 <= x` the mirror of
                        // `x >= 1`
                        if is_literal(program, interner, &lhs, 0) && let Some(sym) = right {
                            self.nonneg.insert(sym);
                        }
                        if is_literal(program, interner, &lhs, 1) && let Some(sym) = right {
                            self.nonzero.insert(sym);
                            self.nonneg.insert(sym);
                        }
                    }
                    // `x == 5u64` says everything a bound could:
                    // the exact value. Reached by an `if x == 0u64`
                    // condition far more often than by a contract.
                    Operator::EQ => {
                        if let (Some(sym), Some(value)) =
                            (left, integer_literal_value(program, interner, &rhs))
                        {
                            self.record_equal(sym, value);
                        }
                        if let (Some(sym), Some(value)) =
                            (right, integer_literal_value(program, interner, &lhs))
                        {
                            self.record_equal(sym, value);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Everything a known value implies. Used for `x == <literal>`,
    /// which an `if` condition states outright.
    fn record_equal(&mut self, sym: DefaultSymbol, value: i128) {
        if value != 0 {
            self.nonzero.insert(sym);
        }
        if value >= 0 {
            self.nonneg.insert(sym);
            if let Ok(limit) = u128::try_from(value) {
                self.record_below(sym, limit + 1);
            }
        }
        if value != -1 {
            self.not_minus_one.insert(sym);
        }
    }
}

/// Which names a reading may draw facts about.
///
/// `requires` speaks about parameters, and only parameters: they are
/// immutable, so a fact drawn on entry holds for the whole body. A
/// branch or loop condition speaks about whatever is in scope, and
/// those *can* change — so there the rule is the other way round, and
/// the caller names what the guarded code assigns.
pub(super) enum Allow<'a> {
    Only(&'a HashSet<DefaultSymbol>),
    Except(&'a HashSet<DefaultSymbol>),
}

impl Allow<'_> {
    fn permits(&self, sym: DefaultSymbol) -> bool {
        match self {
            Allow::Only(set) => set.contains(&sym),
            Allow::Except(set) => !set.contains(&sym),
        }
    }
}

/// The comparison that holds when `op` does not.
fn negate(op: Operator) -> Option<Operator> {
    Some(match op {
        Operator::EQ => Operator::NE,
        Operator::NE => Operator::EQ,
        Operator::LT => Operator::GE,
        Operator::LE => Operator::GT,
        Operator::GT => Operator::LE,
        Operator::GE => Operator::LT,
        _ => return None,
    })
}

fn ident_of(program: &File, expr: &ExprRef) -> Option<DefaultSymbol> {
    match program.expression.get(expr)? {
        Expr::Identifier(sym) => Some(sym),
        _ => None,
    }
}

/// Whether `expr` is the integer literal `value`, at any width and
/// including the unsuffixed `Number` form (`requires b != 0` is what
/// people write; `0u64` is the exception).
fn is_literal(
    program: &File,
    interner: &DefaultStringInterner,
    expr: &ExprRef,
    value: i128,
) -> bool {
    integer_literal_value(program, interner, expr) == Some(value)
}

fn integer_literal_value(
    program: &File,
    interner: &DefaultStringInterner,
    expr: &ExprRef,
) -> Option<i128> {
    match program.expression.get(expr)? {
        Expr::UInt64(v) => Some(v as i128),
        Expr::Int64(v) => Some(v as i128),
        Expr::UInt8(v) => Some(v as i128),
        Expr::UInt16(v) => Some(v as i128),
        Expr::UInt32(v) | Expr::CharLiteral(v) => Some(v as i128),
        Expr::Int8(v) => Some(v as i128),
        Expr::Int16(v) => Some(v as i128),
        Expr::Int32(v) => Some(v as i128),
        Expr::Number(sym) => interner.resolve(sym)?.parse::<i128>().ok(),
        // `- 1i64` with a space between the minus and the literal
        // parses as unary negation rather than a folded literal; peel
        // it so `requires b != - 1i64` reads like `b != -1i64`.
        Expr::Unary(UnaryOp::Negate, inner) => {
            integer_literal_value(program, interner, &inner).map(i128::wrapping_neg)
        }
        _ => None,
    }
}

/// Every name the code in `body` may write to.
///
/// Deliberately blunt: an assignment, a `val` / `var` that re-binds the
/// name, a `&mut` borrow, and any method call on a bare name all count,
/// even where the call could not possibly write. Being wrong this way
/// costs an optimisation; being wrong the other way removes a guard
/// that was doing something.
pub(super) fn mutated_names(program: &File, body: &ExprRef) -> HashSet<DefaultSymbol> {
    let mut found = HashSet::new();
    walk_expr(program, body, &mut found);
    found
}

fn note_root(program: &File, expr: &ExprRef, out: &mut HashSet<DefaultSymbol>) {
    match program.expression.get(expr) {
        Some(Expr::Identifier(sym)) => {
            out.insert(sym);
        }
        Some(Expr::FieldAccess(obj, _)) | Some(Expr::TupleAccess(obj, _)) => {
            note_root(program, &obj, out)
        }
        Some(Expr::SliceAccess(obj, _)) => note_root(program, &obj, out),
        _ => {}
    }
}

fn walk_expr(program: &File, expr: &ExprRef, out: &mut HashSet<DefaultSymbol>) {
    let Some(node) = program.expression.get(expr) else {
        return;
    };
    match node {
        Expr::Assign(lhs, rhs) => {
            note_root(program, &lhs, out);
            walk_expr(program, &lhs, out);
            walk_expr(program, &rhs, out);
        }
        Expr::Unary(UnaryOp::BorrowMut, inner) => {
            note_root(program, &inner, out);
            walk_expr(program, &inner, out);
        }
        Expr::MethodCall(receiver, _, args) => {
            note_root(program, &receiver, out);
            walk_expr(program, &receiver, out);
            for arg in &args {
                walk_expr(program, arg, out);
            }
        }
        Expr::SliceAssign(obj, start, end, value) => {
            note_root(program, &obj, out);
            walk_expr(program, &obj, out);
            for part in [start, end].into_iter().flatten() {
                walk_expr(program, &part, out);
            }
            walk_expr(program, &value, out);
        }
        Expr::Block(stmts) => {
            for stmt in &stmts {
                walk_stmt(program, stmt, out);
            }
        }
        Expr::Binary(_, lhs, rhs) | Expr::Range(lhs, rhs) | Expr::With(lhs, rhs) => {
            walk_expr(program, &lhs, out);
            walk_expr(program, &rhs, out);
        }
        Expr::Unary(_, inner)
        | Expr::Cast(inner, _)
        | Expr::FieldAccess(inner, _)
        | Expr::TupleAccess(inner, _) => walk_expr(program, &inner, out),
        Expr::IfElifElse(cond, then_body, elifs, else_body) => {
            walk_expr(program, &cond, out);
            walk_expr(program, &then_body, out);
            for (c, b) in &elifs {
                walk_expr(program, c, out);
                walk_expr(program, b, out);
            }
            walk_expr(program, &else_body, out);
        }
        Expr::Match(scrutinee, arms) => {
            walk_expr(program, &scrutinee, out);
            for arm in &arms {
                if let Some(guard) = arm.guard {
                    walk_expr(program, &guard, out);
                }
                walk_expr(program, &arm.body, out);
            }
        }
        Expr::Call(_, args) => walk_expr(program, &args, out),
        Expr::AssociatedFunctionCall(_, _, args)
        | Expr::ExprList(args)
        | Expr::ArrayLiteral(args)
        | Expr::TupleLiteral(args) => {
            for arg in &args {
                walk_expr(program, arg, out);
            }
        }
        Expr::BuiltinCall(_, args) => {
            for arg in &args {
                walk_expr(program, arg, out);
            }
        }
        Expr::BuiltinMethodCall(receiver, _, args) => {
            walk_expr(program, &receiver, out);
            for arg in &args {
                walk_expr(program, arg, out);
            }
        }
        Expr::StructLiteral(_, fields) => {
            for (_, value) in &fields {
                walk_expr(program, value, out);
            }
        }
        Expr::DictLiteral(entries) => {
            for (k, v) in &entries {
                walk_expr(program, k, out);
                walk_expr(program, v, out);
            }
        }
        Expr::SliceAccess(obj, info) => {
            walk_expr(program, &obj, out);
            for part in [info.start, info.end].into_iter().flatten() {
                walk_expr(program, &part, out);
            }
        }
        // A closure body runs somewhere this walk cannot place, so
        // anything it touches is assumed written.
        Expr::Closure { body, .. } => {
            let mut inner = HashSet::new();
            walk_expr(program, &body, &mut inner);
            out.extend(inner);
            collect_identifiers(program, &body, out);
        }
        _ => {}
    }
}

fn walk_stmt(program: &File, stmt: &StmtRef, out: &mut HashSet<DefaultSymbol>) {
    let Some(node) = program.statement.get(stmt) else {
        return;
    };
    match node {
        // A re-binding makes the name mean something else from here on.
        Stmt::Val(name, _, value) => {
            out.insert(name);
            walk_expr(program, &value, out);
        }
        Stmt::Var(name, _, value) => {
            out.insert(name);
            if let Some(value) = value {
                walk_expr(program, &value, out);
            }
        }
        Stmt::Expression(e) => walk_expr(program, &e, out),
        Stmt::Return(e) => {
            if let Some(e) = e {
                walk_expr(program, &e, out);
            }
        }
        Stmt::For(_, var, start, end, body) => {
            out.insert(var);
            walk_expr(program, &start, out);
            walk_expr(program, &end, out);
            walk_expr(program, &body, out);
        }
        Stmt::While(_, cond, body) => {
            walk_expr(program, &cond, out);
            walk_expr(program, &body, out);
        }
        _ => {}
    }
}

/// Every name mentioned in `expr`. Used only for a closure body, where
/// "mentioned" is as close as this walk gets to "may be written".
fn collect_identifiers(program: &File, expr: &ExprRef, out: &mut HashSet<DefaultSymbol>) {
    if let Some(Expr::Identifier(sym)) = program.expression.get(expr) {
        out.insert(sym);
    }
    let mut nested = HashSet::new();
    walk_expr(program, expr, &mut nested);
    out.extend(nested);
}
