//! Per-instruction Cranelift lowering.
//!
//! Extracted from `mod.rs` so the giant `lower_instruction` switch
//! lives next to its sibling helpers without bloating the entry
//! point. The function is added back to `LowerCtx` via an
//! `impl<'a, 'b> super::LowerCtx<'a, 'b> { ... }` block — same
//! pattern the AOT `lower/` directory uses for its split impls.

use cranelift::codegen::ir::{condcodes::{FloatCC, IntCC}, types, InstBuilder};
use cranelift_codegen::ir::Value;
use string_interner::Symbol;

use crate::ir::{BinOp, Const, InstKind, Type as IrType, UnaryOp};

use super::{flatten_struct_to_cranelift_tys, ir_to_cranelift_ty, LowerCtx};

impl<'a, 'b> LowerCtx<'a, 'b> {
    /// DEBUG-OBS D4: keep the shadow stack around this instruction.
    ///
    /// Wrapping here rather than in each of the call arms is the point:
    /// there are eleven of them, and one forgetting to push is a frame
    /// missing from a backtrace with nothing to make it obvious.
    pub(super) fn lower_instruction(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        let saved_depth = inst.frame.and_then(|id| self.emit_frame_push(id));
        let result = self.lower_instruction_inner(inst);
        if let Some(depth) = saved_depth {
            self.emit_frame_pop(depth);
        }
        result
    }

    /// `slot = &record; depth = mine + 1` — two stores, both to
    /// addresses computed once in the prologue.
    ///
    /// The first shape did the whole computation per call (load the
    /// depth, mask it, scale it, add it to the base, store, increment,
    /// store) and cost **48% on `fib(32)`**, an order over the 5%
    /// budget `DEBUG_OBSERVABILITY.md` D4 set. Everything but the two
    /// stores is loop-invariant across a function body, because a
    /// callee restores the depth it found: one function activation
    /// occupies one slot no matter how many calls it makes. Hoisting
    /// that into [`ShadowPrologue`] leaves two stores here and one in
    /// the pop.
    fn emit_frame_push(
        &mut self,
        frame: compiler_ir::FrameId,
    ) -> Option<cranelift_codegen::ir::Value> {
        let shadow = self.shadow.as_ref()?;
        let record = *shadow.frames.get(&frame)?;
        let prologue = self.shadow_prologue?;
        let flags = cranelift_codegen::ir::MemFlags::trusted();
        let record_addr = self.builder.ins().symbol_value(types::I64, record);
        self.builder.ins().store(flags, record_addr, prologue.slot, 0);
        self.builder
            .ins()
            .store(flags, prologue.inner_depth, prologue.depth_addr, 0);
        Some(prologue.my_depth)
    }

    fn emit_frame_pop(&mut self, my_depth: cranelift_codegen::ir::Value) {
        let Some(prologue) = self.shadow_prologue else { return };
        let flags = cranelift_codegen::ir::MemFlags::trusted();
        self.builder
            .ins()
            .store(flags, my_depth, prologue.depth_addr, 0);
    }

