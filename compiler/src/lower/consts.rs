//! Compile-time evaluation of top-level `const` initialisers.
//!
//! Each `const NAME: Type = expr` is folded into an IR `Const` value
//! once per program; the resulting `ConstValues` map is consulted by
//! body lowering when a bare identifier resolves to a top-level
//! constant rather than a local binding. The fold supports literal
//! values, references to earlier consts, simple unary/binary
//! arithmetic, and logical-not on bool — anything else is rejected
//! with a clear error so the user knows to bind the value at the
//! function level instead.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, Program};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::Const;

pub(super) type ConstValues = HashMap<DefaultSymbol, Const>;

pub(super) fn evaluate_consts(
    program: &Program,
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

fn eval_const_expr(
    expr_ref: &ExprRef,
    program: &Program,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    let _ = interner;
    match program.expression.get(expr_ref)? {
        Expr::Int64(v) => Some(Const::I64(v)),
        Expr::UInt64(v) => Some(Const::U64(v)),
        Expr::Float64(v) => Some(Const::F64(v)),
        Expr::True => Some(Const::Bool(true)),
        Expr::False => Some(Const::Bool(false)),
        Expr::Identifier(sym) => values.get(&sym).copied(),
        // Fold simple arithmetic / comparison so initialisers like
        // `const TWO_PI: f64 = PI + PI` work. Unsupported operators
        // bubble `None` up, which the caller turns into a compile
        // error.
        Expr::Binary(op, lhs, rhs) => {
            let l = eval_const_expr(&lhs, program, values, interner)?;
            let r = eval_const_expr(&rhs, program, values, interner)?;
            const_fold_binop(op, l, r)
        }
        Expr::Unary(op, operand) => {
            let v = eval_const_expr(&operand, program, values, interner)?;
            match (op, v) {
                (frontend::ast::UnaryOp::Negate, Const::I64(n)) => Some(Const::I64(-n)),
                (frontend::ast::UnaryOp::Negate, Const::F64(n)) => Some(Const::F64(-n)),
                (frontend::ast::UnaryOp::LogicalNot, Const::Bool(b)) => Some(Const::Bool(!b)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn const_fold_binop(op: frontend::ast::Operator, l: Const, r: Const) -> Option<Const> {
    use frontend::ast::Operator;
    match (l, r) {
        (Const::I64(a), Const::I64(b)) => match op {
            Operator::IAdd => Some(Const::I64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::I64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::I64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::I64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::I64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::U64(a), Const::U64(b)) => match op {
            Operator::IAdd => Some(Const::U64(a.wrapping_add(b))),
            Operator::ISub => Some(Const::U64(a.wrapping_sub(b))),
            Operator::IMul => Some(Const::U64(a.wrapping_mul(b))),
            Operator::IDiv if b != 0 => Some(Const::U64(a.wrapping_div(b))),
            Operator::IMod if b != 0 => Some(Const::U64(a.wrapping_rem(b))),
            _ => None,
        },
        (Const::F64(a), Const::F64(b)) => match op {
            Operator::IAdd => Some(Const::F64(a + b)),
            Operator::ISub => Some(Const::F64(a - b)),
            Operator::IMul => Some(Const::F64(a * b)),
            Operator::IDiv => Some(Const::F64(a / b)),
            _ => None,
        },
        _ => None,
    }
}
