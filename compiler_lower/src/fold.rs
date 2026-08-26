//! COMPILE-TIME-EVAL C2: constant folding inside a basic block.
//!
//! `2u64 * 3u64 + 1u64` used to lower to five instructions and get
//! computed on every run. Native code hid it — cranelift folds this
//! itself — but the IR VM is the interpreter's default engine and it
//! really did the multiply, and the test suite builds with
//! `TOYLANG_CRANELIFT_OPT_LEVEL=none`, so the "the optimiser handles
//! it" answer was true on none of the paths that matter here.
//!
//! ## The rule this obeys
//!
//! **Folding may not change what a program does.** Two consequences,
//! both visible below:
//!
//! - Where the language wraps (`+`, `-` on signed, `*`), the fold
//!   wraps, using the host's `wrapping_*`. Rust's checked arithmetic
//!   would abort the compiler on a program that is merely required to
//!   produce a wrapped answer.
//! - Where the language traps (`u64` underflow, division by zero,
//!   `MIN / -1`, an over-wide shift), the fold **declines**. The
//!   guard RUNTIME-TRAP already emitted stays, and the program traps
//!   at run time exactly as before. Turning it into a compile error
//!   instead would be wrong for the same reason `if false { 1u64 /
//!   0u64 }` has to stay legal: this pass knows nothing about
//!   reachability, and a block it folds may never run.
//!
//! Anything reached through libm is left alone — `pow`, `sqrt`. The
//! language compiles for the host today, so nobody can observe a
//! difference between the compiler's libm and the target's; baking
//! constants in is how that difference gets created later
//! (`COMPILE_TIME_EVAL.md` 論点 3).

use compiler_ir::{BinOp, Const, UnaryOp};
use frontend::ast::Operator;

/// The IR opcode an AST operator lowers to. `None` for the
/// short-circuiting pair, which is control flow rather than an
/// operation.
///
/// One table, so the const-initialiser fold and body lowering cannot
/// disagree about what `%` means.
pub(crate) fn binop_for(op: &Operator) -> Option<BinOp> {
    Some(match op {
        Operator::IAdd => BinOp::Add,
        Operator::ISub => BinOp::Sub,
        Operator::IMul => BinOp::Mul,
        Operator::IDiv => BinOp::Div,
        Operator::IMod => BinOp::Rem,
        Operator::EQ => BinOp::Eq,
        Operator::NE => BinOp::Ne,
        Operator::LT => BinOp::Lt,
        Operator::LE => BinOp::Le,
        Operator::GT => BinOp::Gt,
        Operator::GE => BinOp::Ge,
        Operator::BitwiseAnd => BinOp::BitAnd,
        Operator::BitwiseOr => BinOp::BitOr,
        Operator::BitwiseXor => BinOp::BitXor,
        Operator::LeftShift => BinOp::Shl,
        Operator::RightShift => BinOp::Shr,
        Operator::LogicalAnd | Operator::LogicalOr => return None,
    })
}

/// The IR opcode an AST unary operator lowers to.
pub(crate) fn unaryop_for(op: &frontend::ast::UnaryOp) -> Option<UnaryOp> {
    Some(match op {
        frontend::ast::UnaryOp::Negate => UnaryOp::Neg,
        frontend::ast::UnaryOp::BitwiseNot => UnaryOp::BitNot,
        frontend::ast::UnaryOp::LogicalNot => UnaryOp::LogicalNot,
        // `&expr` / `&mut expr` are erased before lowering.
        frontend::ast::UnaryOp::Borrow | frontend::ast::UnaryOp::BorrowMut => return None,
    })
}

