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

use std::collections::HashSet;

use frontend::ast::{Expr, ExprRef, File, Operator, ParameterList};
use string_interner::{DefaultStringInterner, DefaultSymbol};

#[derive(Default)]
pub(super) struct ContractFacts {
    /// Parameters some clause proved to be non-zero.
    nonzero: HashSet<DefaultSymbol>,
    /// Ordered pairs `(a, b)` where some clause proved `a >= b`.
    at_least: HashSet<(DefaultSymbol, DefaultSymbol)>,
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
        for clause in requires {
            facts.collect(program, interner, clause, &params);
        }
        facts
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nonzero.is_empty() && self.at_least.is_empty()
    }

    /// Whether `sym` is a parameter a clause proved non-zero.
    pub(super) fn is_nonzero(&self, sym: DefaultSymbol) -> bool {
        self.nonzero.contains(&sym)
    }

    /// Whether a clause proved `a >= b`.
    pub(super) fn is_at_least(&self, a: DefaultSymbol, b: DefaultSymbol) -> bool {
        self.at_least.contains(&(a, b))
    }

    /// Forget everything known about `sym` — a `val` / `var` in the
    /// body re-bound the name, so a guard site reading it is no longer
    /// reading the parameter the contract spoke about.
    pub(super) fn shadowed(&mut self, sym: DefaultSymbol) {
        if self.is_empty() {
            return;
        }
        self.nonzero.remove(&sym);
        self.at_least.retain(|(a, b)| *a != sym && *b != sym);
    }

    fn collect(
        &mut self,
        program: &File,
        interner: &DefaultStringInterner,
        clause: &ExprRef,
        params: &HashSet<DefaultSymbol>,
    ) {
        let Some(expr) = program.expression.get(clause) else {
            return;
        };
        match expr {
            // `a && b` gives both halves; `||` gives neither, since
            // either side alone may be the one that held.
            Expr::Binary(Operator::LogicalAnd, lhs, rhs) => {
                self.collect(program, interner, &lhs, params);
                self.collect(program, interner, &rhs, params);
            }
            Expr::Binary(op, lhs, rhs) => {
                let left = ident_of(program, &lhs).filter(|s| params.contains(s));
                let right = ident_of(program, &rhs).filter(|s| params.contains(s));
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
                    }
                    // x > 0  (unsigned and signed alike: not zero)
                    Operator::GT => {
                        if right_zero && let Some(sym) = left {
                            self.nonzero.insert(sym);
                        }
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((a, b));
                        }
                    }
                    // x >= 1 proves non-zero for unsigned and signed
                    // alike; `a >= b` is the subtraction fact.
                    Operator::GE => {
                        if right_one && let Some(sym) = left {
                            self.nonzero.insert(sym);
                        }
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((a, b));
                        }
                    }
                    // Mirror images of the two above.
                    Operator::LT => {
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((b, a));
                        }
                    }
                    Operator::LE => {
                        if let (Some(a), Some(b)) = (left, right) {
                            self.at_least.insert((b, a));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
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
        Expr::UInt32(v) => Some(v as i128),
        Expr::Int8(v) => Some(v as i128),
        Expr::Int16(v) => Some(v as i128),
        Expr::Int32(v) => Some(v as i128),
        Expr::Number(sym) => interner.resolve(sym)?.parse::<i128>().ok(),
        _ => None,
    }
}