    fn lower_instruction_inner(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::Const(..)
            | InstKind::BinOp { .. }
            | InstKind::UnaryOp { .. }
            | InstKind::Cast { .. }
            | InstKind::LoadLocal { .. }
            | InstKind::StoreLocal { .. }
            | InstKind::AddressOf { .. }
            | InstKind::LoadRef { .. }
            | InstKind::StoreRef { .. }
            | InstKind::ArrayElemAddr { .. } => self.lower_values_and_locals(inst),
            InstKind::Call { .. }
            | InstKind::FuncAddr { .. }
            | InstKind::CallIndirectFn { .. }
            | InstKind::CallIndirectFnTuple { .. }
            | InstKind::CallIndirectFnEnum { .. }
            | InstKind::CallIndirectFnStruct { .. }
            | InstKind::DynCoerceSlotAddr { .. }
            | InstKind::VtableAddr { .. }
            | InstKind::MakeClosure { .. } => self.lower_callee_resolution(inst),
            InstKind::CallIndirect { .. }
            | InstKind::CallStruct { .. }
            | InstKind::CallTuple { .. }
            | InstKind::CallEnum { .. } => self.lower_calls_through_values(inst),
            InstKind::Print { .. }
            | InstKind::PrintStr { .. }
            | InstKind::ConstStr { .. }
            | InstKind::ConstStrBytes { .. }
            | InstKind::PrintRaw { .. } => self.lower_printing(inst),
            InstKind::ArrayLoad { .. }
            | InstKind::ArrayStore { .. } => self.lower_arrays(inst),
            InstKind::HeapAlloc { .. }
            | InstKind::HeapRealloc { .. }
            | InstKind::HeapFree { .. }
            | InstKind::PtrRead { .. }
            | InstKind::PtrWrite { .. } => self.lower_heap_and_pointer(inst),
            InstKind::StrLen { .. }
            | InstKind::StrEq { .. }
            | InstKind::StrFromBytes { .. }
            | InstKind::StrConcat { .. }
            | InstKind::ToString { .. }
            | InstKind::Backtrace
            | InstKind::Format { .. } => self.lower_strings(inst),
            InstKind::MemCopy { .. }
            | InstKind::MemMove { .. }
            | InstKind::MemSet { .. }
            | InstKind::MemEq { .. }
            | InstKind::MemFind { .. }
            | InstKind::MemFindSeq { .. }
            | InstKind::AllocPush { .. }
            | InstKind::AllocPop
            | InstKind::AllocCurrent
            | InstKind::PtrIsNull { .. }
            | InstKind::MemStat { .. }
            | InstKind::MemStatEnable
            | InstKind::RecordAllocatorLayout { .. }
            | InstKind::PtrEq { .. } => self.lower_allocator_and_memory(inst),
            InstKind::CallWithSelfWriteback { .. }
            | InstKind::CallWithSelfWritebackCompound { .. } => self.lower_self_writeback(inst),
            InstKind::SimdSplat { .. }
            | InstKind::SimdLoad { .. }
            | InstKind::SimdStore { .. }
            | InstKind::SimdExtract { .. }
            | InstKind::SimdInsert { .. }
            | InstKind::SimdSelect { .. }
            | InstKind::SimdReduce { .. }
            | InstKind::SimdTest { .. }
            | InstKind::SimdBitmask { .. }
            | InstKind::SimdSwizzle { .. }
            | InstKind::SimdBitcast { .. }
            | InstKind::SimdShuffle { .. } => self.lower_simd(inst),
        }
    }

    /// Constants, arithmetic, casts, and the local / reference slots
    /// values move through.
    fn lower_values_and_locals(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::Const(c) => {
                let v = match c {
                    Const::I64(n) => self.builder.ins().iconst(types::I64, *n),
                    Const::U64(n) => self.builder.ins().iconst(types::I64, *n as i64),
                    // NUM-W-AOT: narrow integer constants. cranelift's
                    // `iconst` takes the width via the type argument;
                    // the value is widened/narrowed at the cranelift
                    // level by the immediate.
                    Const::I32(n) => self.builder.ins().iconst(types::I32, *n as i64),
                    Const::U32(n) => self.builder.ins().iconst(types::I32, *n as i64),
                    Const::I16(n) => self.builder.ins().iconst(types::I16, *n as i64),
                    Const::U16(n) => self.builder.ins().iconst(types::I16, *n as i64),
                    Const::I8(n) => self.builder.ins().iconst(types::I8, *n as i64),
                    Const::U8(n) => self.builder.ins().iconst(types::I8, *n as i64),
                    Const::F64(n) => self.builder.ins().f64const(*n),
                    // SIMD-F32: single-precision constant.
                    Const::F32(n) => self.builder.ins().f32const(*n),
                    Const::Bool(b) => self.builder.ins().iconst(types::I8, *b as i64),
                };
                self.record_result(inst, v);
            }
            InstKind::BinOp { op, lhs, rhs } => {
                let l = self.value(*lhs);
                let r = self.value(*rhs);
                // Dispatch by operand type. F64 uses the float
                // instruction set (fadd/fsub/fmul/fdiv/fcmp); integer
                // ops further split signed vs unsigned for div/rem and
                // ordered comparisons. The type checker has already
                // enforced that both operands share a type, so we only
                // need to look at the lhs.
                let lhs_ty = self.value_ir_type(*lhs).unwrap_or(IrType::U64);
                // SIMD: lane-wise, before the scalar dispatch below —
                // `Type::is_float()` is false for `f64x2`, so a vector
                // would otherwise take the integer path.
                if let IrType::Vector(v) = lhs_ty {
                    return self.lower_simd_binop(inst, *op, l, r, v);
                }
                if lhs_ty.is_float() {
                    let v = match op {
                        BinOp::Add => self.builder.ins().fadd(l, r),
                        BinOp::Sub => self.builder.ins().fsub(l, r),
                        BinOp::Mul => self.builder.ins().fmul(l, r),
                        BinOp::Div => self.builder.ins().fdiv(l, r),
                        BinOp::Rem => {
                            return Err(
                                "compiler MVP does not support `%` on f64 (cranelift has no native fmod)"
                                    .to_string(),
                            );
                        }
                        BinOp::Eq => self.builder.ins().fcmp(FloatCC::Equal, l, r),
                        BinOp::Ne => self.builder.ins().fcmp(FloatCC::NotEqual, l, r),
                        BinOp::Lt => self.builder.ins().fcmp(FloatCC::LessThan, l, r),
                        BinOp::Le => self.builder.ins().fcmp(FloatCC::LessThanOrEqual, l, r),
                        BinOp::Gt => self.builder.ins().fcmp(FloatCC::GreaterThan, l, r),
                        BinOp::Ge => self.builder.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r),
                        BinOp::BitAnd
                        | BinOp::BitOr
                        | BinOp::BitXor
                        | BinOp::Shl
                        | BinOp::Shr => {
                            return Err(
                                "bitwise / shift operators are not defined on f64".to_string(),
                            );
                        }
                        BinOp::Min | BinOp::Max => {
                            return Err(
                                "compiler MVP does not support min/max on f64 yet".to_string(),
                            );
                        }
                        BinOp::Pow => self.emit_pow_call(l, r)?,
                    };
                    self.record_result(inst, v);
                    return Ok(());
                }
                let signed = self.value_is_signed(*lhs);
                let v = match op {
                    BinOp::Add => self.builder.ins().iadd(l, r),
                    BinOp::Sub => self.builder.ins().isub(l, r),
                    BinOp::Mul => self.builder.ins().imul(l, r),
                    BinOp::Div => {
                        if signed {
                            self.builder.ins().sdiv(l, r)
                        } else {
                            self.builder.ins().udiv(l, r)
                        }
                    }
                    BinOp::Rem => {
                        if signed {
                            self.builder.ins().srem(l, r)
                        } else {
                            self.builder.ins().urem(l, r)
                        }
                    }
                    BinOp::Eq => self.builder.ins().icmp(IntCC::Equal, l, r),
                    BinOp::Ne => self.builder.ins().icmp(IntCC::NotEqual, l, r),
                    BinOp::Lt => self.builder.ins().icmp(
                        if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan },
                        l,
                        r,
                    ),
                    BinOp::Le => self.builder.ins().icmp(
                        if signed {
                            IntCC::SignedLessThanOrEqual
                        } else {
                            IntCC::UnsignedLessThanOrEqual
                        },
                        l,
                        r,
                    ),
                    BinOp::Gt => self.builder.ins().icmp(
                        if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan },
                        l,
                        r,
                    ),
                    BinOp::Ge => self.builder.ins().icmp(
                        if signed {
                            IntCC::SignedGreaterThanOrEqual
                        } else {
                            IntCC::UnsignedGreaterThanOrEqual
                        },
                        l,
                        r,
                    ),
                    BinOp::BitAnd => self.builder.ins().band(l, r),
                    BinOp::BitOr => self.builder.ins().bor(l, r),
                    BinOp::BitXor => self.builder.ins().bxor(l, r),
                    BinOp::Shl => self.builder.ins().ishl(l, r),
                    BinOp::Shr => {
                        if signed {
                            self.builder.ins().sshr(l, r)
                        } else {
                            self.builder.ins().ushr(l, r)
                        }
                    }
                    BinOp::Min => {
                        let cc = if signed {
                            IntCC::SignedLessThan
                        } else {
                            IntCC::UnsignedLessThan
                        };
                        let cmp = self.builder.ins().icmp(cc, l, r);
                        self.builder.ins().select(cmp, l, r)
                    }
                    BinOp::Max => {
                        let cc = if signed {
                            IntCC::SignedGreaterThan
                        } else {
                            IntCC::UnsignedGreaterThan
                        };
                        let cmp = self.builder.ins().icmp(cc, l, r);
                        self.builder.ins().select(cmp, l, r)
                    }
                    BinOp::Pow => {
                        return Err(
                            "BinOp::Pow expects f64 operands; integer pow is not supported"
                                .to_string(),
                        );
                    }
                };
                self.record_result(inst, v);
            }
            InstKind::UnaryOp { op, operand } => {
                let v = self.value(*operand);
                let operand_ty = self.value_ir_type(*operand);
                // SIMD: lane-wise `-` / `~`.
                if let Some(IrType::Vector(vt)) = operand_ty {
                    return self.lower_simd_unaryop(inst, *op, v, vt);
                }
                let result = match op {
                    UnaryOp::Neg => {
                        // SIMD-F32: fneg covers both float widths
                        // (cranelift dispatches by the value's type).
                        if matches!(operand_ty, Some(IrType::F64) | Some(IrType::F32)) {
                            self.builder.ins().fneg(v)
                        } else {
                            self.builder.ins().ineg(v)
                        }
                    }
                    UnaryOp::BitNot => self.builder.ins().bnot(v),
                    UnaryOp::LogicalNot => {
                        let one = self.builder.ins().iconst(types::I8, 1);
                        self.builder.ins().bxor(v, one)
                    }
                    UnaryOp::Abs => {
                        // Polymorphic on operand type. f64 lowers to
                        // cranelift's native `fabs` instruction
                        // (single-cycle on most ISAs); i64 has no
                        // direct equivalent, so we emit
                        // `select(x < 0, -x, x)` which folds to a
                        // conditional move.
                        if matches!(operand_ty, Some(IrType::F64) | Some(IrType::F32)) {
                            self.builder.ins().fabs(v)
                        } else {
                            let zero = self.builder.ins().iconst(types::I64, 0);
                            let neg = self.builder.ins().ineg(v);
                            let cmp = self.builder.ins().icmp(IntCC::SignedLessThan, v, zero);
                            self.builder.ins().select(cmp, neg, v)
                        }
                    }
                    UnaryOp::Sqrt => self.builder.ins().sqrt(v),
                    UnaryOp::Floor => self.builder.ins().floor(v),
                    UnaryOp::Ceil => self.builder.ins().ceil(v),
                    UnaryOp::Sin => self.emit_libm_unary_call(self.runtime.sin, v)?,
                    UnaryOp::Cos => self.emit_libm_unary_call(self.runtime.cos, v)?,
                    UnaryOp::Tan => self.emit_libm_unary_call(self.runtime.tan, v)?,
                    UnaryOp::Log => self.emit_libm_unary_call(self.runtime.log, v)?,
                    UnaryOp::Log2 => self.emit_libm_unary_call(self.runtime.log2, v)?,
                    UnaryOp::Exp => self.emit_libm_unary_call(self.runtime.exp, v)?,
                };
                self.record_result(inst, result);
            }
            InstKind::Cast { value, from, to } => {
                let v = self.value(*value);
                let result = self.lower_cast(v, *from, *to)?;
                self.record_result(inst, result);
            }
            InstKind::LoadLocal(local) => {
                // REF-Stage-2 (c): address-taken locals are stored in
                // an explicit `StackSlot` rather than a SSA `Variable`.
                // Read them via `stack_load` so the canonical storage
                // (the one `AddressOf` returns a `stack_addr` for) is
                // the source of truth.
                if let Some(slot) = self.addr_taken_slots.get(&local.0).copied() {
                    let ir_ty = self.ir_module.function(self.func_id).locals[local.0 as usize];
                    let cl_ty = ir_to_cranelift_ty(ir_ty)
                        .ok_or_else(|| format!("LoadLocal: address-taken local {local:?} has unsupported type {ir_ty:?}"))?;
                    let v = self.builder.ins().stack_load(cl_ty, slot, 0);
                    self.record_result(inst, v);
                } else {
                    let var = self.local(*local);
                    let v = self.builder.use_var(var);
                    self.record_result(inst, v);
                }
            }
            InstKind::StoreLocal { dst, src } => {
                let v = self.value(*src);
                if let Some(slot) = self.addr_taken_slots.get(&dst.0).copied() {
                    self.builder.ins().stack_store(v, slot, 0);
                } else {
                    let var = self.local(*dst);
                    self.builder.def_var(var, v);
                }
            }
            InstKind::AddressOf { local } => {
                let slot = *self.addr_taken_slots.get(&local.0).ok_or_else(|| {
                    format!(
                        "AddressOf {local:?}: local was not registered in `address_taken_locals`",
                    )
                })?;
                let v = self.builder.ins().stack_addr(types::I64, slot, 0);
                self.record_result(inst, v);
            }
            InstKind::LoadRef { ptr, ty } => {
                let p = self.value(*ptr);
                let cl_ty = ir_to_cranelift_ty(*ty)
                    .ok_or_else(|| format!("LoadRef: unsupported pointee type {ty:?}"))?;
                use cranelift_codegen::ir::MemFlags;
                let v = self.builder.ins().load(cl_ty, MemFlags::new(), p, 0);
                self.record_result(inst, v);
            }
            InstKind::StoreRef { ptr, value, ty: _ } => {
                let p = self.value(*ptr);
                let v = self.value(*value);
                use cranelift_codegen::ir::MemFlags;
                self.builder.ins().store(MemFlags::new(), v, p, 0);
            }
            InstKind::ArrayElemAddr { slot, index, elem_ty: _ } => {
                let stack_slot = *self
                    .array_slots
                    .get(&slot.0)
                    .ok_or_else(|| format!("array slot {slot:?} missing"))?;
                let info = &self.ir_module.function(self.func_id).array_slots[slot.0 as usize];
                let stride = info.elem_stride_bytes as i64;
                let base = self.builder.ins().stack_addr(types::I64, stack_slot, 0);
                let idx = self.value(*index);
                let off = self.builder.ins().imul_imm(idx, stride);
                let addr = self.builder.ins().iadd(base, off);
                self.record_result(inst, addr);
            }
            _ => unreachable!("lower_values_and_locals was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Working out what to call: a direct callee, a fat pointer, a vtable
    /// slot, or a closure.
    fn lower_callee_resolution(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::Call { target, args } => {
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if let Some((vid, _ty)) = inst.result {
                    let v = results.first().copied().ok_or_else(|| {
                        "callee declared a return type but produced no Cranelift result".to_string()
                    })?;
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::FuncAddr { target } => {
                // Closures Phase 5b: yield the runtime address of a
                // top-level function as a u64 value. Reuses the
                // function-import FuncRef pre-declared by
                // `declare_imports`.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let addr = self.builder.ins().func_addr(types::I64, func_ref);
                self.record_result(inst, addr);
            }
            InstKind::CallIndirectFn {
                callee,
                args,
                param_tys,
                ret_ty,
            } => {
                // A5-P2: raw fn-pointer indirect call (no env-passing).
                // Mirror the `CallIndirect` lowering but without the
                // implicit env arg and without the env+0 fn_ptr load —
                // `callee` is already the function pointer (e.g. read
                // out of a vtable slot via `PtrRead`).
                let fn_ptr = self.value(*callee);
                let call_conv = self.builder.func.signature.call_conv;
                let mut sig = cranelift_codegen::ir::Signature::new(call_conv);
                for pt in param_tys {
                    let cl = ir_to_cranelift_ty(*pt).ok_or_else(|| {
                        format!("CallIndirectFn: cannot lower param type {pt:?}")
                    })?;
                    sig.params.push(cranelift_codegen::ir::AbiParam::new(cl));
                }
                if !matches!(ret_ty, IrType::Unit) {
                    let cl = ir_to_cranelift_ty(*ret_ty).ok_or_else(|| {
                        format!("CallIndirectFn: cannot lower return type {ret_ty:?}")
                    })?;
                    sig.returns
                        .push(cranelift_codegen::ir::AbiParam::new(cl));
                }
                let sig_ref = self.builder.import_signature(sig);
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self
                    .builder
                    .ins()
                    .call_indirect(sig_ref, fn_ptr, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if let Some((vid, _ty)) = inst.result {
                    let v = results.first().copied().ok_or_else(|| {
                        "CallIndirectFn declared a return type but produced no Cranelift result".to_string()
                    })?;
                    self.values.insert(vid.0, v);
                }
            }
            // A5-P2-MVP-D/E: an indirect call whose return is a
            // compound (struct / tuple / enum). The callee comes from
            // a runtime ValueId (a vtable-loaded fn ptr) rather than a
            // fixed FuncId, and the cranelift signature carries one
            // return slot per flattened leaf, in the same canonical
            // order `flatten_compound_leaf_types` gives the impl side.
            // The three shapes differ only in which IR type names the
            // return, so they share one lowering.
            InstKind::CallIndirectFnTuple {
                callee,
                args,
                param_tys,
                ret_tuple_id,
                dests,
            } => self.lower_indirect_compound_call(
                "CallIndirectFnTuple",
                "call_indirect_fn_tuple",
                *callee,
                args,
                param_tys,
                IrType::Tuple(*ret_tuple_id),
                dests,
            )?,
            InstKind::CallIndirectFnEnum {
                callee,
                args,
                param_tys,
                ret_enum_id,
                dests,
            } => self.lower_indirect_compound_call(
                "CallIndirectFnEnum",
                "call_indirect_fn_enum",
                *callee,
                args,
                param_tys,
                IrType::Enum(*ret_enum_id),
                dests,
            )?,
            InstKind::CallIndirectFnStruct {
                callee,
                args,
                param_tys,
                ret_struct_id,
                dests,
            } => self.lower_indirect_compound_call(
                "CallIndirectFnStruct",
                "call_indirect_fn_struct",
                *callee,
                args,
                param_tys,
                IrType::Struct(*ret_struct_id),
                dests,
            )?,
            InstKind::DynCoerceSlotAddr { slot_idx } => {
                // A5-P2-MVP-B: lazily materialise the cranelift
                // `StackSlot` for this `Function::dyn_coerce_slots`
                // entry, then return its address as I64. The slot
                // lives in the caller's cranelift frame, so its
                // lifetime is bounded by the enclosing function
                // body — matching the toylang `&dyn Trait` borrow
                // semantics (the trait object never escapes the
                // call that constructed it).
                use cranelift_codegen::ir::{StackSlotData, StackSlotKind};
                let slot = if let Some(s) = self.dyn_coerce_stack_slots.get(slot_idx).copied() {
                    s
                } else {
                    let func = self.ir_module.function(self.func_id);
                    let size = *func.dyn_coerce_slots.get(*slot_idx as usize).ok_or_else(
                        || {
                            format!(
                                "DynCoerceSlotAddr {slot_idx}: out of range \
                                 (function has {} dyn_coerce_slots)",
                                func.dyn_coerce_slots.len()
                            )
                        },
                    )?;
                    let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size,
                        0,
                    ));
                    self.dyn_coerce_stack_slots.insert(*slot_idx, slot);
                    slot
                };
                let addr = self.builder.ins().stack_addr(types::I64, slot, 0);
                self.record_result(inst, addr);
            }
            InstKind::VtableAddr {
                trait_sym,
                struct_sym,
            } => {
                // A5-P2: materialise the vtable's runtime address.
                // `declare_vtable_imports` already installed a
                // `GlobalValue` for each `(trait, struct)` pair this
                // function references; `symbol_value` produces the
                // I64 pointer the linker resolves to the vtable's
                // first slot.
                let gv = *self
                    .vtable_imports
                    .get(&(*trait_sym, *struct_sym))
                    .ok_or_else(|| {
                        format!(
                            "vtable_addr {:?}/{:?}: GlobalValue not declared (no `impl {} for {}` or missing vtable layout)",
                            trait_sym, struct_sym, trait_sym.to_usize(), struct_sym.to_usize()
                        )
                    })?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                self.record_result(inst, addr);
            }
            InstKind::MakeClosure {
                target,
                captures,
                capture_tys,
            } => {
                // Closures Phase 6: build an env on the heap.
                // Layout: [fn_ptr: i64][cap0: i64][cap1: i64]...
                // Captures are stored in 8-byte slots (zext for
                // narrow ints would happen at the lift site, but
                // Phase 6 restricts captures to 8-byte scalars).
                if captures.len() != capture_tys.len() {
                    return Err(format!(
                        "MakeClosure: {} captures but {} capture types",
                        captures.len(),
                        capture_tys.len()
                    ));
                }
                let env_size_bytes = (1 + captures.len()) as i64 * 8;
                let size_val = self.builder.ins().iconst(types::I64, env_size_bytes);
                let alloc_call = self.builder.ins().call(self.runtime.malloc, &[size_val]);
                let alloc_results = self.builder.inst_results(alloc_call).to_vec();
                let env_ptr = *alloc_results.first().ok_or_else(|| {
                    "MakeClosure: malloc returned no result".to_string()
                })?;
                // Store fn_ptr at offset 0.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("MakeClosure: missing import for {target:?}"))?;
                let fn_ptr = self.builder.ins().func_addr(types::I64, func_ref);
                use cranelift_codegen::ir::MemFlags;
                let flags = MemFlags::new();
                self.builder
                    .ins()
                    .store(flags, fn_ptr, env_ptr, 0i32);
                // Store each capture at +8, +16, ... — every slot
                // is 8-byte aligned regardless of capture width
                // (Phase 6c: narrow ints occupy the low N bytes
                // of an 8-byte slot, the rest is unused — same
                // pattern the body's PtrRead recovers via a
                // width-aware load).
                for (i, (cap_val, cap_ty)) in captures.iter().zip(capture_tys.iter()).enumerate() {
                    let offset = ((i + 1) * 8) as i32;
                    let v = self.value(*cap_val);
                    let cl_ty = ir_to_cranelift_ty(*cap_ty)
                        .ok_or_else(|| format!("MakeClosure: cannot lower capture type {cap_ty:?}"))?;
                    // cranelift's `store` is width-polymorphic on
                    // the value's type; we don't need to specialise
                    // per-width as long as the matching `load.<ty>`
                    // is used at the read site (driven by the
                    // PtrRead instruction's `elem_ty`).
                    let _ = cl_ty;
                    self.builder.ins().store(flags, v, env_ptr, offset);
                }
                self.record_result(inst, env_ptr);
            }
            _ => unreachable!("lower_callee_resolution was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// One lowering for `CallIndirectFn{Struct,Tuple,Enum}`.
    ///
    /// `what` names the instruction in a parameter-type failure,
    /// `lowered` names it in the snake_case internal-error text the
    /// three arms have always used; `ret` is the IR type whose leaves
    /// become the cranelift return slots.
    #[allow(clippy::too_many_arguments)]
    fn lower_indirect_compound_call(
        &mut self,
        what: &str,
        lowered: &str,
        callee: crate::ir::ValueId,
        args: &[crate::ir::ValueId],
        param_tys: &[IrType],
        ret: IrType,
        dests: &[crate::ir::LocalId],
    ) -> Result<(), String> {
        let fn_ptr = self.value(callee);
        let call_conv = self.builder.func.signature.call_conv;
        let mut sig = cranelift_codegen::ir::Signature::new(call_conv);
        for pt in param_tys {
            let cl = ir_to_cranelift_ty(*pt)
                .ok_or_else(|| format!("{what}: cannot lower param type {pt:?}"))?;
            sig.params.push(cranelift_codegen::ir::AbiParam::new(cl));
        }
        for cl in &flatten_struct_to_cranelift_tys(self.ir_module, ret) {
            sig.returns.push(cranelift_codegen::ir::AbiParam::new(*cl));
        }
        let sig_ref = self.builder.import_signature(sig);
        let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
        let call_inst = self
            .builder
            .ins()
            .call_indirect(sig_ref, fn_ptr, &arg_values);
        let results = self.builder.inst_results(call_inst).to_vec();
        if results.len() != dests.len() {
            return Err(format!(
                "internal error: {lowered} returned {} value(s), expected {}",
                results.len(),
                dests.len(),
            ));
        }
        for (dest, val) in dests.iter().zip(results.iter()) {
            let var = self.local(*dest);
            self.builder.def_var(var, *val);
        }
        Ok(())
    }


    /// Calling through a value, and the calls whose result is compound
    /// (struct / tuple / enum) and so lands in several destinations.
    fn lower_calls_through_values(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::CallIndirect {
                callee,
                args,
                param_tys,
                ret_ty,
            } => {
                // Closures Phase 6b: env-based indirect call. The
                // callee is an env_ptr (i64) — env layout starts
                // with [fn_ptr] at offset 0 followed by captures.
                // Codegen loads fn_ptr from `env+0`, builds a
                // signature `(env: i64, ...param_tys) -> ret_ty`,
                // and calls `call_indirect(sig, fn_ptr, [env, args])`.
                // The lifted closure body's first IR parameter is
                // the env pointer (set up by `lower_closure_body`),
                // matching this calling convention.
                use cranelift_codegen::ir::MemFlags;
                let env_val = self.value(*callee);
                let flags = MemFlags::new();
                let fn_ptr = self.builder.ins().load(types::I64, flags, env_val, 0i32);
                let call_conv = self.builder.func.signature.call_conv;
                let mut sig = cranelift_codegen::ir::Signature::new(call_conv);
                // Implicit env: i64 first parameter — every closure
                // body has it as IR param[0].
                sig.params
                    .push(cranelift_codegen::ir::AbiParam::new(types::I64));
                for pt in param_tys {
                    let cl = ir_to_cranelift_ty(*pt).ok_or_else(|| {
                        format!("CallIndirect: cannot lower param type {pt:?} to cranelift")
                    })?;
                    sig.params.push(cranelift_codegen::ir::AbiParam::new(cl));
                }
                if !matches!(ret_ty, IrType::Unit) {
                    let cl = ir_to_cranelift_ty(*ret_ty).ok_or_else(|| {
                        format!("CallIndirect: cannot lower return type {ret_ty:?} to cranelift")
                    })?;
                    sig.returns.push(cranelift_codegen::ir::AbiParam::new(cl));
                }
                let sig_ref = self.builder.import_signature(sig);
                let mut arg_values: Vec<Value> = Vec::with_capacity(args.len() + 1);
                arg_values.push(env_val);
                for a in args {
                    arg_values.push(self.value(*a));
                }
                let call_inst = self
                    .builder
                    .ins()
                    .call_indirect(sig_ref, fn_ptr, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if let Some((vid, _ty)) = inst.result {
                    let v = results.first().copied().ok_or_else(|| {
                        "CallIndirect declared a return type but produced no Cranelift result"
                            .to_string()
                    })?;
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::CallStruct { target, args, dests } => {
                // Multi-result call: store result `i` into `dests[i]`.
                // Each `dest` is a per-field local pre-allocated by
                // lower.rs, so the def_var mapping into a cranelift
                // Variable is straightforward.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: call returned {} value(s), expected {}",
                        results.len(),
                        dests.len()
                    ));
                }
                for (dest, val) in dests.iter().zip(results.iter()) {
                    let var = self.local(*dest);
                    self.builder.def_var(var, *val);
                }
            }
            InstKind::CallTuple { target, args, dests } => {
                // Same shape as CallStruct, just for tuple returns.
                // The cranelift call signature was already built with
                // one return per tuple element, so the multi-result
                // walk works identically.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: tuple call returned {} value(s), expected {}",
                        results.len(),
                        dests.len()
                    ));
                }
                for (dest, val) in dests.iter().zip(results.iter()) {
                    let var = self.local(*dest);
                    self.builder.def_var(var, *val);
                }
            }
            InstKind::CallEnum { target, args, dests } => {
                // Same shape as CallStruct / CallTuple. The cranelift
                // signature was built with one return per enum slot
                // (tag + every variant's payloads in declaration
                // order); `dests` mirrors that order.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                if results.len() != dests.len() {
                    return Err(format!(
                        "internal error: enum call returned {} value(s), expected {}",
                        results.len(),
                        dests.len()
                    ));
                }
                for (dest, val) in dests.iter().zip(results.iter()) {
                    let var = self.local(*dest);
                    self.builder.def_var(var, *val);
                }
            }
            _ => unreachable!("lower_calls_through_values was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Everything that reaches stdout or stderr, plus the `.rodata`
    /// str blobs the print helpers and interpolation read.
    ///
    /// RUNTIME-LIB P0-A: an instruction marked `stderr` is bracketed
    /// by `toy_print_stream(1)` / `toy_print_stream(0)` rather than
    /// routed to a second set of helpers — one selector call pair per
    /// stderr print instead of a mirrored `toy_eprint_*` table, and
    /// the stdout path is untouched.
    fn lower_printing(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        let stderr = match &inst.kind {
            InstKind::Print { stderr, .. }
            | InstKind::PrintStr { stderr, .. }
            | InstKind::PrintRaw { stderr, .. } => *stderr,
            _ => false,
        };
        if stderr {
            self.set_print_stream(true);
        }
        let result = self.lower_printing_inner(inst);
        if stderr {
            self.set_print_stream(false);
        }
        result
    }

    /// Point the runtime's per-thread print stream at stderr (`on`)
    /// or back at stdout.
    fn set_print_stream(&mut self, on: bool) {
        let flag = self.builder.ins().iconst(cranelift_codegen::ir::types::I8, on as i64);
        let helper = self.runtime.print_stream;
        self.builder.ins().call(helper, &[flag]);
    }

    fn lower_printing_inner(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::Print { value, value_ty, newline, stderr: _ } => {
                let v = self.value(*value);
                // NUM-W-AOT-pack Phase 2: dedicated narrow-int
                // helpers (`toy_print_{i,u}{8,16,32}`) take the
                // value at its native cranelift width — no
                // sextend / uextend needed at the call site.
                // Decimal output is byte-identical to the prior
                // wide-helper routing (the C runtime prints the
                // same digits via `%d` / `%u` of an int / unsigned
                // arg), so this is a codegen-aesthetics + one
                // fewer extension instruction per print site.
                // SIMD: rendered by the one runtime helper, off a
                // stack-slot spill (see `lower_simd_render`).
                if let IrType::Vector(vt) = value_ty {
                    return self.lower_simd_render(inst, v, *vt, Some(*newline));
                }
                let (helper, call_value) = match (value_ty, newline) {
                    (IrType::I64, false) => (self.runtime.print_i64, v),
                    (IrType::I64, true) => (self.runtime.println_i64, v),
                    (IrType::U64, false) => (self.runtime.print_u64, v),
                    (IrType::U64, true) => (self.runtime.println_u64, v),
                    (IrType::I32, false) => (self.runtime.print_i32, v),
                    (IrType::I32, true) => (self.runtime.println_i32, v),
                    (IrType::U32, false) => (self.runtime.print_u32, v),
                    (IrType::U32, true) => (self.runtime.println_u32, v),
                    (IrType::I16, false) => (self.runtime.print_i16, v),
                    (IrType::I16, true) => (self.runtime.println_i16, v),
                    (IrType::U16, false) => (self.runtime.print_u16, v),
                    (IrType::U16, true) => (self.runtime.println_u16, v),
                    (IrType::I8, false) => (self.runtime.print_i8, v),
                    (IrType::I8, true) => (self.runtime.println_i8, v),
                    (IrType::U8, false) => (self.runtime.print_u8, v),
                    (IrType::U8, true) => (self.runtime.println_u8, v),
                    (IrType::F64, false) => (self.runtime.print_f64, v),
                    (IrType::F64, true) => (self.runtime.println_f64, v),
                    // SIMD-F32: single-precision helpers take the f32
                    // at its native width and format it with the same
                    // "always a decimal point" convention.
                    (IrType::F32, false) => (self.runtime.print_f32, v),
                    (IrType::F32, true) => (self.runtime.println_f32, v),
                    (IrType::Bool, false) => (self.runtime.print_bool, v),
                    (IrType::Bool, true) => (self.runtime.println_bool, v),
                    (IrType::Str, _) => {
                        // The helpers take the str value as-is and read
                        // its length field, so there is nothing to
                        // compute here. (This used to hand them the
                        // byte_start and let them walk to the NUL,
                        // which truncated any str containing one.)
                        let helper = if *newline {
                            self.runtime.println_str
                        } else {
                            self.runtime.print_str
                        };
                        (helper, v)
                    }
                    (IrType::Unit, _) => {
                        return Err(
                            "internal error: Print of Unit reached codegen".to_string(),
                        );
                    }
                    // Handled by the `lower_simd_render` short-circuit
                    // above; unreachable here.
                    (IrType::Vector(_), _) => {
                        return Err(
                            "internal error: Print of a vector reached the scalar helper table"
                                .to_string(),
                        );
                    }
                    (IrType::Struct(_), _) => {
                        return Err(
                            "internal error: Print of struct reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                    (IrType::Tuple(_), _) => {
                        return Err(
                            "internal error: Print of tuple reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                    (IrType::Enum(_), _) => {
                        return Err(
                            "internal error: Print of enum reached codegen (should be rejected at lower)"
                                .to_string(),
                        );
                    }
                };
                self.builder.ins().call(helper, &[call_value]);
            }
            InstKind::PrintStr { message, bytes_len, newline, stderr: _ } => {
                let gv = *self
                    .print_imports
                    .get(message)
                    .ok_or_else(|| format!("missing print import for #{}", message.to_usize()))?;
                // The symbol points at the byte_start; the helper wants
                // the handle, which is `bytes_len + 1` further on (the
                // `+1` steps over the NUL).
                let symbol_addr = self.builder.ins().symbol_value(types::I64, gv);
                let addr = self
                    .builder
                    .ins()
                    .iadd_imm(symbol_addr, (*bytes_len as i64) + 1);
                let helper = if *newline {
                    self.runtime.println_str
                } else {
                    self.runtime.print_str
                };
                self.builder.ins().call(helper, &[addr]);
            }
            InstKind::ConstStr { message, bytes_len } => {
                let gv = *self
                    .print_imports
                    .get(message)
                    .ok_or_else(|| {
                        format!("missing print import for #{}", message.to_usize())
                    })?;
                // The `.rodata` symbol points at the byte_start of
                // the layout `[bytes][NUL][u64 len LE]`. The str
                // runtime value points at the **u64 len field** so
                // `__builtin_str_len(s)` is a single
                // `load.i64(s, 0)`. Offset = bytes_len + 1 (the NUL
                // byte sits between bytes and the len field).
                let symbol_addr = self.builder.ins().symbol_value(types::I64, gv);
                let len_field_offset = (*bytes_len as i64) + 1;
                let addr = self.builder.ins().iadd_imm(symbol_addr, len_field_offset);
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, addr);
                }
            }
            InstKind::ConstStrBytes { bytes } => {
                // STR-INTERP-COMPOUND: same `.rodata` shape as
                // `ConstStr`, but the symbol was registered by
                // content rather than by a frontend-interner symbol
                // (the format prefixes the lower assembles for
                // struct/tuple/enum to_string don't have one).
                let gv = *self
                    .const_str_bytes_imports
                    .get(bytes)
                    .ok_or_else(|| {
                        format!("missing ConstStrBytes import (len={})", bytes.len())
                    })?;
                let symbol_addr = self.builder.ins().symbol_value(types::I64, gv);
                let len_field_offset = (bytes.len() as i64) + 1;
                let addr = self.builder.ins().iadd_imm(symbol_addr, len_field_offset);
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, addr);
                }
            }
            InstKind::PrintRaw { text, newline, stderr: _ } => {
                let key = text.as_bytes();
                let gv = *self
                    .raw_print_imports
                    .get(key)
                    .ok_or_else(|| format!("missing raw print import for {text:?}"))?;
                let symbol_addr = self.builder.ins().symbol_value(types::I64, gv);
                let addr = self
                    .builder
                    .ins()
                    .iadd_imm(symbol_addr, (text.len() as i64) + 1);
                let helper = if *newline {
                    self.runtime.println_str
                } else {
                    self.runtime.print_str
                };
                self.builder.ins().call(helper, &[addr]);
            }
            _ => unreachable!("lower_printing was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Array element load and store.
    fn lower_arrays(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::ArrayLoad { slot, index, elem_ty } => {
                let cl_ty = ir_to_cranelift_ty(*elem_ty)
                    .ok_or_else(|| format!("ArrayLoad: unsupported elem_ty {elem_ty:?}"))?;
                let stack_slot = *self
                    .array_slots
                    .get(&slot.0)
                    .ok_or_else(|| format!("missing stack slot for array {:?}", slot.0))?;
                let stride = self
                    .ir_module
                    .function(self.func_id)
                    .array_slots[slot.0 as usize]
                    .elem_stride_bytes;
                let idx_v = self.value(*index);
                // Compute byte offset = index * stride. Index value
                // type is I64 in our IR (always u64/i64); stride is
                // a small u32 constant.
                let stride_v = self.builder.ins().iconst(types::I64, stride as i64);
                let byte_off = self.builder.ins().imul(idx_v, stride_v);
                let base = self.builder.ins().stack_addr(types::I64, stack_slot, 0);
                let addr = self.builder.ins().iadd(base, byte_off);
                let v = self.builder.ins().load(
                    cl_ty,
                    cranelift_codegen::ir::MemFlags::new(),
                    addr,
                    0,
                );
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::ArrayStore { slot, index, value, elem_ty } => {
                let _ = elem_ty;
                let stack_slot = *self
                    .array_slots
                    .get(&slot.0)
                    .ok_or_else(|| format!("missing stack slot for array {:?}", slot.0))?;
                let stride = self
                    .ir_module
                    .function(self.func_id)
                    .array_slots[slot.0 as usize]
                    .elem_stride_bytes;
                let idx_v = self.value(*index);
                let val_v = self.value(*value);
                let stride_v = self.builder.ins().iconst(types::I64, stride as i64);
                let byte_off = self.builder.ins().imul(idx_v, stride_v);
                let base = self.builder.ins().stack_addr(types::I64, stack_slot, 0);
                let addr = self.builder.ins().iadd(base, byte_off);
                self.builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    val_v,
                    addr,
                    0,
                );
            }
            _ => unreachable!("lower_arrays was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Heap allocation and raw pointer access.
    fn lower_heap_and_pointer(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            // #121 Phase A: heap / pointer builtins. malloc/realloc
            // accept and return i64-sized pointers; free returns
            // void. PtrRead / PtrWrite use the IR's recorded element
            // type to pick the correct cranelift load / store width.
            // #121 Phase B-rest Item 3: heap_alloc / realloc / free
            // route through the active allocator. We read the
            // current handle (sentinel 0 = default global / libc
            // direct path) and pass it as the first arg to
            // `toy_dispatched_*` which handles the dispatch.
            // Phase 5: `binding` is informational today — codegen
            // routes every variant through the active-stack
            // dispatch. A future devirt pass can branch on
            // `Static` to emit a direct libc malloc / free without
            // reading `toy_alloc_current`.
            InstKind::HeapAlloc { size, binding: _, site } => {
                let size_v = self.value(*size);
                let handle_call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let handle_v = self.builder.inst_results(handle_call)[0];
                // MEMORY_PROFILING M2: the packed source position rides
                // along as a constant. An extra register argument is
                // cheaper than a separate call to set it, and codegen
                // cannot know whether profiling will be on at run time.
                //
                // The file name goes the same way, as a pointer to a
                // `.rodata` blob — which is how the leak report can
                // name a file the binary never opens.
                let packed = self.ir_module.packed_site(*site);
                let site_v = self.builder.ins().iconst(types::I64, packed as i64);
                let file_v = match self.alloc_file_imports.get(self.ir_module.site_file(*site)) {
                    Some(gv) => self.builder.ins().symbol_value(types::I64, *gv),
                    None => self.builder.ins().iconst(types::I64, 0),
                };
                let call = self.builder.ins().call(
                    self.runtime.dispatched_alloc,
                    &[handle_v, size_v, site_v, file_v],
                );
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::HeapRealloc { ptr, new_size, binding: _, site } => {
                let ptr_v = self.value(*ptr);
                let size_v = self.value(*new_size);
                let handle_call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let handle_v = self.builder.inst_results(handle_call)[0];
                // The site rides along as a constant, used only when
                // the runtime sees a null `ptr` (M2 + D2) — the file
                // name as a `.rodata` pointer, like `HeapAlloc`'s.
                let packed = self.ir_module.packed_site(*site);
                let site_v = self.builder.ins().iconst(types::I64, packed as i64);
                let file_v = match self.alloc_file_imports.get(self.ir_module.site_file(*site)) {
                    Some(gv) => self.builder.ins().symbol_value(types::I64, *gv),
                    None => self.builder.ins().iconst(types::I64, 0),
                };
                let call = self.builder.ins().call(
                    self.runtime.dispatched_realloc,
                    &[handle_v, ptr_v, size_v, site_v, file_v],
                );
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::HeapFree { ptr, binding: _ } => {
                let ptr_v = self.value(*ptr);
                let handle_call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let handle_v = self.builder.inst_results(handle_call)[0];
                self.builder
                    .ins()
                    .call(self.runtime.dispatched_free, &[handle_v, ptr_v]);
            }
            InstKind::PtrRead { ptr, offset, elem_ty } => {
                let cl_ty = ir_to_cranelift_ty(*elem_ty)
                    .ok_or_else(|| format!("PtrRead: unsupported elem_ty {elem_ty:?}"))?;
                let ptr_v = self.value(*ptr);
                let off_v = self.value(*offset);
                let addr = self.builder.ins().iadd(ptr_v, off_v);
                let v = self.builder.ins().load(
                    cl_ty,
                    cranelift_codegen::ir::MemFlags::new(),
                    addr,
                    0,
                );
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, v);
                }
            }
            InstKind::PtrWrite { ptr, offset, value, value_ty } => {
                let _ = value_ty;
                let ptr_v = self.value(*ptr);
                let off_v = self.value(*offset);
                let val_v = self.value(*value);
                let addr = self.builder.ins().iadd(ptr_v, off_v);
                self.builder.ins().store(
                    cranelift_codegen::ir::MemFlags::new(),
                    val_v,
                    addr,
                    0,
                );
            }
            _ => unreachable!("lower_heap_and_pointer was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// str length, equality, construction, concatenation, and rendering
    /// (`__builtin_to_string` / `__builtin_format`).
    fn lower_strings(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::StrLen { value } => {
                // O(1): the str runtime value points directly at
                // the u64 len field (see ConstStr above).
                // `load.i64(s, 0)` reads the stored byte length
                // without walking the bytes.
                let v = self.value(*value);
                let result = self.builder.ins().load(
                    types::I64,
                    cranelift_codegen::ir::MemFlags::new(),
                    v,
                    0,
                );
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::StrEq { a, b } => {
                let av = self.value(*a);
                let bv = self.value(*b);
                let call = self.builder.ins().call(self.runtime.str_eq, &[av, bv]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::StrFromBytes { ptr, len } => {
                let p = self.value(*ptr);
                let n = self.value(*len);
                let call = self.builder.ins().call(self.runtime.str_from_bytes, &[p, n]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::StrConcat { a, b } => {
                // Direct call to `toy_str_concat(a, b)` — both args
                // and the result are str runtime values (= u64
                // pointers in cranelift IR; see toylang_rt for
                // the concrete heap layout).
                let av = self.value(*a);
                let bv = self.value(*b);
                let call = self.builder.ins().call(self.runtime.str_concat, &[av, bv]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            // DEBUG-OBS D5: the runtime walks the shadow stack and
            // builds the str; nothing here is known at compile time.
            InstKind::Backtrace => {
                let call = self.builder.ins().call(self.runtime.backtrace_str, &[]);
                let result = self.builder.inst_results(call)[0];
                if let Some((v, _)) = inst.result {
                    self.values.insert(v.0, result);
                }
            }
            InstKind::ToString { value, value_ty } => {
                // Direct call to the matching `toy_to_string_<ty>`
                // helper. The IR captured the value's type at lower
                // time so we can pick the right helper without
                // re-inferring at codegen.
                let v = self.value(*value);
                let helper = match value_ty {
                    IrType::I64 => self.runtime.to_string_i64,
                    IrType::U64 => self.runtime.to_string_u64,
                    IrType::F64 => self.runtime.to_string_f64,
                    IrType::F32 => self.runtime.to_string_f32,
                    IrType::Bool => self.runtime.to_string_bool,
                    IrType::Str => self.runtime.to_string_str,
                    IrType::I8 => self.runtime.to_string_i8,
                    IrType::U8 => self.runtime.to_string_u8,
                    IrType::I16 => self.runtime.to_string_i16,
                    IrType::U16 => self.runtime.to_string_u16,
                    IrType::I32 => self.runtime.to_string_i32,
                    IrType::U32 => self.runtime.to_string_u32,
                    IrType::Unit => {
                        return Err(
                            "internal error: __builtin_to_string of Unit reached codegen \
                             (lower should have rejected)"
                                .to_string(),
                        );
                    }
                    IrType::Struct(_) | IrType::Tuple(_) | IrType::Enum(_) => {
                        return Err(format!(
                            "internal error: __builtin_to_string of compound type {:?} \
                             reached codegen (lower should have rejected)",
                            value_ty
                        ));
                    }
                    // SIMD: same helper as `print`, minus the newline.
                    IrType::Vector(vt) => {
                        return self.lower_simd_render(inst, v, *vt, None);
                    }
                };
                let call = self.builder.ins().call(helper, &[v]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::Format { value, value_ty, spec } => {
                // STR-INTERP-FMT: `(value, spec[, bits])` into one of
                // the five `toy_format_*` helpers. Narrow ints widen
                // here — sign-extended or zero-extended to match their
                // own signedness — and carry their original width so
                // the runtime can mask a two's-complement rendering to
                // it (`{-1i32:x}` is `ffffffff`, not 16 digits).
                let v = self.value(*value);
                let spec_v = self.builder.ins().iconst(types::I64, *spec as i64);
                let (helper, args) = match value_ty {
                    IrType::I64 | IrType::I32 | IrType::I16 | IrType::I8 => {
                        let bits = match value_ty {
                            IrType::I8 => 8i64,
                            IrType::I16 => 16,
                            IrType::I32 => 32,
                            _ => 64,
                        };
                        let widened = if bits == 64 {
                            v
                        } else {
                            self.builder.ins().sextend(types::I64, v)
                        };
                        let bits_v = self.builder.ins().iconst(types::I64, bits);
                        (self.runtime.format_i64, vec![widened, spec_v, bits_v])
                    }
                    IrType::U64 | IrType::U32 | IrType::U16 | IrType::U8 => {
                        let bits = match value_ty {
                            IrType::U8 => 8i64,
                            IrType::U16 => 16,
                            IrType::U32 => 32,
                            _ => 64,
                        };
                        let widened = if bits == 64 {
                            v
                        } else {
                            self.builder.ins().uextend(types::I64, v)
                        };
                        let bits_v = self.builder.ins().iconst(types::I64, bits);
                        (self.runtime.format_u64, vec![widened, spec_v, bits_v])
                    }
                    IrType::F64 => (self.runtime.format_f64, vec![v, spec_v]),
                    // STDLIB-NUMERIC N5: its own helper, not a promotion
                    // through f64 -- `0.1f32` prints `0.1` at single
                    // precision and `0.10000000149011612` promoted.
                    IrType::F32 => (self.runtime.format_f32, vec![v, spec_v]),
                    IrType::Bool => (self.runtime.format_bool, vec![v, spec_v]),
                    IrType::Str => (self.runtime.format_str, vec![v, spec_v]),
                    IrType::Unit | IrType::Struct(_) | IrType::Tuple(_) | IrType::Enum(_)
                    // SIMD: a format spec's knobs (width / precision /
                    // radix) have no single meaning across lanes, so a
                    // spec on a vector is rejected the same way one on
                    // a struct is.
                    | IrType::Vector(_) => {
                        return Err(format!(
                            "internal error: __builtin_format of {value_ty:?} reached codegen \
                             (the type checker only allows primitives)"
                        ));
                    }
                };
                let call = self.builder.ins().call(helper, &args);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            _ => unreachable!("lower_strings was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// The allocator stack, allocation counters, bulk memory, and the
    /// pointer predicates.
    fn lower_allocator_and_memory(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            InstKind::MemCopy { src, dest, size } => {
                // libc memcpy uses (dest, src, n) — swap from
                // toylang's (src, dest, size) order.
                let src_v = self.value(*src);
                let dest_v = self.value(*dest);
                let size_v = self.value(*size);
                self.builder
                    .ins()
                    .call(self.runtime.memcpy, &[dest_v, src_v, size_v]);
            }
            InstKind::MemMove { src, dest, size } => {
                // Same (src, dest, size) -> (dest, src, n) swap as
                // MemCopy; libc memmove tolerates overlap.
                let src_v = self.value(*src);
                let dest_v = self.value(*dest);
                let size_v = self.value(*size);
                self.builder
                    .ins()
                    .call(self.runtime.memmove, &[dest_v, src_v, size_v]);
            }
            InstKind::MemSet { dest, byte, size } => {
                // libc memset takes the fill value as an `int`, so the
                // toylang `u8` is zero-extended (never sign-extended:
                // 0xFFu8 fills with 0xFF, not with 0xFFFFFFFF).
                let dest_v = self.value(*dest);
                let byte_v = self.value(*byte);
                let size_v = self.value(*size);
                let byte_ty = self.builder.func.dfg.value_type(byte_v);
                let byte_i32 = if byte_ty == types::I32 {
                    byte_v
                } else if byte_ty.bits() < 32 {
                    self.builder.ins().uextend(types::I32, byte_v)
                } else {
                    self.builder.ins().ireduce(types::I32, byte_v)
                };
                self.builder
                    .ins()
                    .call(self.runtime.memset, &[dest_v, byte_i32, size_v]);
            }
            InstKind::MemEq { a, b, size } => {
                let a_v = self.value(*a);
                let b_v = self.value(*b);
                let size_v = self.value(*size);
                let call = self.builder.ins().call(self.runtime.mem_eq, &[a_v, b_v, size_v]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::MemFind { ptr, len, byte } => {
                let p = self.value(*ptr);
                let n = self.value(*len);
                let b = self.value(*byte);
                // The helper takes the byte at its own width; the
                // value arrives as an I8 already.
                let b_ty = self.builder.func.dfg.value_type(b);
                let b8 = if b_ty == types::I8 {
                    b
                } else {
                    self.builder.ins().ireduce(types::I8, b)
                };
                let call = self.builder.ins().call(self.runtime.mem_find, &[p, n, b8]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::MemFindSeq { hay, hay_len, needle, needle_len } => {
                let h = self.value(*hay);
                let hn = self.value(*hay_len);
                let n = self.value(*needle);
                let nn = self.value(*needle_len);
                let call = self
                    .builder
                    .ins()
                    .call(self.runtime.mem_find_seq, &[h, hn, n, nn]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            // #121 Phase B-min: active-allocator stack ops.
            // `AllocPush(handle)` and `AllocPop` emit a libc call
            // to `toy_alloc_push(handle)` / `toy_alloc_pop()`.
            // `AllocCurrent` returns the current top as a u64
            // value (sentinel 0 when the stack is empty).
            InstKind::AllocPush { handle } => {
                let handle_v = self.value(*handle);
                self.builder.ins().call(self.runtime.alloc_push, &[handle_v]);
            }
            InstKind::AllocPop => {
                self.builder.ins().call(self.runtime.alloc_pop, &[]);
            }
            InstKind::AllocCurrent => {
                let call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::PtrIsNull { ptr } => {
                let p = self.value(*ptr);
                let cmp = self
                    .builder
                    .ins()
                    .icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, p, 0);
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, cmp);
                }
            }
            // MEMORY_PROFILING M4. One call with a constant selector
            // rather than an entry point per counter — the constant
            // costs a register, a second symbol costs a declaration
            // and a place to get them out of step.
            InstKind::MemStat { stat } => {
                let which = self.builder.ins().iconst(types::I64, *stat as i64);
                let call = self.builder.ins().call(self.runtime.prof_stat, &[which]);
                let value = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, value);
                }
            }
            InstKind::MemStatEnable => {
                self.builder.ins().call(self.runtime.prof_force_counting, &[]);
            }
            InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest } => {
                let name_v = self.value(*name);
                let managed_v = self.value(*managed);
                let live_v = self.value(*live);
                let free_blocks_v = self.value(*free_blocks);
                let largest_v = self.value(*largest);
                self.builder.ins().call(
                    self.runtime.record_allocator_layout,
                    &[name_v, managed_v, live_v, free_blocks_v, largest_v],
                );
            }
            InstKind::PtrEq { a, b } => {
                let av = self.value(*a);
                let bv = self.value(*b);
                let cmp = self
                    .builder
                    .ins()
                    .icmp(cranelift_codegen::ir::condcodes::IntCC::Equal, av, bv);
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, cmp);
                }
            }
            _ => unreachable!("lower_allocator_and_memory was handed an instruction it does not own"),
        }
        Ok(())
    }

    /// Calls that hand back mutated `self` leaves alongside their return
    /// value (the `&mut self` convention).
    fn lower_self_writeback(
        &mut self,
        inst: &crate::ir::Instruction,
    ) -> Result<(), String> {
        match &inst.kind {
            // Stage 1 of `&` references: call to a `&mut self`
            // method. The cranelift call returns
            // `(user_return_leaves..., self_writeback_leaves...)`
            // — index 0 is the user-visible scalar return (when
            // `ret_ty` is Some) and the trailing slots are the
            // receiver leaves to store back into the caller's
            // binding locals via `def_var`.
            InstKind::CallWithSelfWriteback { target, args, ret_dest, ret_ty: _, self_dests } => {
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                let expected = ret_dest.map(|_| 1usize).unwrap_or(0) + self_dests.len();
                if results.len() != expected {
                    return Err(format!(
                        "internal error: call_with_self_writeback returned {} value(s), expected {}",
                        results.len(),
                        expected,
                    ));
                }
                let mut idx = 0usize;
                if let Some(local) = ret_dest {
                    let var = self.local(*local);
                    self.builder.def_var(var, results[idx]);
                    idx += 1;
                }
                for local in self_dests {
                    let var = self.local(*local);
                    self.builder.def_var(var, results[idx]);
                    idx += 1;
                }
            }
            InstKind::CallWithSelfWritebackCompound { target, args, ret_dests, self_dests } => {
                // A5-P2-MVP-F: cranelift call returns
                // `[ret_leaves..., self_writeback_leaves...]`. The
                // split is fixed at `ret_dests.len()` because the
                // impl method's signature was built by appending
                // `self_writeback_types` after the lowered user
                // return type (see lower/program.rs writeback
                // setup). Fan each half into its dest local via
                // `def_var`.
                let func_ref = *self
                    .imports
                    .get(target)
                    .ok_or_else(|| format!("missing import for {target:?}"))?;
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self.builder.ins().call(func_ref, &arg_values);
                let results = self.builder.inst_results(call_inst).to_vec();
                let expected = ret_dests.len() + self_dests.len();
                if results.len() != expected {
                    return Err(format!(
                        "internal error: call_with_self_writeback_compound returned {} value(s), expected {}",
                        results.len(),
                        expected,
                    ));
                }
                for (i, local) in ret_dests.iter().enumerate() {
                    let var = self.local(*local);
                    self.builder.def_var(var, results[i]);
                }
                let offset = ret_dests.len();
                for (i, local) in self_dests.iter().enumerate() {
                    let var = self.local(*local);
                    self.builder.def_var(var, results[offset + i]);
                }
            }
            _ => unreachable!("lower_self_writeback was handed an instruction it does not own"),
        }
        Ok(())
    }


}