/// The constant `op` produces from two constant operands, or `None`
/// when the fold would be unfaithful (a trap, an over-wide shift, a
/// libm call, or mismatched operand types the type checker rules out
/// anyway).
pub(crate) fn fold_binop(op: BinOp, lhs: Const, rhs: Const) -> Option<Const> {
    match (lhs, rhs) {
        (Const::Bool(a), Const::Bool(b)) => fold_bool(op, a, b),
        (Const::F64(a), Const::F64(b)) => fold_f64(op, a, b),
        (Const::I64(a), Const::I64(b)) => signed(op, a, b, i64::MIN, Const::I64),
        (Const::I32(a), Const::I32(b)) => {
            signed(op, a as i64, b as i64, i32::MIN as i64, |v| Const::I32(v as i32))
        }
        (Const::I16(a), Const::I16(b)) => {
            signed(op, a as i64, b as i64, i16::MIN as i64, |v| Const::I16(v as i16))
        }
        (Const::I8(a), Const::I8(b)) => {
            signed(op, a as i64, b as i64, i8::MIN as i64, |v| Const::I8(v as i8))
        }
        (Const::U64(a), Const::U64(b)) => unsigned(op, a, b, 64, Const::U64),
        (Const::U32(a), Const::U32(b)) => unsigned(op, a as u64, b as u64, 32, |v| Const::U32(v as u32)),
        (Const::U16(a), Const::U16(b)) => unsigned(op, a as u64, b as u64, 16, |v| Const::U16(v as u16)),
        (Const::U8(a), Const::U8(b)) => unsigned(op, a as u64, b as u64, 8, |v| Const::U8(v as u8)),
        _ => None,
    }
}

/// The constant `op` produces from one constant operand.
pub(crate) fn fold_unary(op: UnaryOp, operand: Const) -> Option<Const> {
    Some(match (op, operand) {
        (UnaryOp::LogicalNot, Const::Bool(b)) => Const::Bool(!b),
        (UnaryOp::Neg, Const::I64(v)) => Const::I64(v.wrapping_neg()),
        (UnaryOp::Neg, Const::I32(v)) => Const::I32(v.wrapping_neg()),
        (UnaryOp::Neg, Const::I16(v)) => Const::I16(v.wrapping_neg()),
        (UnaryOp::Neg, Const::I8(v)) => Const::I8(v.wrapping_neg()),
        (UnaryOp::Neg, Const::F64(v)) => Const::F64(-v),
        (UnaryOp::BitNot, Const::I64(v)) => Const::I64(!v),
        (UnaryOp::BitNot, Const::I32(v)) => Const::I32(!v),
        (UnaryOp::BitNot, Const::I16(v)) => Const::I16(!v),
        (UnaryOp::BitNot, Const::I8(v)) => Const::I8(!v),
        (UnaryOp::BitNot, Const::U64(v)) => Const::U64(!v),
        (UnaryOp::BitNot, Const::U32(v)) => Const::U32(!v),
        (UnaryOp::BitNot, Const::U16(v)) => Const::U16(!v),
        (UnaryOp::BitNot, Const::U8(v)) => Const::U8(!v),
        // `abs` matches `i64::wrapping_abs`, so `abs(MIN) == MIN`.
        (UnaryOp::Abs, Const::I64(v)) => Const::I64(v.wrapping_abs()),
        // `Sqrt` goes through libm; see the module docs.
        _ => return None,
    })
}

/// Signed integer arithmetic at `i64` width, narrowed by `wrap` on the
/// way out. `min` is the narrow type's most negative value, which is
/// the operand `MIN / -1` traps on.
fn signed(op: BinOp, a: i64, b: i64, min: i64, wrap: fn(i64) -> Const) -> Option<Const> {
    if let Some(answer) = compare(op, a.cmp(&b)) {
        return Some(answer);
    }
    Some(wrap(match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        // RUNTIME-TRAP territory: leave the guard to do its job.
        BinOp::Div if b == 0 || (a == min && b == -1) => return None,
        BinOp::Rem if b == 0 || (a == min && b == -1) => return None,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Shl | BinOp::Shr => return None,
        BinOp::Min => a.min(b),
        BinOp::Max => a.max(b),
        _ => return None,
    }))
}

