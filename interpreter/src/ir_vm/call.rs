//! Call dispatch for the IR VM.
//!
//! Direct calls (`Call`, `CallStruct`, `CallTuple`, `CallEnum`) and
//! indirect calls (`CallIndirect`, `CallIndirectFn`, …) are handled here.
//! Phase 1 only supports scalar direct calls.

use compiler_ir::{FuncId, InstKind, LocalId, ValueId};

use crate::ir_vm::{RawSlot, Vm};

/// Handle a direct scalar call. The caller's `result` value (if any)
/// will be filled when the callee reaches `Terminator::Return`.
pub fn handle_call(vm: &mut Vm, target: FuncId, args: &[ValueId]) {
    let arg_slots: Vec<RawSlot> = args.iter().map(|a| vm.read_value(*a)).collect();
    vm.call_function(target, arg_slots, None);
}

/// When a `Return` terminator is reached, copy the returned scalar
/// values into the caller's value map so the next instruction can
/// use them.
pub fn handle_return(vm: &mut Vm, ret_values: &[ValueId]) -> Vec<RawSlot> {
    ret_values.iter().map(|v| vm.read_value(*v)).collect()
}
