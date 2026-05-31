//! Instruction dispatch for the IR VM.
//!
//! Each instruction is executed against the current `Vm` state.
//! Terminators are handled by the caller (`run_loop`) so this
//! function only processes non-terminator instructions.

use compiler_ir::{ArraySlotId, BinOp, Const, InstKind, Instruction, LocalId, Type, UnaryOp, ValueId};
use string_interner::Symbol;

use crate::ir_vm::{heap, RawSlot, Vm};

/// Execute a single non-terminator instruction.
pub fn execute(vm: &mut Vm, inst: &Instruction) {
    match &inst.kind {
        InstKind::Const(c) => {
            let slot = const_to_slot(*c);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, slot);
            }
        }
        InstKind::BinOp { op, lhs, rhs } => {
            let l = vm.read_value(*lhs);
            let r = vm.read_value(*rhs);
            let result = eval_binop(*op, l, r);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, result);
            }
        }
        InstKind::UnaryOp { op, operand } => {
            let v = vm.read_value(*operand);
            let result = eval_unaryop(*op, v);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, result);
            }
        }
        InstKind::LoadLocal(local) => {
            let slot = vm.read_local(*local);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, slot);
            }
        }
        InstKind::StoreLocal { dst, src } => {
            let slot = vm.read_value(*src);
            vm.write_local(*dst, slot);
        }
        InstKind::Call { target, args } => {
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            let return_dest = inst.result.map(|(vid, _)| vid);
            vm.call_function(*target, arg_slots, return_dest, Vec::new());
        }
        InstKind::CallStruct { target, args, dests } => {
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            vm.call_function(*target, arg_slots, None, dests.clone());
        }
        InstKind::CallTuple { target, args, dests } => {
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            vm.call_function(*target, arg_slots, None, dests.clone());
        }
        InstKind::CallEnum { target, args, dests } => {
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            vm.call_function(*target, arg_slots, None, dests.clone());
        }
        InstKind::Cast { value, from, to } => {
            let v = vm.read_value(*value);
            let result = eval_cast(v, *from, *to);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, result);
            }
        }
        InstKind::Print { value, value_ty, newline } => {
            let v = vm.read_value(*value);
            let text = format_scalar(v, *value_ty);
            if *newline {
                crate::output::println_text(&text);
            } else {
                crate::output::print_text(&text);
            }
        }
        InstKind::PrintStr { message, newline } => {
            let text = format!("printstr #{}", message.to_usize());
            if *newline {
                crate::output::println_text(&text);
            } else {
                crate::output::print_text(&text);
            }
        }
        InstKind::PrintRaw { text, newline } => {
            if *newline {
                crate::output::println_text(text);
            } else {
                crate::output::print_text(text);
            }
        }
        InstKind::ConstStr { message, .. } => {
            if let Some(interner) = vm.interner() {
                let text = interner.resolve(*message).unwrap_or("").to_string();
                let addr = heap::alloc_string(text);
                if let Some((vid, _)) = inst.result {
                    vm.write_value(vid, RawSlot::from_u64(addr));
                }
            }
        }
        InstKind::ConstStrBytes { bytes } => {
            let text = String::from_utf8_lossy(bytes).to_string();
            let addr = heap::alloc_string(text);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::ArrayLoad { slot, index, elem_ty } => {
            let idx = vm.read_value(*index);
            let base = vm.current_frame().array_bases[slot.0 as usize];
            let stride = scalar_size_bytes(*elem_ty);
            let addr = base + unsafe { idx.u64 } * stride as u64;
            if let Some((vid, _)) = inst.result {
                let result = heap::ptr_read(addr, 0, *elem_ty);
                if let Some(slot_val) = result {
                    vm.write_value(vid, slot_val);
                }
            }
        }
        InstKind::ArrayStore { slot, index, value, elem_ty } => {
            let idx = vm.read_value(*index);
            let val = vm.read_value(*value);
            let base = vm.current_frame().array_bases[slot.0 as usize];
            let stride = scalar_size_bytes(*elem_ty);
            let addr = base + unsafe { idx.u64 } * stride as u64;
            heap::ptr_write(addr, 0, val, *elem_ty);
        }
        InstKind::HeapAlloc { size, .. } => {
            let sz = vm.read_value(*size);
            let addr = heap::heap_alloc(unsafe { sz.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::HeapRealloc { ptr, new_size, .. } => {
            let p = vm.read_value(*ptr);
            let ns = vm.read_value(*new_size);
            let addr = heap::heap_realloc(unsafe { p.u64 }, unsafe { ns.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::HeapFree { ptr, .. } => {
            let p = vm.read_value(*ptr);
            heap::heap_free(unsafe { p.u64 });
        }
        InstKind::PtrRead { ptr, offset, elem_ty } => {
            let p = vm.read_value(*ptr);
            let off = vm.read_value(*offset);
            let result = heap::ptr_read(unsafe { p.u64 }, unsafe { off.u64 }, *elem_ty);
            if let Some((vid, _)) = inst.result {
                if let Some(slot) = result {
                    vm.write_value(vid, slot);
                }
            }
        }
        InstKind::PtrWrite { ptr, offset, value, value_ty } => {
            let p = vm.read_value(*ptr);
            let off = vm.read_value(*offset);
            let v = vm.read_value(*value);
            heap::ptr_write(unsafe { p.u64 }, unsafe { off.u64 }, v, *value_ty);
        }
        InstKind::StrLen { value } => {
            let v = vm.read_value(*value);
            let len = heap::string_len(unsafe { v.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(len));
            }
        }
        InstKind::StrConcat { a, b } => {
            let l = vm.read_value(*a);
            let r = vm.read_value(*b);
            let addr = heap::concat_strings(unsafe { l.u64 }, unsafe { r.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::ToString { value, value_ty } => {
            let v = vm.read_value(*value);
            let addr = heap::to_string_value(v, *value_ty);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::MemCopy { .. } => {
            // Phase 2: memory support.
        }
        InstKind::CallStruct { .. } => {
            // Phase 2: compound return support.
        }
        InstKind::CallTuple { .. } => {
            // Phase 2: compound return support.
        }
        InstKind::CallEnum { .. } => {
            // Phase 2: compound return support.
        }
        InstKind::CallWithSelfWriteback { .. } => {
            // Phase 3: reference support.
        }
        InstKind::CallWithSelfWritebackCompound { .. } => {
            // Phase 3: reference support.
        }
        InstKind::AllocPush { handle } => {
            let h = vm.read_value(*handle);
            crate::runtime_state::RT.with(|s| {
                if let Some(ref mut rt) = *s.borrow_mut() {
                    rt.alloc_push(unsafe { h.u64 });
                }
            });
        }
        InstKind::AllocPop => {
            crate::runtime_state::RT.with(|s| {
                if let Some(ref mut rt) = *s.borrow_mut() {
                    rt.alloc_pop();
                }
            });
        }
        InstKind::AllocCurrent => {
            let handle = crate::runtime_state::RT.with(|s| {
                s.borrow().as_ref().map(|rt| rt.alloc_current()).unwrap_or(0)
            });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(handle));
            }
        }
        InstKind::PtrIsNull { ptr } => {
            let p = vm.read_value(*ptr);
            let is_null = unsafe { p.u64 } == 0;
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_bool(is_null));
            }
        }
        InstKind::PtrEq { a, b } => {
            let pa = vm.read_value(*a);
            let pb = vm.read_value(*b);
            let eq = unsafe { pa.u64 } == unsafe { pb.u64 };
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_bool(eq));
            }
        }
        InstKind::AddressOf { .. } => {
            // Phase 3: reference support.
        }
        InstKind::LoadRef { .. } => {
            // Phase 3: reference support.
        }
        InstKind::StoreRef { .. } => {
            // Phase 3: reference support.
        }
        InstKind::ArrayElemAddr { slot, index, elem_ty } => {
            let idx = vm.read_value(*index);
            let base = vm.current_frame().array_bases[slot.0 as usize];
            let stride = scalar_size_bytes(*elem_ty);
            let addr = base + unsafe { idx.u64 } * stride as u64;
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::FuncAddr { .. } => {
            // Phase 3: closure support.
        }
        InstKind::CallIndirect { .. } => {
            // Phase 3: closure support.
        }
        InstKind::MakeClosure { .. } => {
            // Phase 3: closure support.
        }
        InstKind::VtableAddr { .. } => {
            // Phase 3: dyn trait support.
        }
        InstKind::CallIndirectFn { .. } => {
            // Phase 3: dyn trait support.
        }
        InstKind::CallIndirectFnStruct { .. } => {
            // Phase 3: dyn trait support.
        }
        InstKind::CallIndirectFnTuple { .. } => {
            // Phase 3: dyn trait support.
        }
        InstKind::CallIndirectFnEnum { .. } => {
            // Phase 3: dyn trait support.
        }
        InstKind::DynCoerceSlotAddr { .. } => {
            // Phase 3: dyn trait support.
        }
    }
}

fn const_to_slot(c: Const) -> RawSlot {
    match c {
        Const::I64(v) => RawSlot::from_i64(v),
        Const::U64(v) => RawSlot::from_u64(v),
        Const::I8(v) => RawSlot::from_i64(v as i64),
        Const::U8(v) => RawSlot::from_u64(v as u64),
        Const::I16(v) => RawSlot::from_i64(v as i64),
        Const::U16(v) => RawSlot::from_u64(v as u64),
        Const::I32(v) => RawSlot::from_i64(v as i64),
        Const::U32(v) => RawSlot::from_u64(v as u64),
        Const::F64(v) => RawSlot::from_f64(v),
        Const::Bool(v) => RawSlot::from_bool(v),
    }
}

fn eval_binop(op: BinOp, lhs: RawSlot, rhs: RawSlot) -> RawSlot {
    use compiler_ir::BinOp::*;
    // Phase 1: dispatch by the most common scalar types.
    // We read both sides as i64/u64/f64 and let the caller's type
    // knowledge decide which union arm matters.
    match op {
        Add => {
            // Try i64 first, then u64, then f64.
            RawSlot::from_i64(unsafe { lhs.i64.wrapping_add(rhs.i64) })
        }
        Sub => RawSlot::from_i64(unsafe { lhs.i64.wrapping_sub(rhs.i64) }),
        Mul => RawSlot::from_i64(unsafe { lhs.i64.wrapping_mul(rhs.i64) }),
        Div => RawSlot::from_i64(unsafe { lhs.i64.wrapping_div(rhs.i64) }),
        Rem => RawSlot::from_i64(unsafe { lhs.i64.wrapping_rem(rhs.i64) }),
        Eq => RawSlot::from_bool(unsafe { lhs.i64 == rhs.i64 }),
        Ne => RawSlot::from_bool(unsafe { lhs.i64 != rhs.i64 }),
        Lt => RawSlot::from_bool(unsafe { lhs.i64 < rhs.i64 }),
        Le => RawSlot::from_bool(unsafe { lhs.i64 <= rhs.i64 }),
        Gt => RawSlot::from_bool(unsafe { lhs.i64 > rhs.i64 }),
        Ge => RawSlot::from_bool(unsafe { lhs.i64 >= rhs.i64 }),
        BitAnd => RawSlot::from_u64(unsafe { lhs.u64 & rhs.u64 }),
        BitOr => RawSlot::from_u64(unsafe { lhs.u64 | rhs.u64 }),
        BitXor => RawSlot::from_u64(unsafe { lhs.u64 ^ rhs.u64 }),
        Shl => RawSlot::from_u64(unsafe { lhs.u64.wrapping_shl(rhs.u64 as u32) }),
        Shr => RawSlot::from_u64(unsafe { lhs.u64.wrapping_shr(rhs.u64 as u32) }),
        Min => {
            let a = unsafe { lhs.i64 };
            let b = unsafe { rhs.i64 };
            RawSlot::from_i64(if a < b { a } else { b })
        }
        Max => {
            let a = unsafe { lhs.i64 };
            let b = unsafe { rhs.i64 };
            RawSlot::from_i64(if a > b { a } else { b })
        }
        Pow => {
            let base = unsafe { lhs.f64 };
            let exp = unsafe { rhs.f64 };
            RawSlot::from_f64(base.powf(exp))
        }
    }
}

fn eval_unaryop(op: UnaryOp, operand: RawSlot) -> RawSlot {
    use compiler_ir::UnaryOp::*;
    match op {
        Neg => RawSlot::from_i64(unsafe { -(operand.i64) }),
        BitNot => RawSlot::from_u64(unsafe { !(operand.u64) }),
        LogicalNot => RawSlot::from_bool(unsafe { !(operand.bool) }),
        Abs => RawSlot::from_i64(unsafe { operand.i64.wrapping_abs() }),
        Sqrt => RawSlot::from_f64(unsafe { operand.f64.sqrt() }),
        Floor => RawSlot::from_f64(unsafe { operand.f64.floor() }),
        Ceil => RawSlot::from_f64(unsafe { operand.f64.ceil() }),
        Sin => RawSlot::from_f64(unsafe { operand.f64.sin() }),
        Cos => RawSlot::from_f64(unsafe { operand.f64.cos() }),
        Tan => RawSlot::from_f64(unsafe { operand.f64.tan() }),
        Log => RawSlot::from_f64(unsafe { operand.f64.ln() }),
        Log2 => RawSlot::from_f64(unsafe { operand.f64.log2() }),
        Exp => RawSlot::from_f64(unsafe { operand.f64.exp() }),
    }
}

fn eval_cast(value: RawSlot, from: Type, to: Type) -> RawSlot {
    match (from, to) {
        (Type::I64, Type::F64) => RawSlot::from_f64(unsafe { value.i64 as f64 }),
        (Type::U64, Type::F64) => RawSlot::from_f64(unsafe { value.u64 as f64 }),
        (Type::F64, Type::I64) => RawSlot::from_i64(unsafe { value.f64 as i64 }),
        (Type::F64, Type::U64) => RawSlot::from_u64(unsafe { value.f64 as u64 }),
        (Type::I64, Type::U64) => RawSlot::from_u64(unsafe { value.i64 as u64 }),
        (Type::U64, Type::I64) => RawSlot::from_i64(unsafe { value.u64 as i64 }),
        (Type::I8, Type::I64) => RawSlot::from_i64(unsafe { value.i64 }),
        (Type::U8, Type::U64) => RawSlot::from_u64(unsafe { value.u64 }),
        (Type::I16, Type::I64) => RawSlot::from_i64(unsafe { value.i64 }),
        (Type::U16, Type::U64) => RawSlot::from_u64(unsafe { value.u64 }),
        (Type::I32, Type::I64) => RawSlot::from_i64(unsafe { value.i64 }),
        (Type::U32, Type::U64) => RawSlot::from_u64(unsafe { value.u64 }),
        (Type::Bool, Type::Bool) => value,
        (Type::I64, Type::I8) => RawSlot::from_i64(unsafe { value.i64 as i8 as i64 }),
        (Type::U64, Type::U8) => RawSlot::from_u64(unsafe { value.u64 as u8 as u64 }),
        (Type::I64, Type::I16) => RawSlot::from_i64(unsafe { value.i64 as i16 as i64 }),
        (Type::U64, Type::U16) => RawSlot::from_u64(unsafe { value.u64 as u16 as u64 }),
        (Type::I64, Type::I32) => RawSlot::from_i64(unsafe { value.i64 as i32 as i64 }),
        (Type::U64, Type::U32) => RawSlot::from_u64(unsafe { value.u64 as u32 as u64 }),
        _ => value,
    }
}

fn format_scalar(slot: RawSlot, ty: Type) -> String {
    match ty {
        Type::I64 => format!("{}", unsafe { slot.i64 }),
        Type::U64 => format!("{}", unsafe { slot.u64 }),
        Type::I8 => format!("{}", unsafe { slot.i64 as i8 }),
        Type::U8 => format!("{}", unsafe { slot.u64 as u8 }),
        Type::I16 => format!("{}", unsafe { slot.i64 as i16 }),
        Type::U16 => format!("{}", unsafe { slot.u64 as u16 }),
        Type::I32 => format!("{}", unsafe { slot.i64 as i32 }),
        Type::U32 => format!("{}", unsafe { slot.u64 as u32 }),
        Type::F64 => format!("{}", unsafe { slot.f64 }),
        Type::Bool => format!("{}", unsafe { slot.bool }),
        _ => format!("{:?}", unsafe { slot.u64 }),
    }
}

/// Byte size for scalar types. Compound types return 8 (pointer-sized)
/// because the IR VM stores them as opaque handles in RawSlot.
fn scalar_size_bytes(ty: Type) -> u32 {
    match ty {
        Type::I8 | Type::U8 => 1,
        Type::I16 | Type::U16 => 2,
        Type::I32 | Type::U32 => 4,
        Type::I64 | Type::U64 | Type::F64 | Type::Bool | Type::Str => 8,
        Type::Unit => 0,
        _ => 8, // Struct / Tuple / Enum stored as pointer-sized handles
    }
}