/// Unsigned integer arithmetic at `u64` width, narrowed by `wrap`.
/// `bits` is the narrow type's width, which bounds a legal shift.
fn unsigned(op: BinOp, a: u64, b: u64, bits: u32, wrap: fn(u64) -> Const) -> Option<Const> {
    if let Some(answer) = compare(op, a.cmp(&b)) {
        return Some(answer);
    }
    let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    Some(wrap(match op {
        BinOp::Add => a.wrapping_add(b) & mask,
        // Underflow traps, so the guard keeps it.
        BinOp::Sub if b > a => return None,
        BinOp::Sub => a - b,
        BinOp::Mul => a.wrapping_mul(b) & mask,
        BinOp::Div | BinOp::Rem if b == 0 => return None,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        // An over-wide shift has no answer to agree with: the host
        // would panic and the target would do something of its own.
        BinOp::Shl | BinOp::Shr if b >= bits as u64 => return None,
        BinOp::Shl => (a << b) & mask,
        BinOp::Shr => a >> b,
        BinOp::Min => a.min(b),
        BinOp::Max => a.max(b),
        _ => return None,
    }))
}

fn fold_bool(op: BinOp, a: bool, b: bool) -> Option<Const> {
    Some(Const::Bool(match op {
        BinOp::Eq => a == b,
        BinOp::Ne => a != b,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        _ => return None,
    }))
}

fn fold_f64(op: BinOp, a: f64, b: f64) -> Option<Const> {
    Some(match op {
        BinOp::Add => Const::F64(a + b),
        BinOp::Sub => Const::F64(a - b),
        BinOp::Mul => Const::F64(a * b),
        // IEEE 754 division, including the infinities and NaN a
        // zero denominator produces: `f64` has no trap to preserve.
        BinOp::Div => Const::F64(a / b),
        BinOp::Eq => Const::Bool(a == b),
        BinOp::Ne => Const::Bool(a != b),
        BinOp::Lt => Const::Bool(a < b),
        BinOp::Le => Const::Bool(a <= b),
        BinOp::Gt => Const::Bool(a > b),
        BinOp::Ge => Const::Bool(a >= b),
        // `Rem` on f64 is not part of the language, and `Pow` is libm.
        _ => return None,
    })
}

/// The comparison operators, shared by both integer widths.
fn compare(op: BinOp, ordering: std::cmp::Ordering) -> Option<Const> {
    use std::cmp::Ordering::*;
    Some(Const::Bool(match op {
        BinOp::Eq => ordering == Equal,
        BinOp::Ne => ordering != Equal,
        BinOp::Lt => ordering == Less,
        BinOp::Le => ordering != Greater,
        BinOp::Gt => ordering == Greater,
        BinOp::Ge => ordering != Less,
        _ => return None,
    }))
}

