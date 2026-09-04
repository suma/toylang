//! Call frame for the IR VM.
//!
//! Each function invocation pushes one `CallFrame` onto the VM call stack.
//! The frame owns the flat `locals` array (`LocalId`-indexed) and tracks
//! the current `BlockId` + instruction offset (`pc`) within the function.

use std::collections::HashMap;

use compiler_ir::{BlockId, FuncId, LocalId, Type, ValueId};

use super::slot::RawSlot;

pub struct CallFrame {
    /// The function being executed.
    pub func_id: FuncId,
    /// DEBUG-OBS D4: the backtrace frame this call entered, carried
    /// from the calling instruction. `None` for the entry function,
    /// which nothing called.
    pub frame: Option<compiler_ir::FrameId>,
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
    /// Per-array-slot base addresses (heap-allocated). Indexed by `ArraySlotId.0`.
    pub array_bases: Vec<u64>,
    /// Per-array-slot element stride, from the slot the lowering
    /// sized. Indexing used to re-derive this from the element type
    /// and got `bool` wrong (8 bytes against a slot packed at 1), so
    /// a `soa [T; N]`'s bool column wrote over its neighbour's bytes
    /// -- invisible while reads came from the typed-slot map, and a
    /// wrong answer once they came from the bytes (MEMORY-ACCESS M1).
    pub array_strides: Vec<u64>,
    /// Lazily materialised `&dyn Trait` coercion buffers, keyed by
    /// `Function::dyn_coerce_slots` index. Mirrors the AOT `StackSlot`
    /// reuse so repeated `DynCoerceSlotAddr` for the same slot returns a
    /// stable address within a frame.
    pub dyn_coerce_addrs: HashMap<u32, u64>,
    /// Heap-backed cells for address-taken locals (`Function::
    /// address_taken_locals`). When a local appears here, its canonical
    /// storage is the heap cell (so a pointer from `AddressOf` survives
    /// `LoadRef` / `StoreRef` and cross-call `&mut T` propagation) and
    /// `read_local` / `write_local` route through it.
    pub addr_cells: HashMap<LocalId, u64>,
    /// SSA value pool for this function.
    pub values: HashMap<ValueId, RawSlot>,
    /// Static type of each defined SSA value (from the instruction's
    /// `result` annotation). Used to dispatch type-polymorphic ops like
    /// `BinOp` (e.g. f64 vs i64 vs u64 arithmetic / comparison) since the
    /// `RawSlot` itself is untyped.
    pub value_types: HashMap<ValueId, Type>,
}

impl CallFrame {
    pub fn new(func_id: FuncId, total_locals: usize) -> Self {
        Self {
            func_id,
            frame: None,
            locals: vec![RawSlot::default(); total_locals],
            block: BlockId(0),
            pc: 0,
            return_dest: None,
            return_dests: Vec::new(),
            array_bases: Vec::new(),
            array_strides: Vec::new(),
            dyn_coerce_addrs: HashMap::new(),
            addr_cells: HashMap::new(),
            values: HashMap::new(),
            value_types: HashMap::new(),
        }
    }

    pub fn read_local(&self, id: LocalId) -> RawSlot {
        self.locals[id.0 as usize]
    }

    pub fn write_local(&mut self, id: LocalId, slot: RawSlot) {
        self.locals[id.0 as usize] = slot;
    }
}
