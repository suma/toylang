//! SIMD codegen (SIMD.md Phase 2).
//!
//! A vector maps straight onto a cranelift vector type — `F64X2`,
//! `F32X4`, `I32X4`, `I64X2`, `I8X16` — all of which exist
//! unconditionally on x86-64 (SSE2) and aarch64 (NEON), which is the
//! whole reason the language stops at 128 bits. Nothing here is
//! feature-gated or host-dependent, so an AOT binary stays portable
//! and `assert_consistent` has no ISA to disagree about.
//!
//! Two operations are deliberately *not* single instructions:
//!
//! * `SimdReduce` emits an `extractlane` chain, folding lane 0
//!   through n in order. Cranelift has no horizontal add, and even if
//!   it did, a pairwise tree would give a different `f64` sum than
//!   the tree-walker's loop — the fold order is part of the language.
//! * `Print` / `ToString` spill the vector to a stack slot and pass a
//!   pointer. Passing a vector across the C ABI would tie the runtime
//!   helper to a vector calling convention for no gain.

use cranelift::codegen::ir::{condcodes::{FloatCC, IntCC}, types, InstBuilder};
use cranelift_codegen::ir::{StackSlotData, StackSlotKind, Value};

use crate::ir::{BinOp, InstKind, SimdReduceOp, Type as IrType, UnaryOp, VecTy};

use super::LowerCtx;

/// The cranelift type one vector maps to.
pub(super) fn vec_to_cranelift_ty(v: VecTy) -> types::Type {
    match v {
        VecTy::F64x2 => types::F64X2,
        VecTy::F32x4 => types::F32X4,
        VecTy::I32x4 => types::I32X4,
        VecTy::I64x2 => types::I64X2,
        VecTy::U8x16 => types::I8X16,
    }
}

/// The runtime's stable selector for a vector type. Mirrors
/// `frontend::type_decl::VectorType::code`, which is also what the
/// AST stamp carries — do not renumber.
pub(super) fn vec_type_code(v: VecTy) -> i64 {
    match v {
        VecTy::F64x2 => 0,
        VecTy::F32x4 => 1,
        VecTy::I32x4 => 2,
        VecTy::I64x2 => 3,
        VecTy::U8x16 => 4,
    }
}