/// Drop the constants nothing reads.
///
/// Folding leaves the operands behind: `2u64 * 3u64` becomes
/// `const 2`, `const 3`, `const 6`, of which only the last matters.
/// They are harmless on the compiled backends (cranelift drops an
/// unused `iconst`) but the IR VM really executes each one, and IR
/// dumps are read by people.
///
/// Only `Const` instructions are removed, and only when no
/// instruction and no terminator anywhere in the function names their
/// result. Liveness comes from [`compiler_ir::InstKind::for_each_operand`],
/// whose match is exhaustive precisely so that a new operand-carrying
/// variant cannot silently make this delete a live definition.
pub(crate) fn drop_dead_consts(function: &mut compiler_ir::Function) {
    let mut used: std::collections::HashSet<compiler_ir::ValueId> =
        std::collections::HashSet::new();
    for block in &function.blocks {
        for inst in &block.instructions {
            inst.kind.for_each_operand(&mut |v| {
                used.insert(v);
            });
        }
        if let Some(terminator) = &block.terminator {
            terminator.for_each_operand(&mut |v| {
                used.insert(v);
            });
        }
    }
    for block in &mut function.blocks {
        block.instructions.retain(|inst| match (&inst.kind, inst.result) {
            (compiler_ir::InstKind::Const(_), Some((value, _))) => used.contains(&value),
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Const` deliberately has no `PartialEq` — bit-equality on the
    /// `F64` variant is not a comparison the IR layer wants anyone
    /// making. Comparing the `Debug` form is enough for these
    /// assertions and leaves that decision where it is.
    #[track_caller]
    fn assert_folds(actual: Option<Const>, expected: Option<Const>) {
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }

    /// Wrapping is the language's answer for `+` / `-` (signed) / `*`,
    /// in every build profile, so it has to be the fold's answer too.
    #[test]
    fn arithmetic_wraps_at_the_operand_width() {
        assert_folds(fold_binop(BinOp::Add, Const::U64(u64::MAX), Const::U64(3)), Some(Const::U64(2)));
        assert_folds(fold_binop(BinOp::Mul, Const::U8(200), Const::U8(3)), Some(Const::U8(88)));
        assert_folds(fold_binop(BinOp::Add, Const::I8(100), Const::I8(100)), Some(Const::I8(-56)));
        assert_folds(fold_binop(BinOp::Mul, Const::I64(i64::MAX), Const::I64(2)), Some(Const::I64(-2)));
    }

    /// Where the language traps, the fold declines and leaves
    /// RUNTIME-TRAP's guard to do its job. Answering here would either
    /// abort the compiler (Rust's checked arithmetic) or invent a
    /// value the program never produces.
    #[test]
    fn the_fold_declines_where_the_language_traps() {
        assert_folds(fold_binop(BinOp::Sub, Const::U64(3), Const::U64(5)), None);
        assert_folds(fold_binop(BinOp::Div, Const::U64(1), Const::U64(0)), None);
        assert_folds(fold_binop(BinOp::Rem, Const::I64(1), Const::I64(0)), None);
        assert_folds(fold_binop(BinOp::Div, Const::I64(i64::MIN), Const::I64(-1)), None);
        assert_folds(fold_binop(BinOp::Div, Const::I8(i8::MIN), Const::I8(-1)), None);
        // An over-wide shift has no answer the target would agree
        // with, and the host would panic working one out.
        assert_folds(fold_binop(BinOp::Shl, Const::U64(1), Const::U64(64)), None);
        assert_folds(fold_binop(BinOp::Shr, Const::U8(1), Const::U8(8)), None);
    }

    /// Signed division truncates toward zero, and the remainder takes
    /// the dividend's sign: `-7 / 3 == -2`, `-7 % 3 == -1`.
    #[test]
    fn signed_division_truncates() {
        assert_folds(fold_binop(BinOp::Div, Const::I64(-7), Const::I64(3)), Some(Const::I64(-2)));
        assert_folds(fold_binop(BinOp::Rem, Const::I64(-7), Const::I64(3)), Some(Const::I64(-1)));
    }

    /// libm is not consulted at compile time: the language compiles
    /// for the host today, and folding these is how a difference
    /// between the compiler's libm and the target's gets baked in.
    #[test]
    fn libm_backed_operations_are_left_alone() {
        assert_folds(fold_binop(BinOp::Pow, Const::F64(2.0), Const::F64(3.0)), None);
        assert_folds(fold_unary(UnaryOp::Sqrt, Const::F64(4.0)), None);
    }

    /// Mixed widths never reach the backends — the type checker
    /// rejects them — but declining rather than guessing keeps the
    /// fold from being the place that decides.
    #[test]
    fn mismatched_operand_types_are_left_alone() {
        assert_folds(fold_binop(BinOp::Add, Const::U64(1), Const::U8(1)), None);
        assert_folds(fold_binop(BinOp::Add, Const::F64(1.0), Const::I64(1)), None);
    }
}
