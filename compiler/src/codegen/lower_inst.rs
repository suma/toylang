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

use super::{ir_to_cranelift_ty, LowerCtx};

impl<'a, 'b> LowerCtx<'a, 'b> {
    pub(super) fn lower_instruction(
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
                let result = match op {
                    UnaryOp::Neg => {
                        if matches!(operand_ty, Some(IrType::F64)) {
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
                        if matches!(operand_ty, Some(IrType::F64)) {
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
            InstKind::CallIndirect {
                callee,
                args,
                param_tys,
                ret_ty,
            } => {
                // Closures Phase 5b: indirect call through a fn-ptr
                // value. Build a cranelift signature from `param_tys`
                // / `ret_ty` (Unit returns produce no result), import
                // it onto the current function for a fresh SigRef,
                // then `call_indirect` against the callee value.
                let call_conv = self.builder.func.signature.call_conv;
                let mut sig = cranelift_codegen::ir::Signature::new(call_conv);
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
                let callee_val = self.value(*callee);
                let arg_values: Vec<Value> = args.iter().map(|a| self.value(*a)).collect();
                let call_inst = self
                    .builder
                    .ins()
                    .call_indirect(sig_ref, callee_val, &arg_values);
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
            InstKind::Print { value, value_ty, newline } => {
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
                    (IrType::Bool, false) => (self.runtime.print_bool, v),
                    (IrType::Bool, true) => (self.runtime.println_bool, v),
                    (IrType::Str, _) => {
                        // The str runtime value points at the u64
                        // len field (see ConstStr codegen above);
                        // toy_print_str / toy_println_str expect a
                        // NUL-terminated cstring at the byte_start.
                        // Compute byte_start = len_field_addr - 1
                        // (NUL) - len.
                        let len = self.builder.ins().load(
                            types::I64,
                            cranelift_codegen::ir::MemFlags::new(),
                            v,
                            0,
                        );
                        let one = self.builder.ins().iconst(types::I64, 1);
                        let nul_offset = self.builder.ins().iadd(len, one);
                        let byte_start = self.builder.ins().isub(v, nul_offset);
                        let helper = if *newline {
                            self.runtime.println_str
                        } else {
                            self.runtime.print_str
                        };
                        (helper, byte_start)
                    }
                    (IrType::Unit, _) => {
                        return Err(
                            "internal error: Print of Unit reached codegen".to_string(),
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
            InstKind::PrintStr { message, newline } => {
                let gv = *self
                    .print_imports
                    .get(message)
                    .ok_or_else(|| format!("missing print import for #{}", message.to_usize()))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
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
            InstKind::PrintRaw { text, newline } => {
                let key = text.as_bytes();
                let gv = *self
                    .raw_print_imports
                    .get(key)
                    .ok_or_else(|| format!("missing raw print import for {text:?}"))?;
                let addr = self.builder.ins().symbol_value(types::I64, gv);
                let helper = if *newline {
                    self.runtime.println_str
                } else {
                    self.runtime.print_str
                };
                self.builder.ins().call(helper, &[addr]);
            }
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
            InstKind::HeapAlloc { size, binding: _ } => {
                let size_v = self.value(*size);
                let handle_call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let handle_v = self.builder.inst_results(handle_call)[0];
                let call = self
                    .builder
                    .ins()
                    .call(self.runtime.dispatched_alloc, &[handle_v, size_v]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::HeapRealloc { ptr, new_size, binding: _ } => {
                let ptr_v = self.value(*ptr);
                let size_v = self.value(*new_size);
                let handle_call = self.builder.ins().call(self.runtime.alloc_current, &[]);
                let handle_v = self.builder.inst_results(handle_call)[0];
                let call = self.builder.ins().call(
                    self.runtime.dispatched_realloc,
                    &[handle_v, ptr_v, size_v],
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
            // #121 Phase B-rest Item 1: arena / fixed_buffer
            // constructors. Both return a non-zero u64 handle
            // that goes onto the allocator stack via `with`.
            InstKind::AllocArena => {
                let call = self.builder.ins().call(self.runtime.arena_new, &[]);
                let result = self.builder.inst_results(call)[0];
                if let Some((vid, _)) = inst.result {
                    self.values.insert(vid.0, result);
                }
            }
            InstKind::AllocFixedBuffer { capacity } => {
                let cap_v = self.value(*capacity);
                let call = self
                    .builder
                    .ins()
                    .call(self.runtime.fixed_buffer_new, &[cap_v]);
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
            InstKind::AllocArenaDrop { handle } => {
                let h = self.value(*handle);
                self.builder.ins().call(self.runtime.arena_drop, &[h]);
            }
            InstKind::AllocFixedBufferDrop { handle } => {
                let h = self.value(*handle);
                self.builder.ins().call(self.runtime.fixed_buffer_drop, &[h]);
            }
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
        }
        Ok(())
    }

}
