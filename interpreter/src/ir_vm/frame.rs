//! Call frame for the IR VM.
//!
//! Each function invocation pushes one `CallFrame` onto the VM call stack.
//! The frame owns the flat `locals` array (`LocalId`-indexed) and tracks
//! the current `BlockId` + instruction offset (`pc`) within the function.

use std::collections::HashMap;

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
    /// When this frame is a callee for `CallStruct`/`CallTuple`/`CallEnum`,
    /// the caller's `LocalId`s that should receive the compound return
    /// values (one per scalar leaf). Empty for scalar returns.
    pub return_dests: Vec<LocalId>,
    /// SSA value pool for this function.
    pub values: HashMap<ValueId, RawSlot>,
}

impl CallFrame {
    pub fn new(func_id: FuncId, total_locals: usize) -> Self {
        Self {
            func_id,
            locals: vec![RawSlot::default(); total_locals],
            block: BlockId(0),
            pc: 0,
            return_dest: None,
            return_dests: Vec::new(),
            values: HashMap::new(),
        }
    }

    pub fn read_local(&self, id: LocalId) -> RawSlot {
        self.locals[id.0 as usize]
    }

    pub fn write_local(&mut self, id: LocalId, slot: RawSlot) {
        self.locals[id.0 as usize] = slot;
    }
}
