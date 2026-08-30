//! SIMD lowering (SIMD.md Phase 2) — the `__simd_*` intrinsics and
//! the result types of lane-wise operators.
//!
//! Lives in `compiler_lower`, so one implementation serves three of
//! the four engines: the IR VM, the AOT compiler, and the compiler's
//! own JIT. The tree-walker has its own (in
//! `interpreter/src/evaluation/simd.rs`), which is what makes it an
//! independent oracle.
//!
//! Lane-wise arithmetic is **not** here: `a * b` on two vectors
//! lowers through the ordinary `InstKind::BinOp` path and codegen
//! picks the vector instruction off the operand type. Only the result
//! type of a comparison needs a word — it is a mask, not a `bool`.

use frontend::ast::{ExprRef, SimdOp};
use frontend::type_decl::VectorType;

use crate::ir::{InstKind, SimdReduceOp, Type, ValueId, VecTy};
use crate::types::vector_to_ir;
use crate::FunctionLower;

impl<'a> FunctionLower<'a> {
    /// The IR type a `__simd_*` call produces, without emitting
    /// anything. Mirrors the type checker's rules.
    pub(super) fn simd_result_type(&self, op: &SimdOp, args: &[ExprRef]) -> Option<Type> {
        match op {
            SimdOp::Splat | SimdOp::Load => self.simd_stamped_type(*op, args).map(Type::Vector),
            SimdOp::Store => Some(Type::Unit),
            SimdOp::Any | SimdOp::All => Some(Type::Bool),
            SimdOp::Insert => self.simd_operand_type(args, 0).map(Type::Vector),
            // `select`'s first argument is the *mask*, whose lane
            // type can differ from the value's (`f64x2`'s mask is
            // `i64x2`). The result has the shape of the values.
            SimdOp::Select => self.simd_operand_type(args, 1).map(Type::Vector),
            SimdOp::Extract
            | SimdOp::ReduceAdd
            | SimdOp::ReduceMin
            | SimdOp::ReduceMax
            | SimdOp::ReduceAnd
            | SimdOp::ReduceOr => self.simd_operand_type(args, 0).map(|v| v.lane()),
        }
    }

    /// Lower one `__simd_*` call.
    pub(super) fn lower_builtin_simd(
        &mut self,
        op: &SimdOp,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        let name = op.builtin_name();
        let stamped = self.simd_stamped_type(*op, args);
        // The stamp rides along as a trailing synthetic argument; the
        // rest of this function only cares about what the user wrote.
        let user_args: &[ExprRef] = if op.needs_result_annotation() && args.len() == op.arity() + 1 {
            &args[..op.arity()]
        } else {
            args
        };
        if user_args.len() != op.arity() {
            return Err(format!(
                "{name} expects {} argument(s), got {}",
                op.arity(),
                user_args.len()
            ));
        }

        match op {
            SimdOp::Splat => {
                let ty = self.simd_require_stamp(*op, stamped)?;
                let value = self.simd_operand_value(name, &user_args[0])?;
                Ok(self.emit(InstKind::SimdSplat { value, ty }, Some(Type::Vector(ty))))
            }
            SimdOp::Load => {
                let ty = self.simd_require_stamp(*op, stamped)?;
                let ptr = self.simd_operand_value(name, &user_args[0])?;
                let offset = self.simd_byte_offset(name, &user_args[1], ty)?;
                Ok(self.emit(InstKind::SimdLoad { ptr, offset, ty }, Some(Type::Vector(ty))))
            }
            SimdOp::Store => {
                let value = self.simd_operand_value(name, &user_args[2])?;
                let ty = self.simd_value_vector_type(name, value)?;
                let ptr = self.simd_operand_value(name, &user_args[0])?;
                let offset = self.simd_byte_offset(name, &user_args[1], ty)?;
                self.emit(InstKind::SimdStore { ptr, offset, value, ty }, None);
                Ok(None)
            }
            SimdOp::Extract => {
                let value = self.simd_operand_value(name, &user_args[0])?;
                let ty = self.simd_value_vector_type(name, value)?;
                let lane = self.simd_lane_literal(name, &user_args[1], ty)?;
                Ok(self.emit(InstKind::SimdExtract { value, lane, ty }, Some(ty.lane())))
            }
            SimdOp::Insert => {
                let value = self.simd_operand_value(name, &user_args[0])?;
                let ty = self.simd_value_vector_type(name, value)?;
                let lane = self.simd_lane_literal(name, &user_args[1], ty)?;
                let scalar = self.simd_operand_value(name, &user_args[2])?;
                Ok(self.emit(
                    InstKind::SimdInsert { value, lane, scalar, ty },
                    Some(Type::Vector(ty)),
                ))
            }
            SimdOp::Select => {
                let mask = self.simd_operand_value(name, &user_args[0])?;
                let a = self.simd_operand_value(name, &user_args[1])?;
                let b = self.simd_operand_value(name, &user_args[2])?;
                let ty = self.simd_value_vector_type(name, a)?;
                Ok(self.emit(InstKind::SimdSelect { mask, a, b, ty }, Some(Type::Vector(ty))))
            }
            SimdOp::ReduceAdd
            | SimdOp::ReduceMin
            | SimdOp::ReduceMax
            | SimdOp::ReduceAnd
            | SimdOp::ReduceOr => {
                let value = self.simd_operand_value(name, &user_args[0])?;
                let ty = self.simd_value_vector_type(name, value)?;
                let reduce = match op {
                    SimdOp::ReduceAdd => SimdReduceOp::Add,
                    SimdOp::ReduceMin => SimdReduceOp::Min,
                    SimdOp::ReduceMax => SimdReduceOp::Max,
                    SimdOp::ReduceAnd => SimdReduceOp::And,
                    _ => SimdReduceOp::Or,
                };
                Ok(self.emit(
                    InstKind::SimdReduce { value, op: reduce, ty },
                    Some(ty.lane()),
                ))
            }
            SimdOp::Any | SimdOp::All => {
                let value = self.simd_operand_value(name, &user_args[0])?;
                let ty = self.simd_value_vector_type(name, value)?;
                let all = matches!(op, SimdOp::All);
                Ok(self.emit(InstKind::SimdTest { value, all, ty }, Some(Type::Bool)))
            }
        }
    }

