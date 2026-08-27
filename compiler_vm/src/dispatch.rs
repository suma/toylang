//! Instruction dispatch for the IR VM.
//!
//! Each instruction is executed against the current `Vm` state.
//! Terminators are handled by the caller (`run_loop`) so this
//! function only processes non-terminator instructions.
//!
//! Everything the interpreter used to reach for here — stdout, the
//! heap manager, the allocator stack, the allocation counters — now
//! arrives through the `VmHost` (COMPILE-TIME-EVAL C6), so this
//! module has no crate-internal dependencies beyond `compiler_ir`.

use compiler_ir::{BinOp, Const, InstKind, Instruction, LocalId, Type, UnaryOp};
use string_interner::Symbol;

use crate::host::VmHost;
use crate::{RawSlot, Vm};

/// Execute a single non-terminator instruction.
pub fn execute(vm: &mut Vm, inst: &Instruction) {
    // DEBUG-OBS D4: hand the calling instruction's backtrace frame to
    // whichever `call_function` this dispatch reaches. Set here rather
    // than in each of the call arms — they all end in the same place,
    // and one of them forgetting is exactly the kind of hole this
    // phase exists to close.
    vm.set_pending_frame(inst.frame);
    let host = vm.host();
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
            // Operand type drives f64 / signed / unsigned dispatch. Prefer
            // the recorded lhs type; fall back to the result type (correct
            // for arithmetic, where result type == operand type).
            let ty = vm
                .value_type(*lhs)
                .or_else(|| inst.result.map(|(_, t)| t))
                .unwrap_or(Type::I64);
            // `str` equality compares content, not the handle pointer
            // (each `ConstStr` allocates a fresh str, so identical literals
            // have distinct handles — pointer eq would always be false,
            // matching the tree-walker which compares bytes). Used by
            // `match s { "lit" => .. }` and `==` / `!=` on `str`.
            let result = if matches!(ty, Type::Str)
                && matches!(*op, BinOp::Eq | BinOp::Ne)
            {
                let eq = host.read_str(unsafe { l.u64 }) == host.read_str(unsafe { r.u64 });
                RawSlot::from_bool(if matches!(*op, BinOp::Eq) { eq } else { !eq })
            } else {
                eval_binop(*op, l, r, ty)
            };
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, result);
            }
        }
        InstKind::UnaryOp { op, operand } => {
            let v = vm.read_value(*operand);
            let ty = vm
                .value_type(*operand)
                .or_else(|| inst.result.map(|(_, t)| t))
                .unwrap_or(Type::I64);
            let result = eval_unaryop(*op, v, ty);
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
            let text = format_scalar(host, v, *value_ty);
            if *newline {
                host.println_text(&text);
            } else {
                host.print_text(&text);
            }
        }
        InstKind::PrintStr { message, newline, .. } => {
            let text = vm
                .interner()
                .and_then(|i| i.resolve(*message))
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("printstr #{}", message.to_usize()));
            if *newline {
                host.println_text(&text);
            } else {
                host.print_text(&text);
            }
        }
        InstKind::PrintRaw { text, newline } => {
            if *newline {
                host.println_text(text);
            } else {
                host.print_text(text);
            }
        }
        InstKind::ConstStr { message, .. } => {
            if let Some(interner) = vm.interner() {
                let text = interner.resolve(*message).unwrap_or("");
                let addr = host.alloc_str_bytes(text.as_bytes());
                if let Some((vid, _)) = inst.result {
                    vm.write_value(vid, RawSlot::from_u64(addr));
                }
            }
        }
        // DEBUG-OBS D5: the VM has a call stack of its own, so the
        // answer is the same walk the panic path makes.
        InstKind::Backtrace => {
            let text = vm.backtrace_text();
            let addr = host.alloc_str_bytes(text.trim_start_matches('\n').as_bytes());
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::ConstStrBytes { bytes } => {
            let addr = host.alloc_str_bytes(bytes);
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
                let result = host.ptr_read(addr, 0, *elem_ty);
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
            host.ptr_write(addr, 0, val, *elem_ty);
        }
        InstKind::HeapAlloc { size, site, .. } => {
            let sz = vm.read_value(*size);
            let addr = host.alloc_at(unsafe { sz.u64 }, *site);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::HeapRealloc { ptr, new_size, .. } => {
            let p = vm.read_value(*ptr);
            let ns = vm.read_value(*new_size);
            let addr = host.realloc(unsafe { p.u64 }, unsafe { ns.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::HeapFree { ptr, .. } => {
            let p = vm.read_value(*ptr);
            host.free(unsafe { p.u64 });
        }
        InstKind::PtrRead { ptr, offset, elem_ty } => {
            let p = vm.read_value(*ptr);
            let off = vm.read_value(*offset);
            let result = host.ptr_read(unsafe { p.u64 }, unsafe { off.u64 }, *elem_ty);
            if let (Some((vid, _)), Some(slot)) = (inst.result, result) {
                vm.write_value(vid, slot);
            }
        }
        InstKind::PtrWrite { ptr, offset, value, value_ty } => {
            let p = vm.read_value(*ptr);
            let off = vm.read_value(*offset);
            let v = vm.read_value(*value);
            host.ptr_write(unsafe { p.u64 }, unsafe { off.u64 }, v, *value_ty);
        }
        InstKind::StrLen { value } => {
            let v = vm.read_value(*value);
            let len = host.string_len(unsafe { v.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(len));
            }
        }
        InstKind::StrConcat { a, b } => {
            let l = vm.read_value(*a);
            let r = vm.read_value(*b);
            let addr = host.concat_strings(unsafe { l.u64 }, unsafe { r.u64 });
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::StrEq { a, b } => {
            let l = unsafe { vm.read_value(*a).u64 };
            let r = unsafe { vm.read_value(*b).u64 };
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_bool(host.str_eq(l, r)));
            }
        }
        InstKind::StrFromBytes { ptr, len } => {
            let p = unsafe { vm.read_value(*ptr).u64 };
            let n = unsafe { vm.read_value(*len).u64 };
            let addr = host.str_from_bytes(p, n);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::ToString { value, value_ty } => {
            let v = vm.read_value(*value);
            let addr = host.to_string_value(v, *value_ty);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::Format { value, value_ty, spec } => {
            // STR-INTERP-FMT: same shape as ToString, plus the packed
            // spec the parser fixed at compile time.
            let v = vm.read_value(*value);
            let addr = host.format_value(v, *value_ty, *spec);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::MemCopy { src, dest, size } => {
            // Phase 3c: libc memcpy (toylang arg order src, dest, size).
            let s = unsafe { vm.read_value(*src).u64 };
            let d = unsafe { vm.read_value(*dest).u64 };
            let n = unsafe { vm.read_value(*size).u64 };
            host.mem_copy(s, d, n);
        }
        InstKind::CallWithSelfWriteback { target, args, ret_dest, self_dests, .. } => {
            // Phase 3c: `&mut self` call. The callee returns
            // `[ret_leaf?, self_writeback_leaves...]`; route every
            // returned leaf into the combined dest list so the existing
            // multi-result return wiring distributes them.
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            let mut dests: Vec<LocalId> = Vec::with_capacity(self_dests.len() + 1);
            if let Some(local) = ret_dest {
                dests.push(*local);
            }
            dests.extend_from_slice(self_dests);
            vm.call_function(*target, arg_slots, None, dests);
        }
        InstKind::CallWithSelfWritebackCompound { target, args, ret_dests, self_dests } => {
            // Phase 3c: writeback + compound user return. Results come
            // back as `[ret_leaves..., self_writeback_leaves...]`.
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            let mut dests: Vec<LocalId> = Vec::with_capacity(ret_dests.len() + self_dests.len());
            dests.extend_from_slice(ret_dests);
            dests.extend_from_slice(self_dests);
            vm.call_function(*target, arg_slots, None, dests);
        }
        InstKind::AllocPush { handle } => {
            let h = vm.read_value(*handle);
            host.alloc_push(unsafe { h.u64 });
        }
        InstKind::AllocPop => {
            host.alloc_pop();
        }
        InstKind::AllocCurrent => {
            let handle = host.alloc_current();
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
        // MEMORY_PROFILING M4. The interpreter's counters are always
        // being kept, so there is nothing for `MemStatEnable` to turn
        // on here — only the compiled runtime gates counting.
        InstKind::MemStat { stat } => {
            let value = frontend::ast::MemStat::from_code(*stat)
                .map(|s| host.mem_stat(s))
                .unwrap_or(0);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(value));
            }
        }
        InstKind::MemStatEnable => {}
        InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest } => {
            let name_str = host.read_str(unsafe { vm.read_value(*name).u64 });
            let managed = unsafe { vm.read_value(*managed).u64 };
            let live = unsafe { vm.read_value(*live).u64 };
            let free_blocks = unsafe { vm.read_value(*free_blocks).u64 };
            let largest = unsafe { vm.read_value(*largest).u64 };
            host.record_allocator_layout(&name_str, managed, live, free_blocks, largest);
        }
        InstKind::AddressOf { local } => {
            // Phase 3c: pointer to an address-taken local's backing cell.
            let addr = vm.addr_of_local(*local);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::LoadRef { ptr, ty } => {
            // Phase 3c: dereference a pointer to read a scalar of `ty`.
            let p = unsafe { vm.read_value(*ptr).u64 };
            let result = host.ptr_read(p, 0, *ty).unwrap_or_default();
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, result);
            }
        }
        InstKind::StoreRef { ptr, value, ty } => {
            // Phase 3c: write a scalar through a pointer.
            let p = unsafe { vm.read_value(*ptr).u64 };
            let v = vm.read_value(*value);
            host.ptr_write(p, 0, v, *ty);
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
        InstKind::FuncAddr { target } => {
            // Phase 3a: a function pointer is represented in the VM as the
            // raw FuncId index encoded into a u64. `CallIndirect` /
            // `MakeClosure` recover it as `FuncId(slot.u64 as u32)`.
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(target.0 as u64));
            }
        }
        InstKind::MakeClosure { target, captures, capture_tys } => {
            // Phase 3a: heap-allocate the env `[fn_ptr][cap0][cap1]...`,
            // mirroring the AOT layout (8-byte slots, fn_ptr at +0,
            // capture `i` at +(i+1)*8). The fn_ptr stores the FuncId so
            // CallIndirect can dispatch back into the VM.
            let env_size = ((1 + captures.len()) as u64) * 8;
            let addr = host.alloc_at(env_size, 0);
            host.ptr_write(addr, 0, RawSlot::from_u64(target.0 as u64), Type::U64);
            for (i, (cap, cap_ty)) in captures.iter().zip(capture_tys.iter()).enumerate() {
                let v = vm.read_value(*cap);
                host.ptr_write(addr, ((i + 1) * 8) as u64, v, *cap_ty);
            }
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::CallIndirect { callee, args, .. } => {
            // Phase 3a: env-based indirect call. `callee` is an env_ptr;
            // fn_ptr lives at env+0. The lifted closure body's first
            // parameter is the env_ptr, so prepend it to the user args.
            let env_ptr = unsafe { vm.read_value(*callee).u64 };
            let fn_ptr = host
                .ptr_read(env_ptr, 0, Type::U64)
                .map(|s| unsafe { s.u64 })
                .unwrap_or(0);
            let target = compiler_ir::FuncId(fn_ptr as u32);
            let mut arg_slots: Vec<RawSlot> = Vec::with_capacity(args.len() + 1);
            arg_slots.push(RawSlot::from_u64(env_ptr));
            for a in args {
                arg_slots.push(vm.read_value(*a));
            }
            let return_dest = inst.result.map(|(vid, _)| vid);
            vm.call_function(target, arg_slots, return_dest, Vec::new());
        }
        InstKind::VtableAddr { trait_sym, struct_sym } => {
            // Phase 3b: materialise the vtable on the heap and yield its
            // address (a U64). A later PtrRead recovers the per-method
            // dispatch FuncId.
            let addr = vm.vtable_addr(*trait_sym, *struct_sym);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::DynCoerceSlotAddr { slot_idx } => {
            // Phase 3b: yield the address of a caller-frame coercion buffer
            // for a field-bearing struct passed through `&dyn Trait`.
            let addr = vm.dyn_coerce_addr(*slot_idx);
            if let Some((vid, _)) = inst.result {
                vm.write_value(vid, RawSlot::from_u64(addr));
            }
        }
        InstKind::CallIndirectFn { callee, args, .. } => {
            // Phase 3b: raw fn-pointer indirect call (no implicit env).
            // `callee` is already a FuncId (loaded out of a vtable slot).
            let target = compiler_ir::FuncId(unsafe { vm.read_value(*callee).u64 } as u32);
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            let return_dest = inst.result.map(|(vid, _)| vid);
            vm.call_function(target, arg_slots, return_dest, Vec::new());
        }
        InstKind::CallIndirectFnStruct { callee, args, dests, .. }
        | InstKind::CallIndirectFnTuple { callee, args, dests, .. }
        | InstKind::CallIndirectFnEnum { callee, args, dests, .. } => {
            // Phase 3b: indirect call returning a compound (struct/tuple/
            // enum). The compound leaves fan out into `dests`, mirroring
            // the direct-call CallStruct/CallTuple/CallEnum lowering.
            let target = compiler_ir::FuncId(unsafe { vm.read_value(*callee).u64 } as u32);
            let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
            vm.call_function(target, arg_slots, None, dests.clone());
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

/// Whether `ty` is an unsigned integer (drives Div/Rem signedness and
/// comparison interpretation).
fn is_unsigned(ty: Type) -> bool {
    matches!(ty, Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::Bool)
}

/// NUM-W: a narrow integer slot always holds its value normalised —
/// zero-extended when unsigned, sign-extended when signed — so a
/// later `eq` compares the numbers rather than whatever bits happened
/// to survive a 64-bit computation.
///
/// The arithmetic below is done at 64 bits whatever the operand type
/// is, which left `200u8 * 3u8` sitting in the slot as 600. It printed
/// as 88 (the printer masks) and cast to 88, so the mistake was
/// invisible until something compared it: `200u8 * 3u8 == 88u8` was
/// **false** on the IR VM and true on the tree-walker, the JIT and the
/// AOT binary, all three of which work at the narrow width natively.
/// Found by the COMPILE-TIME-EVAL C0 lane, which compares a folded
/// constant against the same computation done at run time.
fn normalise_narrow(raw: RawSlot, ty: Type) -> RawSlot {
    match int_desc(ty) {
        Some((bits, signed)) if bits < 64 => {
            encode_int(decode_int(unsafe { raw.u64 }, bits, signed), bits, signed)
        }
        _ => raw,
    }
}

fn eval_binop(op: BinOp, lhs: RawSlot, rhs: RawSlot, ty: Type) -> RawSlot {
    let raw = eval_binop_raw(op, lhs, rhs, ty);
    // A comparison produces a `bool` regardless of the operand width;
    // everything else produces a value of the operand type.
    if op.produces_bool() {
        raw
    } else {
        normalise_narrow(raw, ty)
    }
}

fn eval_binop_raw(op: BinOp, lhs: RawSlot, rhs: RawSlot, ty: Type) -> RawSlot {
    use compiler_ir::BinOp::*;
    let is_f64 = matches!(ty, Type::F64);
    let unsigned = is_unsigned(ty);
    match op {
        // Arithmetic: result type == operand type.
        Add if is_f64 => RawSlot::from_f64(unsafe { lhs.f64 + rhs.f64 }),
        Sub if is_f64 => RawSlot::from_f64(unsafe { lhs.f64 - rhs.f64 }),
        Mul if is_f64 => RawSlot::from_f64(unsafe { lhs.f64 * rhs.f64 }),
        Div if is_f64 => RawSlot::from_f64(unsafe { lhs.f64 / rhs.f64 }),
        Rem if is_f64 => RawSlot::from_f64(unsafe { lhs.f64 % rhs.f64 }),
        Add => RawSlot::from_i64(unsafe { lhs.i64.wrapping_add(rhs.i64) }),
        Sub => RawSlot::from_i64(unsafe { lhs.i64.wrapping_sub(rhs.i64) }),
        Mul => RawSlot::from_i64(unsafe { lhs.i64.wrapping_mul(rhs.i64) }),
        Div if unsigned => RawSlot::from_u64(unsafe { lhs.u64.wrapping_div(rhs.u64) }),
        Rem if unsigned => RawSlot::from_u64(unsafe { lhs.u64.wrapping_rem(rhs.u64) }),
        Div => RawSlot::from_i64(unsafe { lhs.i64.wrapping_div(rhs.i64) }),
        Rem => RawSlot::from_i64(unsafe { lhs.i64.wrapping_rem(rhs.i64) }),
        // Comparisons: dispatch on operand type, result is bool.
        Eq if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 == rhs.f64 }),
        Ne if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 != rhs.f64 }),
        Lt if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 < rhs.f64 }),
        Le if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 <= rhs.f64 }),
        Gt if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 > rhs.f64 }),
        Ge if is_f64 => RawSlot::from_bool(unsafe { lhs.f64 >= rhs.f64 }),
        Eq => RawSlot::from_bool(unsafe { lhs.u64 == rhs.u64 }),
        Ne => RawSlot::from_bool(unsafe { lhs.u64 != rhs.u64 }),
        Lt if unsigned => RawSlot::from_bool(unsafe { lhs.u64 < rhs.u64 }),
        Le if unsigned => RawSlot::from_bool(unsafe { lhs.u64 <= rhs.u64 }),
        Gt if unsigned => RawSlot::from_bool(unsafe { lhs.u64 > rhs.u64 }),
        Ge if unsigned => RawSlot::from_bool(unsafe { lhs.u64 >= rhs.u64 }),
        Lt => RawSlot::from_bool(unsafe { lhs.i64 < rhs.i64 }),
        Le => RawSlot::from_bool(unsafe { lhs.i64 <= rhs.i64 }),
        Gt => RawSlot::from_bool(unsafe { lhs.i64 > rhs.i64 }),
        Ge => RawSlot::from_bool(unsafe { lhs.i64 >= rhs.i64 }),
        // Bitwise / shift: integer, width-agnostic on the raw bits.
        BitAnd => RawSlot::from_u64(unsafe { lhs.u64 & rhs.u64 }),
        BitOr => RawSlot::from_u64(unsafe { lhs.u64 | rhs.u64 }),
        BitXor => RawSlot::from_u64(unsafe { lhs.u64 ^ rhs.u64 }),
        Shl => RawSlot::from_u64(unsafe { lhs.u64.wrapping_shl(rhs.u64 as u32) }),
        Shr if unsigned => RawSlot::from_u64(unsafe { lhs.u64.wrapping_shr(rhs.u64 as u32) }),
        Shr => RawSlot::from_i64(unsafe { lhs.i64.wrapping_shr(rhs.u64 as u32) }),
        Min if is_f64 => RawSlot::from_f64(unsafe { lhs.f64.min(rhs.f64) }),
        Max if is_f64 => RawSlot::from_f64(unsafe { lhs.f64.max(rhs.f64) }),
        Min if unsigned => RawSlot::from_u64(unsafe { lhs.u64.min(rhs.u64) }),
        Max if unsigned => RawSlot::from_u64(unsafe { lhs.u64.max(rhs.u64) }),
        Min => RawSlot::from_i64(unsafe { lhs.i64.min(rhs.i64) }),
        Max => RawSlot::from_i64(unsafe { lhs.i64.max(rhs.i64) }),
        Pow => RawSlot::from_f64(unsafe { lhs.f64.powf(rhs.f64) }),
    }
}

