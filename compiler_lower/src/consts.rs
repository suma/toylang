//! Compile-time evaluation of top-level `const` initialisers, for the
//! backends that consume the IR.
//!
//! **Mostly historical now.** Since COMPILE-TIME-EVAL C3 the driver
//! evaluates every `const` initialiser on the tree-walker before
//! lowering and rewrites the scalar ones to literals, so what arrives
//! here is normally already a literal. This pass stays because
//! `lower_program` can be called on an AST that did not go through
//! that driver, and because non-scalar initialisers (a `str`) are
//! deliberately left alone by it.
//!
//! What it must **not** do is have arithmetic of its own. Having a
//! second, weaker evaluator here is what made `const D: u64 =
//! double(21u64)` mean 42 on the tree-walker and "cannot evaluate the
//! initialiser" everywhere else, and what made `const X: u64 = 3u64 -
//! 5u64` a run-time trap on one engine and a silently wrapped number
//! on the other three. The operators below therefore delegate to
//! [`crate::fold`], the same table body lowering folds with.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, File};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::Const;

pub type ConstValues = HashMap<DefaultSymbol, Const>;

pub(super) fn evaluate_consts(
    program: &File,
    interner: &DefaultStringInterner,
) -> Result<ConstValues, String> {
    let mut values: ConstValues = HashMap::new();
    for c in &program.consts {
        let v = eval_const_expr(&c.value, program, &values, interner).ok_or_else(|| {
            format!(
                "compiler MVP cannot evaluate the initialiser for `const {}`: only literal values and references to earlier consts are supported",
                interner.resolve(c.name).unwrap_or("?")
            )
        })?;
        // The type-checker has already validated the declared type
        // against the initialiser; we don't re-check here.
        values.insert(c.name, v);
    }
    Ok(values)
}

/// Evaluate an expression to a scalar constant, or `None` when it is
/// not one the compiler can fold: a call, a string, a struct, an
/// identifier that is not an earlier const, or an operation that
/// traps. This is the literal reader COMPILE-TIME-EVAL C3/C6 leaves
/// in place for lowering calls that did not go through the driver —
/// the driver's fold runs the calls on the IR VM and rewrites them to
/// literals first, so what arrives here is normally already flat.
///
/// Also the evaluator behind the driver's array-length resolution
/// (C5): after the fold, a computed length like `double(2u64) + 1u64`
/// is a tree of literals and folded consts, and this turns it into
/// the count.
pub fn eval_const_expr(
    expr_ref: &ExprRef,
    program: &File,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    eval_const_expr_in_pool(expr_ref, &program.expression, values, interner)
}

/// Pool-scoped variant of [`eval_const_expr`], for callers that hold
/// the program's pools apart from its other fields.
pub fn eval_const_expr_in_pool(
    expr_ref: &ExprRef,
    pool: &frontend::ast::ExprPool,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    let _ = interner;
    match pool.get(expr_ref)? {
        Expr::Int64(v) => Some(Const::I64(v)),
        Expr::UInt64(v) => Some(Const::U64(v)),
        Expr::Float64(v) => Some(Const::F64(v)),
        // SIMD-F32: single-precision const initialisers.
        Expr::Float32(v) => Some(Const::F32(v)),
        Expr::True => Some(Const::Bool(true)),
        Expr::False => Some(Const::Bool(false)),
        Expr::Identifier(sym) => values.get(&sym).copied(),
        // Fold simple arithmetic / comparison so initialisers like
        // `const TWO_PI: f64 = PI + PI` work. Unsupported operators
        // bubble `None` up, which the caller turns into a compile
        // error.
        Expr::Binary(op, lhs, rhs) => {
            let l = eval_const_expr_in_pool(&lhs, pool, values, interner)?;
            let r = eval_const_expr_in_pool(&rhs, pool, values, interner)?;
            const_fold_binop(op, l, r)
        }
        Expr::Unary(op, operand) => {
            let v = eval_const_expr_in_pool(&operand, pool, values, interner)?;
            crate::fold::fold_unary(crate::fold::unaryop_for(&op)?, v)
        }
        _ => None,
    }
}

fn const_fold_binop(op: frontend::ast::Operator, l: Const, r: Const) -> Option<Const> {
    crate::fold::fold_binop(crate::fold::binop_for(&op)?, l, r)
}