    /// Read back the vector type the type checker stamped onto a
    /// `__simd_splat` / `__simd_load` call.
    fn simd_stamped_type(&self, op: SimdOp, args: &[ExprRef]) -> Option<VecTy> {
        if !op.needs_result_annotation() || args.len() != op.arity() + 1 {
            return None;
        }
        let frontend::ast::Expr::UInt64(code) = self.program.expression.get(&args[op.arity()])?
        else {
            return None;
        };
        VectorType::from_code(code).map(vector_to_ir)
    }

    fn simd_require_stamp(&self, op: SimdOp, stamped: Option<VecTy>) -> Result<VecTy, String> {
        stamped.ok_or_else(|| {
            format!(
                "{} takes its type from a `val` / `var` annotation naming a vector; \
                 write `val v: f64x2 = {}(...)`",
                op.builtin_name(),
                op.builtin_name()
            )
        })
    }

    /// The vector type of argument `i`, read structurally.
    fn simd_operand_type(&self, args: &[ExprRef], i: usize) -> Option<VecTy> {
        match self.value_scalar(args.get(i)?) {
            Some(Type::Vector(v)) => Some(v),
            _ => None,
        }
    }

    /// Lower one argument to a value, requiring that it produces one.
    fn simd_operand_value(&mut self, name: &str, arg: &ExprRef) -> Result<ValueId, String> {
        self.lower_expr(arg)?
            .ok_or_else(|| format!("{name}: argument produced no value"))
    }

    /// The vector type behind an already-lowered value.
    fn simd_value_vector_type(&self, name: &str, value: ValueId) -> Result<VecTy, String> {
        match self.value_ir_type_for(value) {
            Some(Type::Vector(v)) => Ok(v),
            other => Err(format!(
                "{name} expects a vector operand, got {other:?}"
            )),
        }
    }

    /// `__simd_load` / `__simd_store` address by element; the IR
    /// instruction addresses by byte. Multiply here so both the VM
    /// and codegen see a plain byte offset, and fold the common case
    /// where the index is a literal.
    fn simd_byte_offset(
        &mut self,
        name: &str,
        arg: &ExprRef,
        ty: VecTy,
    ) -> Result<ValueId, String> {
        let index = self.simd_operand_value(name, arg)?;
        let stride = self
            .emit(
                InstKind::Const(crate::ir::Const::U64(ty.lane_bytes() as u64)),
                Some(Type::U64),
            )
            .ok_or_else(|| format!("{name}: could not materialise the lane width"))?;
        self.emit(
            InstKind::BinOp { op: crate::ir::BinOp::Mul, lhs: index, rhs: stride },
            Some(Type::U64),
        )
        .ok_or_else(|| format!("{name}: could not compute the byte offset"))
    }

    /// A lane index, which has to be a literal the compiler can read.
    fn simd_lane_literal(&self, name: &str, arg: &ExprRef, ty: VecTy) -> Result<u8, String> {
        let k = match self.program.expression.get(arg) {
            Some(frontend::ast::Expr::UInt64(v)) => v,
            Some(frontend::ast::Expr::Int64(v)) if v >= 0 => v as u64,
            Some(frontend::ast::Expr::UInt32(v) | frontend::ast::Expr::CharLiteral(v)) => v as u64,
            Some(frontend::ast::Expr::UInt8(v)) => v as u64,
            _ => {
                return Err(format!(
                    "{name} needs a literal lane index (the lane is part of the instruction)"
                ));
            }
        };
        if k as usize >= ty.lanes() {
            return Err(format!(
                "{name}: lane {k} is out of range for {}",
                ty.source_name()
            ));
        }
        Ok(k as u8)
    }
}