fn eval_unaryop(op: UnaryOp, operand: RawSlot, ty: Type) -> RawSlot {
    // Same reason as `eval_binop`: `~0u8` is 255, not 18446744073709551615.
    let raw = eval_unaryop_raw(op, operand, ty);
    if matches!(op, UnaryOp::LogicalNot) {
        raw
    } else {
        normalise_narrow(raw, ty)
    }
}

fn eval_unaryop_raw(op: UnaryOp, operand: RawSlot, ty: Type) -> RawSlot {
    use compiler_ir::UnaryOp::*;
    match op {
        Neg if matches!(ty, Type::F64) => RawSlot::from_f64(unsafe { -(operand.f64) }),
        Neg => RawSlot::from_i64(unsafe { -(operand.i64) }),
        BitNot => RawSlot::from_u64(unsafe { !(operand.u64) }),
        LogicalNot => RawSlot::from_bool(unsafe { !(operand.bool) }),
        Abs if matches!(ty, Type::F64) => RawSlot::from_f64(unsafe { operand.f64.abs() }),
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

/// Integer type descriptor: `(bit_width, is_signed)`. `None` for
/// non-integer types (handled separately or passed through).
fn int_desc(ty: Type) -> Option<(u32, bool)> {
    match ty {
        Type::I8 => Some((8, true)),
        Type::U8 => Some((8, false)),
        Type::I16 => Some((16, true)),
        Type::U16 => Some((16, false)),
        Type::I32 => Some((32, true)),
        Type::U32 => Some((32, false)),
        Type::I64 => Some((64, true)),
        Type::U64 => Some((64, false)),
        Type::Bool => Some((8, false)),
        _ => None,
    }
}

/// Truncate `raw` low bits to `bits` and interpret per `signed`.
fn decode_int(raw: u64, bits: u32, signed: bool) -> i128 {
    if bits >= 64 {
        return if signed { raw as i64 as i128 } else { raw as i128 };
    }
    let mask = (1u64 << bits) - 1;
    let low = raw & mask;
    if signed && (low >> (bits - 1)) & 1 == 1 {
        // sign-extend
        (low as i128) - (1i128 << bits)
    } else {
        low as i128
    }
}

/// Encode integer `v` into a `RawSlot` of integer type `(bits, signed)`.
fn encode_int(v: i128, bits: u32, signed: bool) -> RawSlot {
    if bits >= 64 {
        return if signed {
            RawSlot::from_i64(v as i64)
        } else {
            RawSlot::from_u64(v as u64)
        };
    }
    let mask = (1u128 << bits) - 1;
    let low = (v as u128) & mask;
    if signed {
        // sign-extend the masked low bits to i64
        let sign = (low >> (bits - 1)) & 1 == 1;
        let ext = if sign { (low as i128) - (1i128 << bits) } else { low as i128 };
        RawSlot::from_i64(ext as i64)
    } else {
        RawSlot::from_u64(low as u64)
    }
}

fn eval_cast(value: RawSlot, from: Type, to: Type) -> RawSlot {
    // Float involvement is handled explicitly; everything else is an
    // integer-to-integer width/signedness conversion.
    match (int_desc(from), int_desc(to), from, to) {
        // int -> int (covers widening, narrowing, signed/unsigned reinterpret)
        (Some((fb, fs)), Some((tb, ts)), _, _) => {
            let v = decode_int(unsafe { value.u64 }, fb, fs);
            encode_int(v, tb, ts)
        }
        // int -> f64
        (Some((fb, fs)), None, _, Type::F64) => {
            let v = decode_int(unsafe { value.u64 }, fb, fs);
            RawSlot::from_f64(v as f64)
        }
        // f64 -> int (truncate toward zero)
        (None, Some((tb, ts)), Type::F64, _) => {
            let f = unsafe { value.f64 };
            encode_int(f as i128, tb, ts)
        }
        // f64 -> f64 and any other shape: pass through.
        _ => value,
    }
}

fn format_scalar(host: &dyn VmHost, slot: RawSlot, ty: Type) -> String {
    match ty {
        Type::I64 => format!("{}", unsafe { slot.i64 }),
        Type::U64 => format!("{}", unsafe { slot.u64 }),
        Type::I8 => format!("{}", unsafe { slot.i64 as i8 }),
        Type::U8 => format!("{}", unsafe { slot.u64 as u8 }),
        Type::I16 => format!("{}", unsafe { slot.i64 as i16 }),
        Type::U16 => format!("{}", unsafe { slot.u64 as u16 }),
        Type::I32 => format!("{}", unsafe { slot.i64 as i32 }),
        Type::U32 => format!("{}", unsafe { slot.u64 as u32 }),
        Type::F64 => crate::heap::format_f64(unsafe { slot.f64 }),
        Type::Bool => format!("{}", unsafe { slot.bool }),
        Type::Str => host.read_str(unsafe { slot.u64 }),
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