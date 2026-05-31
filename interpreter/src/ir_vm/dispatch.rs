//! Instruction dispatch for the IR VM.
//!
//! Each instruction is executed against the current `Vm` state.
//! Terminators are handled by the caller (`run_loop`) so this
//! function only processes non-terminator instructions.

use compiler_ir::{BinOp, Const, InstKind, Instruction, LocalId, Type, UnaryOp, ValueId};
use string_interner::Symbol;

use crate::ir_vm::{RawSlot, Vm};

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
            vm.call_function(*target, arg_slots, return_dest);
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
        InstKind::ConstStr { .. } => {
            // Phase 2: string support.
        }
        InstKind::ConstStrBytes { .. } => {
            // Phase 2: string support.
        }
        InstKind::ArrayLoad { .. } => {
            // Phase 2: compound type support.
        }
        InstKind::ArrayStore { .. } => {
            // Phase 2: compound type support.
        }
        InstKind::HeapAlloc { .. } => {
            // Phase 2: heap support.
        }
        InstKind::HeapRealloc { .. } => {
            // Phase 2: heap support.
        }
        InstKind::HeapFree { .. } => {
            // Phase 2: heap support.
        }
        InstKind::PtrRead { .. } => {
            // Phase 2: pointer support.
        }
        InstKind::PtrWrite { .. } => {
            // Phase 2: pointer support.
        }
        InstKind::StrLen { .. } => {
            // Phase 2: string support.
        }
        InstKind::StrConcat { .. } => {
            // Phase 2: string support.
        }
        InstKind::ToString { .. } => {
            // Phase 2: string support.
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
        InstKind::AllocPush { .. } => {
            // Phase 2: allocator support.
        }
        InstKind::AllocPop => {
            // Phase 2: allocator support.
        }
        InstKind::AllocCurrent => {
            // Phase 2: allocator support.
        }
        InstKind::PtrIsNull { .. } => {
            // Phase 2: pointer support.
        }
        InstKind::PtrEq { .. } => {
            // Phase 2: pointer support.
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
        InstKind::ArrayElemAddr { .. } => {
            // Phase 2: array support.
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
