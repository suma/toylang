//! Call frame for the IR VM.
//!
//! Each function invocation pushes one `CallFrame` onto the VM call stack.
//! The frame owns the flat `locals` array (`LocalId`-indexed) and tracks
//! the current `BlockId` + instruction offset (`pc`) within the function.

use compiler_ir::{BlockId, FuncId, LocalId, ValueId};

use super::slot::RawSlot;

pub struct CallFrame {
    /// The function being executed.
    pub func_id: FuncId,
    /// Flat local slot array. Indices `0..func.params.len()` are parameters.
    pub locals: Vec<RawSlot>,
    /// Current basic block.
    pub block: BlockId,
    /// Instruction index within the current block.
    pub pc: usize,
    /// When this frame is a callee, the caller's `ValueId` that should
    /// receive the scalar return value. `None` for Unit returns or when
    /// the caller doesn't bind the result.
    pub return_dest: Option<ValueId>,
}

impl CallFrame {
    pub fn new(func_id: FuncId, param_count: usize, local_count: usize) -> Self {
        let total = param_count.max(local_count);
        Self {
            func_id,
            locals: vec![RawSlot::default(); total],
            block: BlockId(0),
            pc: 0,
            return_dest: None,
        }
    }

    pub fn read_local(&self, id: LocalId) -> RawSlot {
        self.locals[id.0 as usize]
    }

    pub fn write_local(&mut self, id: LocalId, slot: RawSlot) {
        self.locals[id.0 as usize] = slot;
    }
}