impl<'a, 'b> LowerCtx<'a, 'b> {
    /// The `__simd_*` instructions, one arm each.
    pub(super) fn lower_simd(&mut self, inst: &crate::ir::Instruction) -> Result<(), String> {
        match &inst.kind {
            InstKind::SimdSplat { value, ty } => {
                let v = self.value(*value);
                let cl = vec_to_cranelift_ty(*ty);
                let out = self.builder.ins().splat(cl, v);
                self.record_result(inst, out);
            }
            InstKind::SimdLoad { ptr, offset, ty } => {
                let addr = self.simd_address(*ptr, *offset);
                let cl = vec_to_cranelift_ty(*ty);
                // `MemFlags::new()` claims nothing about alignment,
                // so cranelift emits the unaligned load. A vector
                // that starts at element `i` of a byte buffer has no
                // reason to be 16-byte aligned.
                let out = self.builder.ins().load(
                    cl,
                    cranelift_codegen::ir::MemFlags::new(),
                    addr,
                    0,
                );
                self.record_result(inst, out);
            }
            InstKind::SimdStore { ptr, offset, value, .. } => {
                let addr = self.simd_address(*ptr, *offset);
                let v = self.value(*value);
                self.builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    v,
                    addr,
                    0,
                );
            }
            InstKind::SimdExtract { value, lane, .. } => {
                let v = self.value(*value);
                let out = self.builder.ins().extractlane(v, *lane);
                self.record_result(inst, out);
            }
            InstKind::SimdInsert { value, lane, scalar, .. } => {
                let v = self.value(*value);
                let s = self.value(*scalar);
                let out = self.builder.ins().insertlane(v, s, *lane);
                self.record_result(inst, out);
            }
            InstKind::SimdSelect { mask, a, b, ty } => {
                // `bitselect(c, a, b)` picks `a` bit-wise where `c` is
                // set — exactly the all-ones / all-zeros pattern a
                // comparison produces, so no lane-wise conversion is
                // needed. Cranelift does insist all three operands
                // have the *same* type, though, and a float vector's
                // mask is an integer vector: reinterpret the bits (a
                // no-op at run time) rather than convert them.
                let m = self.value(*mask);
                let av = self.value(*a);
                let bv = self.value(*b);
                let want = vec_to_cranelift_ty(*ty);
                let m = if self.builder.func.dfg.value_type(m) == want {
                    m
                } else {
                    self.builder.ins().bitcast(
                        want,
                        cranelift_codegen::ir::MemFlags::new(),
                        m,
                    )
                };
                let out = self.builder.ins().bitselect(m, av, bv);
                self.record_result(inst, out);
            }
            InstKind::SimdReduce { value, op, ty } => {
                let out = self.emit_simd_reduce(*value, *op, *ty)?;
                self.record_result(inst, out);
            }
            InstKind::SimdTest { value, all, .. } => {
                let v = self.value(*value);
                let raw = if *all {
                    self.builder.ins().vall_true(v)
                } else {
                    self.builder.ins().vany_true(v)
                };
                // The `v*_true` instructions answer in an I8; the IR
                // says this instruction produces `Bool`, which is also
                // an I8, so the value passes through unchanged.
                self.record_result(inst, raw);
            }
            InstKind::SimdBitmask { value, ty } => {
                // `vhigh_bits` only lowers for *integer* vectors on
                // aarch64 (x86 accepts a float vector because
                // `movmskps` does), so route a float vector through
                // its own mask type first — a register-level
                // reinterpretation with no run-time cost.
                let v = self.value(*value);
                let v = if ty.is_float() {
                    let want = vec_to_cranelift_ty(ty.mask());
                    self.builder.ins().bitcast(
                        want,
                        cranelift_codegen::ir::MemFlags::new(),
                        v,
                    )
                } else {
                    v
                };
                // At most 16 bits come back, so an `I32` result is
                // wide enough for every lane count; the IR says the
                // instruction produces `u64`, hence the extension.
                let bits = self.builder.ins().vhigh_bits(types::I32, v);
                let out = self.builder.ins().uextend(types::I64, bits);
                self.record_result(inst, out);
            }
            InstKind::SimdSwizzle { table, indices } => {
                let t = self.value(*table);
                let i = self.value(*indices);
                let out = self.builder.ins().swizzle(t, i);
                self.record_result(inst, out);
            }
            InstKind::SimdBitcast { value, to, .. } => {
                let v = self.value(*value);
                let want = vec_to_cranelift_ty(*to);
                // Cranelift insists on an explicit byte order when
                // the lane count changes, and little-endian is the
                // language's definition of a vector's memory image
                // (`__simd_store` writes it, `SimdValue::to_bytes`
                // mirrors it) — not a property of the host.
                let flags = cranelift_codegen::ir::MemFlags::new()
                    .with_endianness(cranelift_codegen::ir::Endianness::Little);
                let out = self.builder.ins().bitcast(want, flags, v);
                self.record_result(inst, out);
            }
            _ => unreachable!("lower_simd was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Lane-wise arithmetic / comparison, dispatched from the ordinary
    /// `BinOp` arm once the operand type turns out to be a vector.
    ///
    /// Integer lanes **wrap** and never trap: no underflow or
    /// divide-by-zero guard is emitted, because the type checker has
    /// already rejected integer `/` and `%` on vectors and every other
    /// integer operator wraps by definition (SIMD.md "意味論" 2).
    pub(super) fn lower_simd_binop(
        &mut self,
        inst: &crate::ir::Instruction,
        op: BinOp,
        lhs: Value,
        rhs: Value,
        ty: VecTy,
    ) -> Result<(), String> {
        let out = if ty.is_float() {
            match op {
                BinOp::Add => self.builder.ins().fadd(lhs, rhs),
                BinOp::Sub => self.builder.ins().fsub(lhs, rhs),
                BinOp::Mul => self.builder.ins().fmul(lhs, rhs),
                BinOp::Div => self.builder.ins().fdiv(lhs, rhs),
                BinOp::Min => self.builder.ins().fmin(lhs, rhs),
                BinOp::Max => self.builder.ins().fmax(lhs, rhs),
                BinOp::Eq => self.builder.ins().fcmp(FloatCC::Equal, lhs, rhs),
                BinOp::Ne => self.builder.ins().fcmp(FloatCC::NotEqual, lhs, rhs),
                BinOp::Lt => self.builder.ins().fcmp(FloatCC::LessThan, lhs, rhs),
                BinOp::Le => self.builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs, rhs),
                BinOp::Gt => self.builder.ins().fcmp(FloatCC::GreaterThan, lhs, rhs),
                BinOp::Ge => self.builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lhs, rhs),
                other => {
                    return Err(format!(
                        "`{other:?}` is not defined on the float lanes of {}",
                        ty.source_name()
                    ));
                }
            }
        } else {
            let signed = !matches!(ty, VecTy::U8x16);
            match op {
                // A shift's right operand is a scalar amount, not a
                // vector: cranelift's vector shifts take one integer
                // and apply it to every lane.
                BinOp::Shl => self.builder.ins().ishl(lhs, rhs),
                BinOp::Shr if signed => self.builder.ins().sshr(lhs, rhs),
                BinOp::Shr => self.builder.ins().ushr(lhs, rhs),
                BinOp::Add => self.builder.ins().iadd(lhs, rhs),
                BinOp::Sub => self.builder.ins().isub(lhs, rhs),
                BinOp::Mul => self.builder.ins().imul(lhs, rhs),
                BinOp::BitAnd => self.builder.ins().band(lhs, rhs),
                BinOp::BitOr => self.builder.ins().bor(lhs, rhs),
                BinOp::BitXor => self.builder.ins().bxor(lhs, rhs),
                BinOp::Min => {
                    if signed {
                        self.builder.ins().smin(lhs, rhs)
                    } else {
                        self.builder.ins().umin(lhs, rhs)
                    }
                }
                BinOp::Max => {
                    if signed {
                        self.builder.ins().smax(lhs, rhs)
                    } else {
                        self.builder.ins().umax(lhs, rhs)
                    }
                }
                BinOp::Eq => self.builder.ins().icmp(IntCC::Equal, lhs, rhs),
                BinOp::Ne => self.builder.ins().icmp(IntCC::NotEqual, lhs, rhs),
                BinOp::Lt => self.builder.ins().icmp(
                    if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan },
                    lhs,
                    rhs,
                ),
                BinOp::Le => self.builder.ins().icmp(
                    if signed {
                        IntCC::SignedLessThanOrEqual
                    } else {
                        IntCC::UnsignedLessThanOrEqual
                    },
                    lhs,
                    rhs,
                ),
                BinOp::Gt => self.builder.ins().icmp(
                    if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan },
                    lhs,
                    rhs,
                ),
                BinOp::Ge => self.builder.ins().icmp(
                    if signed {
                        IntCC::SignedGreaterThanOrEqual
                    } else {
                        IntCC::UnsignedGreaterThanOrEqual
                    },
                    lhs,
                    rhs,
                ),
                other => {
                    return Err(format!(
                        "`{other:?}` is not defined on the integer lanes of {} \
                         (a per-lane divide guard would defeat the vectorisation)",
                        ty.source_name()
                    ));
                }
            }
        };
        self.record_result(inst, out);
        Ok(())
    }

    /// Lane-wise `-` / `~`.
    pub(super) fn lower_simd_unaryop(
        &mut self,
        inst: &crate::ir::Instruction,
        op: UnaryOp,
        operand: Value,
        ty: VecTy,
    ) -> Result<(), String> {
        let out = match op {
            UnaryOp::Neg if ty.is_float() => self.builder.ins().fneg(operand),
            UnaryOp::Neg => self.builder.ins().ineg(operand),
            UnaryOp::BitNot => self.builder.ins().bnot(operand),
            other => {
                return Err(format!(
                    "`{other:?}` is not defined on {}",
                    ty.source_name()
                ));
            }
        };
        self.record_result(inst, out);
        Ok(())
    }

    /// `print` / `println` / `__builtin_to_string` of a vector.
    ///
    /// Spills to a stack slot and hands the runtime helper a pointer
    /// plus the type code, so the rendering lives in one place
    /// (`toy_print_vec` / `toy_to_string_vec`) instead of being
    /// re-derived per lane type in cranelift.
    pub(super) fn lower_simd_render(
        &mut self,
        inst: &crate::ir::Instruction,
        value: Value,
        ty: VecTy,
        newline: Option<bool>,
    ) -> Result<(), String> {
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            16,
            4,
        ));
        self.builder.ins().stack_store(value, slot, 0);
        let addr = self.builder.ins().stack_addr(types::I64, slot, 0);
        let code = self.builder.ins().iconst(types::I64, vec_type_code(ty));
        match newline {
            Some(nl) => {
                let nl_v = self.builder.ins().iconst(types::I8, i64::from(nl));
                self.builder
                    .ins()
                    .call(self.runtime.print_vec, &[addr, code, nl_v]);
            }
            None => {
                let call = self
                    .builder
                    .ins()
                    .call(self.runtime.to_string_vec, &[addr, code]);
                let out = self.builder.inst_results(call)[0];
                self.record_result(inst, out);
            }
        }
        Ok(())
    }

    /// `ptr + byte_offset`, shared by the two memory intrinsics.
    fn simd_address(&mut self, ptr: crate::ir::ValueId, offset: crate::ir::ValueId) -> Value {
        let p = self.value(ptr);
        let off = self.value(offset);
        self.builder.ins().iadd(p, off)
    }

    /// The horizontal fold, as an explicit lane 0 → n chain.
    fn emit_simd_reduce(
        &mut self,
        value: crate::ir::ValueId,
        op: SimdReduceOp,
        ty: VecTy,
    ) -> Result<Value, String> {
        let v = self.value(value);
        let signed = !matches!(ty, VecTy::U8x16);
        let mut acc = self.builder.ins().extractlane(v, 0);
        for lane in 1..ty.lanes() as u8 {
            let next = self.builder.ins().extractlane(v, lane);
            acc = match (op, ty.is_float()) {
                (SimdReduceOp::Add, true) => self.builder.ins().fadd(acc, next),
                (SimdReduceOp::Add, false) => self.builder.ins().iadd(acc, next),
                (SimdReduceOp::Min, true) => self.builder.ins().fmin(acc, next),
                (SimdReduceOp::Max, true) => self.builder.ins().fmax(acc, next),
                (SimdReduceOp::Min, false) => {
                    if signed {
                        self.builder.ins().smin(acc, next)
                    } else {
                        self.builder.ins().umin(acc, next)
                    }
                }
                (SimdReduceOp::Max, false) => {
                    if signed {
                        self.builder.ins().smax(acc, next)
                    } else {
                        self.builder.ins().umax(acc, next)
                    }
                }
                (SimdReduceOp::And, false) => self.builder.ins().band(acc, next),
                (SimdReduceOp::Or, false) => self.builder.ins().bor(acc, next),
                (SimdReduceOp::And | SimdReduceOp::Or, true) => {
                    return Err(format!(
                        "a bitwise reduction needs integer lanes, but {} has float lanes",
                        ty.source_name()
                    ));
                }
            };
        }
        // A narrow lane comes out of `extractlane` at its own width
        // (`I8` for `u8x16`), which is what the IR's lane type says
        // the result is — no extension needed.
        let _ = IrType::Unit;
        Ok(acc)
    }
}
