//! IR VM — interprets compiler IR directly without walking the AST.
//!
//! Phase 1 scope: scalar types (i64/u64/f64/bool + narrow ints),
//! arithmetic, comparison, if/while/for/return, and pure function calls.
//! Compound types (struct/tuple/enum) and advanced features are handled
//! in later phases.
//!
//! The VM receives a fully lowered `compiler_ir::Module` and executes it
//! using flat local slots (`RawSlot`) and a call stack of `CallFrame`s.

mod call;
mod dispatch;
pub mod eligibility;
pub mod frame;
mod heap;
pub mod lift;
pub mod slot;

use std::collections::HashMap;

use compiler_ir::{FuncId, LocalId, Module, Terminator, ValueId};
use string_interner::{DefaultStringInterner, DefaultSymbol, Symbol};

use crate::runtime_state::RuntimeState;

use frame::CallFrame;
use slot::RawSlot;

/// Result of executing a module.
pub enum VmResult {
    /// Normal termination with an exit code.
    ExitCode(i64),
    /// Divergence (panic, assert failure, or unreachable hit).
    Diverged { message: String },
}

/// VM execution engine.
pub struct Vm<'a> {
    module: &'a Module,
    /// Call stack. The bottom frame is `main`.
    frames: Vec<CallFrame>,
    /// Optional interner for resolving string symbols.
    interner: Option<&'a DefaultStringInterner>,
    /// Materialised vtables, keyed by `(trait_sym, struct_sym)`. Each is a
    /// heap address holding `Vec<FuncId>` entries (one U64 per method, in
    /// trait declaration order), matching the AOT vtable layout so a
    /// `PtrRead(vtable_ptr, idx*8, U64)` recovers the dispatch FuncId.
    vtable_addrs: HashMap<(DefaultSymbol, DefaultSymbol), u64>,
}

impl<'a> Vm<'a> {
    pub fn new(module: &'a Module) -> Self {
        Self {
            module,
            frames: Vec::new(),
            interner: None,
            vtable_addrs: HashMap::new(),
        }
    }

    pub fn with_interner(module: &'a Module, interner: &'a DefaultStringInterner) -> Self {
        Self {
            module,
            frames: Vec::new(),
            interner: Some(interner),
            vtable_addrs: HashMap::new(),
        }
    }

        /// Execute the module starting from `main` (FuncId 0 by convention).
    ///
    /// **Deprecated for multi-function modules**: `run_module` finds `main`
    /// by name and wires the correct `FuncId`. Use `run_module` instead.
    pub fn run(&mut self) -> VmResult {
        let main_id = FuncId(0);
        // Verify main exists
        if main_id.0 as usize >= self.module.functions.len() {
            return VmResult::Diverged {
                message: "no main function".to_string(),
            };
        }
        self.call_function(main_id, Vec::new(), None, Vec::new());
        self.run_loop()
    }

    fn run_loop(&mut self) -> VmResult {
        loop {
            // Snapshot frame info so we can drop the borrow before dispatch.
            let (func_id, block_id, pc) = match self.frames.last() {
                Some(f) => (f.func_id, f.block, f.pc),
                None => return VmResult::ExitCode(0),
            };
            let func = &self.module.functions[func_id.0 as usize];
            let block = &func.blocks[block_id.0 as usize];
            if pc < block.instructions.len() {
                let inst = block.instructions[pc].clone();
                // Advance pc while we have a mutable borrow.
                {
                    let frame = self.frames.last_mut().expect("frame vanished");
                    frame.pc += 1;
                }
                // Ordinary instruction — dispatch only, no terminator.
                dispatch::execute(self, &inst);
                // Record the result's static type so later type-polymorphic
                // ops (BinOp f64/i64/u64) can recover operand types.
                if let Some((vid, ty)) = inst.result {
                    self.frames
                        .last_mut()
                        .expect("frame vanished")
                        .value_types
                        .insert(vid, ty);
                }
            } else if let Some(term) = block.terminator.clone() {
                match term {
                    Terminator::Return(values) => {
                        let ret_slots: Vec<RawSlot> = values
                            .iter()
                            .map(|v| self.read_value(*v))
                            .collect();
                        let (caller_dest, caller_dests) = {
                            let frame = self.frames.last().expect("no active frame");
                            (frame.return_dest, frame.return_dests.clone())
                        };
                        self.frames.pop();
                        if self.frames.is_empty() {
                            // main returned — compute exit code
                            let code = if ret_slots.is_empty() {
                                0
                            } else {
                                unsafe { ret_slots[0].i64 }
                            };
                            return VmResult::ExitCode(code);
                        }
                        // Wire the scalar return into the caller's value map.
                        if let (Some(dest), Some(slot)) = (caller_dest, ret_slots.first()) {
                            self.write_value(dest, *slot);
                        }
                        // Wire compound return into the caller's locals.
                        if !caller_dests.is_empty() {
                            let frame = self.frames.last_mut().expect("no active frame");
                            for (i, dest) in caller_dests.iter().enumerate() {
                                if let Some(slot) = ret_slots.get(i) {
                                    frame.write_local(*dest, *slot);
                                }
                            }
                        }
                    }
                    Terminator::Jump(target) => {
                        {
                            let frame = self.frames.last_mut().expect("frame vanished");
                            frame.block = target;
                            frame.pc = 0;
                        }
                    }
                    Terminator::Branch { cond, then_blk, else_blk } => {
                        let cond_slot = self.read_value(cond);
                        let taken = unsafe { cond_slot.bool };
                        let target = if taken { then_blk } else { else_blk };
                        {
                            let frame = self.frames.last_mut().expect("frame vanished");
                            frame.block = target;
                            frame.pc = 0;
                        }
                    }
                    Terminator::Panic { message } => {
                        let text = self
                            .interner
                            .and_then(|i| i.resolve(message))
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("panic #{}", message.to_usize()));
                        return VmResult::Diverged { message: text };
                    }
                    Terminator::Unreachable => {
                        return VmResult::Diverged {
                            message: "unreachable".to_string(),
                        };
                    }
                }
            } else {
                // Block ended without terminator — shouldn't happen for valid IR.
                return VmResult::Diverged {
                    message: "unterminated block".to_string(),
                };
            }
        }
    }

    fn read_value(&self, id: ValueId) -> RawSlot {
        let frame = self.frames.last().expect("no active frame");
        *frame.values.get(&id).expect("value not defined")
    }

    /// Static type of a defined SSA value, if recorded.
    pub(super) fn value_type(&self, id: ValueId) -> Option<compiler_ir::Type> {
        self.frames.last().and_then(|f| f.value_types.get(&id).copied())
    }

    fn write_value(&mut self, id: ValueId, slot: RawSlot) {
        let frame = self.frames.last_mut().expect("no active frame");
        frame.values.insert(id, slot);
    }

    fn read_local(&self, local: LocalId) -> RawSlot {
        let frame = self.frames.last().expect("no active frame");
        if let Some(&addr) = frame.addr_cells.get(&local) {
            let ty = self.local_ty(frame.func_id, local);
            return heap::ptr_read(addr, 0, ty).unwrap_or_default();
        }
        frame.read_local(local)
    }

    fn write_local(&mut self, local: LocalId, slot: RawSlot) {
        let frame = self.frames.last().expect("no active frame");
        if let Some(&addr) = frame.addr_cells.get(&local) {
            let ty = self.local_ty(frame.func_id, local);
            heap::ptr_write(addr, 0, slot, ty);
            return;
        }
        let frame = self.frames.last_mut().expect("no active frame");
        frame.write_local(local, slot);
    }

    /// Type of `local` in `func_id`, defaulting to `U64` when out of range
    /// (e.g. parameter-only indices not mirrored into `locals`).
    fn local_ty(&self, func_id: FuncId, local: LocalId) -> compiler_ir::Type {
        self.module.functions[func_id.0 as usize]
            .locals
            .get(local.0 as usize)
            .copied()
            .unwrap_or(compiler_ir::Type::U64)
    }

    pub(super) fn current_frame(&self) -> &CallFrame {
        self.frames.last().expect("no active frame")
    }

    pub(super) fn current_frame_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("no active frame")
    }

    pub(super) fn interner(&self) -> Option<&DefaultStringInterner> {
        self.interner
    }

    /// Push a new call frame for `func_id` with `args` as the initial
    /// parameter locals. `return_dest` is the caller's `ValueId` that
    /// will receive the scalar return value. `return_dests` is used for
    /// `CallStruct`/`CallTuple`/`CallEnum` compound returns.
    fn call_function(
        &mut self,
        func_id: FuncId,
        args: Vec<RawSlot>,
        return_dest: Option<ValueId>,
        return_dests: Vec<LocalId>,
    ) {
        let func = &self.module.functions[func_id.0 as usize];
        let total_locals = func.locals.len().max(func.params.len());
        let mut frame = CallFrame::new(func_id, total_locals);
        frame.return_dest = return_dest;
        frame.return_dests = return_dests;
        // Allocate each array slot from the shared heap.
        for slot_info in &func.array_slots {
            let size = (slot_info.length as u64) * (slot_info.elem_stride_bytes as u64);
            let base = heap::heap_alloc(size);
            frame.array_bases.push(base);
        }
        // Back each address-taken local with a heap cell so `AddressOf`
        // yields a stable pointer and `&mut T` mutations propagate.
        for local in &func.address_taken_locals {
            let addr = heap::heap_alloc(8);
            frame.addr_cells.insert(*local, addr);
        }
        self.frames.push(frame);
        // Write params via the cell-aware path so address-taken params
        // land in their backing cells.
        for (i, arg) in args.into_iter().enumerate() {
            self.write_local(LocalId(i as u32), arg);
        }
    }

    pub(super) fn module(&self) -> &Module {
        self.module
    }

    /// Address of an address-taken local's backing heap cell. Allocates one
    /// lazily if the local wasn't pre-registered (defensive; `call_function`
    /// normally pre-allocates all `address_taken_locals`).
    pub(super) fn addr_of_local(&mut self, local: LocalId) -> u64 {
        if let Some(&addr) = self.current_frame().addr_cells.get(&local) {
            return addr;
        }
        let addr = heap::heap_alloc(8);
        self.current_frame_mut().addr_cells.insert(local, addr);
        addr
    }

    /// Materialise (and cache) the vtable for `(trait_sym, struct_sym)` as a
    /// heap buffer of FuncId entries, returning its address. Mirrors the AOT
    /// `toy_vtable_<trait>_<struct>` global: method `i` lives at offset `i*8`
    /// as a U64 holding the dispatch FuncId.
    pub(super) fn vtable_addr(&mut self, trait_sym: DefaultSymbol, struct_sym: DefaultSymbol) -> u64 {
        if let Some(addr) = self.vtable_addrs.get(&(trait_sym, struct_sym)) {
            return *addr;
        }
        let func_ids = self
            .module
            .vtables
            .get(&(trait_sym, struct_sym))
            .cloned()
            .unwrap_or_default();
        let addr = heap::heap_alloc((func_ids.len().max(1) as u64) * 8);
        for (i, fid) in func_ids.iter().enumerate() {
            heap::ptr_write(addr, (i * 8) as u64, RawSlot::from_u64(fid.0 as u64), compiler_ir::Type::U64);
        }
        self.vtable_addrs.insert((trait_sym, struct_sym), addr);
        addr
    }

    /// Materialise (and cache per-frame) the `&dyn Trait` coercion buffer for
    /// `slot_idx`, returning its heap address. The buffer is sized from the
    /// current function's `dyn_coerce_slots[slot_idx]` byte size.
    pub(super) fn dyn_coerce_addr(&mut self, slot_idx: u32) -> u64 {
        if let Some(addr) = self.current_frame().dyn_coerce_addrs.get(&slot_idx) {
            return *addr;
        }
        let func_id = self.current_frame().func_id;
        let size = self.module.functions[func_id.0 as usize]
            .dyn_coerce_slots
            .get(slot_idx as usize)
            .copied()
            .unwrap_or(0);
        let addr = heap::heap_alloc(size.max(1) as u64);
        self.current_frame_mut().dyn_coerce_addrs.insert(slot_idx, addr);
        addr
    }
}

/// High-level entry: run a lowered IR module and return the exit code.
/// This is the IR VM path; the caller is responsible for AST → IR lowering.
pub fn run_module(module: &Module) -> Result<i64, String> {
    run_module_with_interner(module, None)
}

/// Run a lowered IR module with an optional interner for string resolution.
pub fn run_module_with_interner(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
) -> Result<i64, String> {
    // Find main by export_name (works for both single-function and multi-function modules).
    let main_id = module.functions.iter().enumerate().find(|(_, f)| f.export_name == "main").map(|(i, _)| FuncId(i as u32)).ok_or("no main function")?;

    // Install a fresh runtime state for this execution.
    crate::runtime_state::RT.with(|s| {
        *s.borrow_mut() = Some(RuntimeState::new());
    });
    let result = {
        let mut vm = match interner {
            Some(i) => Vm::with_interner(module, i),
            None => Vm::new(module),
        };
        vm.call_function(main_id, Vec::new(), None, Vec::new());
        match vm.run_loop() {
            VmResult::ExitCode(code) => Ok(code),
            VmResult::Diverged { message } => Err(message),
        }
    };
    crate::runtime_state::RT.with(|s| {
        *s.borrow_mut() = None;
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use compiler_ir::{Const, Instruction, Linkage, Module, StructId, Terminator, Type, ValueId};
    use string_interner::DefaultStringInterner;

    #[test]
    fn vm_returns_constant_u64() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(42)),
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(0)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_returns_constant_i64() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::I64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::I64)),
            kind: compiler_ir::InstKind::Const(Const::I64(-7)),
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(0)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, -7);
    }

    #[test]
    fn vm_adds_two_constants() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(10)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(32)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(0),
                rhs: ValueId(1),
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_branch_takes_true_arm() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        let then_blk = func.add_block();
        let else_blk = func.add_block();
        func.entry = entry;

        // entry: cond = true; br cond, then, else
        let entry_block = func.block_mut(entry);
        entry_block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::Bool)),
            kind: compiler_ir::InstKind::Const(Const::Bool(true)),
        });
        entry_block.terminator = Some(Terminator::Branch {
            cond: ValueId(0),
            then_blk,
            else_blk,
        });

        // then: return 1
        let then_block = func.block_mut(then_blk);
        then_block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        then_block.terminator = Some(Terminator::Return(vec![ValueId(1)]));

        // else: return 2
        let else_block = func.block_mut(else_blk);
        else_block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(2)),
        });
        else_block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn vm_calls_function() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let add_sym = interner.get_or_intern("add");

        let mut module = Module::new();
        // main must be FuncId(0) because Vm::run hardcodes it.
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let add_id = module.declare_function(
            add_sym,
            "add".to_string(),
            Linkage::Local,
            vec![Type::U64, Type::U64],
            Type::U64,
        );
        {
            let func = module.function_mut(add_id);
            let entry = func.add_block();
            func.entry = entry;
            let block = func.block_mut(entry);
            // params are @l0 and @l1
            block.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(2), Type::U64)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Add,
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                },
            });
            block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
        }

        {
            let func = module.function_mut(main_id);
            let entry = func.add_block();
            func.entry = entry;
            let block = func.block_mut(entry);
            block.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(10)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(32)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(2), Type::U64)),
                kind: compiler_ir::InstKind::Call {
                    target: add_id,
                    args: vec![ValueId(0), ValueId(1)],
                },
            });
            block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_while_loop_counts_down() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        // Register locals so CallFrame allocates slots for them.
        let _acc = func.add_local(Type::U64);
        let _n = func.add_local(Type::U64);

        let entry = func.add_block();
        let body = func.add_block();
        let _exit = func.add_block();
        func.entry = entry;

        // entry: acc = 0; n = 5; jump body
        let entry_block = func.block_mut(entry);
        entry_block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(0)),
        });
        entry_block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(5)),
        });
        entry_block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(0),
            },
        });
        entry_block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(1),
                src: ValueId(1),
            },
        });
        entry_block.terminator = Some(Terminator::Jump(body));

        let loop_body = func.add_block();
        let after_loop = func.add_block();

        // body: load n; cond = n > 0; br cond, loop_body, after_loop
        let body_block = func.block_mut(body);
        body_block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        body_block.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(0)),
        });
        body_block.instructions.push(Instruction {
            result: Some((ValueId(4), Type::Bool)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Gt,
                lhs: ValueId(2),
                rhs: ValueId(3),
            },
        });
        body_block.terminator = Some(Terminator::Branch {
            cond: ValueId(4),
            then_blk: loop_body,
            else_blk: after_loop,
        });

        // loop_body: acc = acc + n; n = n - 1; jump body
        let lb = func.block_mut(loop_body);
        lb.instructions.push(Instruction {
            result: Some((ValueId(5), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        lb.instructions.push(Instruction {
            result: Some((ValueId(6), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        lb.instructions.push(Instruction {
            result: Some((ValueId(7), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(5),
                rhs: ValueId(6),
            },
        });
        lb.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(7),
            },
        });
        lb.instructions.push(Instruction {
            result: Some((ValueId(8), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        lb.instructions.push(Instruction {
            result: Some((ValueId(9), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        lb.instructions.push(Instruction {
            result: Some((ValueId(10), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Sub,
                lhs: ValueId(8),
                rhs: ValueId(9),
            },
        });
        lb.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(1),
                src: ValueId(10),
            },
        });
        lb.terminator = Some(Terminator::Jump(body));

        // after_loop: load acc; return acc
        let al = func.block_mut(after_loop);
        al.instructions.push(Instruction {
            result: Some((ValueId(11), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        al.terminator = Some(Terminator::Return(vec![ValueId(11)]));

        let result = run_module(&module).unwrap();
        // sum of 5+4+3+2+1 = 15
        assert_eq!(result, 15);
    }

    #[test]
    fn vm_factorial_via_loop() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let _result = func.add_local(Type::U64);
        let _i = func.add_local(Type::U64);
        let entry = func.add_block();
        let header = func.add_block();
        let body = func.add_block();
        let exit = func.add_block();
        func.entry = entry;

        // entry: result = 1; i = 5; jump header
        let e = func.block_mut(entry);
        e.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        e.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(5)),
        });
        e.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(0),
            },
        });
        e.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(1),
                src: ValueId(1),
            },
        });
        e.terminator = Some(Terminator::Jump(header));

        // header: load i; cond = i > 0; br cond, body, exit
        let h = func.block_mut(header);
        h.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        h.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(0)),
        });
        h.instructions.push(Instruction {
            result: Some((ValueId(4), Type::Bool)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Gt,
                lhs: ValueId(2),
                rhs: ValueId(3),
            },
        });
        h.terminator = Some(Terminator::Branch {
            cond: ValueId(4),
            then_blk: body,
            else_blk: exit,
        });

        // body: result = result * i; i = i - 1; jump header
        let b = func.block_mut(body);
        b.instructions.push(Instruction {
            result: Some((ValueId(5), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(6), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(7), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Mul,
                lhs: ValueId(5),
                rhs: ValueId(6),
            },
        });
        b.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(7),
            },
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(8), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(9), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(10), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Sub,
                lhs: ValueId(8),
                rhs: ValueId(9),
            },
        });
        b.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(1),
                src: ValueId(10),
            },
        });
        b.terminator = Some(Terminator::Jump(header));

        // exit: load result; return result
        let x = func.block_mut(exit);
        x.instructions.push(Instruction {
            result: Some((ValueId(11), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        x.terminator = Some(Terminator::Return(vec![ValueId(11)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 120);
    }

    #[test]
    fn vm_store_local_and_reload() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let _x = func.add_local(Type::U64);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(7)),
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(0),
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(3)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(1),
                rhs: ValueId(2),
            },
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(3),
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(4), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(4)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 10);
    }

    #[test]
    fn vm_cast_i64_to_f64_and_back() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::I64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::I64)),
            kind: compiler_ir::InstKind::Const(Const::I64(42)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::F64)),
            kind: compiler_ir::InstKind::Cast {
                value: ValueId(0),
                from: Type::I64,
                to: Type::F64,
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::I64)),
            kind: compiler_ir::InstKind::Cast {
                value: ValueId(1),
                from: Type::F64,
                to: Type::I64,
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_recursive_fib() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let fib_sym = interner.get_or_intern("fib");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let fib_id = module.declare_function(
            fib_sym,
            "fib".to_string(),
            Linkage::Local,
            vec![Type::U64],
            Type::U64,
        );

        // fib(n):
        {
            let func = module.function_mut(fib_id);
            // No body locals needed — all computation uses values directly.
            let entry = func.add_block();
            let recurse = func.add_block();
            let base = func.add_block();
            func.entry = entry;

            let e = func.block_mut(entry);
            e.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            e.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(1)),
            });
            e.instructions.push(Instruction {
                result: Some((ValueId(2), Type::Bool)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Le,
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                },
            });
            e.terminator = Some(Terminator::Branch {
                cond: ValueId(2),
                then_blk: base,
                else_blk: recurse,
            });

            let b = func.block_mut(base);
            b.instructions.push(Instruction {
                result: Some((ValueId(3), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            b.terminator = Some(Terminator::Return(vec![ValueId(3)]));

            let r = func.block_mut(recurse);
            r.instructions.push(Instruction {
                result: Some((ValueId(4), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(5), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(1)),
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(6), Type::U64)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Sub,
                    lhs: ValueId(4),
                    rhs: ValueId(5),
                },
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(7), Type::U64)),
                kind: compiler_ir::InstKind::Call {
                    target: fib_id,
                    args: vec![ValueId(6)],
                },
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(8), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(9), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(2)),
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(10), Type::U64)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Sub,
                    lhs: ValueId(8),
                    rhs: ValueId(9),
                },
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(11), Type::U64)),
                kind: compiler_ir::InstKind::Call {
                    target: fib_id,
                    args: vec![ValueId(10)],
                },
            });
            r.instructions.push(Instruction {
                result: Some((ValueId(12), Type::U64)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Add,
                    lhs: ValueId(7),
                    rhs: ValueId(11),
                },
            });
            r.terminator = Some(Terminator::Return(vec![ValueId(12)]));
        }

        // main: return fib(6)
        {
            let func = module.function_mut(main_id);
            let entry = func.add_block();
            func.entry = entry;
            let block = func.block_mut(entry);
            block.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(6)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::Call {
                    target: fib_id,
                    args: vec![ValueId(0)],
                },
            });
            block.terminator = Some(Terminator::Return(vec![ValueId(1)]));
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn vm_call_struct_returns_two_fields() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let make_sym = interner.get_or_intern("make_point");

        let mut module = Module::new();
        let make_id = module.declare_function(
            make_sym,
            "make_point".to_string(),
            Linkage::Local,
            vec![],
            Type::Struct(StructId(0)),
        );
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );

        // make_point(): return Point { x: 10, y: 20 }
        {
            let func = module.function_mut(make_id);
            func.add_local(Type::U64); // x
            func.add_local(Type::U64); // y
            let entry = func.add_block();
            func.entry = entry;
            let block = func.block_mut(entry);
            block.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(10)),
            });
            block.instructions.push(Instruction {
                result: None,
                kind: compiler_ir::InstKind::StoreLocal {
                    dst: LocalId(0),
                    src: ValueId(0),
                },
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::Const(Const::U64(20)),
            });
            block.instructions.push(Instruction {
                result: None,
                kind: compiler_ir::InstKind::StoreLocal {
                    dst: LocalId(1),
                    src: ValueId(1),
                },
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(2), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(3), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
            });
            block.terminator = Some(Terminator::Return(vec![ValueId(2), ValueId(3)]));
        }

        // main(): val p = make_point(); return p.x + p.y
        {
            let func = module.function_mut(main_id);
            func.add_local(Type::U64); // p.x
            func.add_local(Type::U64); // p.y
            let entry = func.add_block();
            func.entry = entry;
            let block = func.block_mut(entry);
            block.instructions.push(Instruction {
                result: None,
                kind: compiler_ir::InstKind::CallStruct {
                    target: make_id,
                    args: vec![],
                    dests: vec![LocalId(0), LocalId(1)],
                },
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
            });
            block.instructions.push(Instruction {
                result: Some((ValueId(2), Type::U64)),
                kind: compiler_ir::InstKind::BinOp {
                    op: compiler_ir::BinOp::Add,
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                },
            });
            block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 30);
    }

    #[test]
    fn vm_heap_alloc_ptr_write_read() {
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        func.add_local(Type::U64); // ptr
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);

        // ptr = heap_alloc(8)
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(8)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::HeapAlloc {
                size: ValueId(0),
                binding: compiler_ir::AllocatorBinding::Ambient,
            },
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(1),
            },
        });

        // ptr_write(ptr, 0, 42u64)
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(42)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(0)),
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::PtrWrite {
                ptr: ValueId(1),
                offset: ValueId(3),
                value: ValueId(2),
                value_ty: Type::U64,
            },
        });

        // val = ptr_read(ptr, 0, U64)
        block.instructions.push(Instruction {
            result: Some((ValueId(4), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(5), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(6), Type::U64)),
            kind: compiler_ir::InstKind::PtrRead {
                ptr: ValueId(4),
                offset: ValueId(5),
                elem_ty: Type::U64,
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(6)]));

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_capturing_closure_via_make_and_call_indirect() {
        use compiler_ir::{AllocatorBinding, BinOp, FuncId, InstKind};
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let body_sym = interner.get_or_intern("add_n_closure");

        let mut module = Module::new();
        // Lifted closure body: fn(env: u64, x: i64) -> i64 { x + *(env+8) }
        let body_id = module.declare_function(
            body_sym,
            "add_n_closure".to_string(),
            Linkage::Local,
            vec![Type::U64, Type::I64],
            Type::I64,
        );
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::I64,
        );

        // body: param[0] = env, param[1] = x; n = ptr_read(env, 8, I64); x + n
        {
            let func = module.function_mut(body_id);
            let entry = func.add_block();
            func.entry = entry;
            let b = func.block_mut(entry);
            b.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: InstKind::LoadLocal(LocalId(0)),
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: InstKind::Const(Const::U64(8)),
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(2), Type::I64)),
                kind: InstKind::PtrRead {
                    ptr: ValueId(0),
                    offset: ValueId(1),
                    elem_ty: Type::I64,
                },
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(3), Type::I64)),
                kind: InstKind::LoadLocal(LocalId(1)),
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(4), Type::I64)),
                kind: InstKind::BinOp {
                    op: BinOp::Add,
                    lhs: ValueId(3),
                    rhs: ValueId(2),
                },
            });
            b.terminator = Some(Terminator::Return(vec![ValueId(4)]));
        }

        // main: n = 10; cl = MakeClosure(add_n_closure, [n]); cl(5)
        {
            let func = module.function_mut(main_id);
            let entry = func.add_block();
            func.entry = entry;
            let m = func.block_mut(entry);
            m.instructions.push(Instruction {
                result: Some((ValueId(0), Type::I64)),
                kind: InstKind::Const(Const::I64(10)),
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: InstKind::MakeClosure {
                    target: body_id,
                    captures: vec![ValueId(0)],
                    capture_tys: vec![Type::I64],
                },
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(2), Type::I64)),
                kind: InstKind::Const(Const::I64(5)),
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(3), Type::I64)),
                kind: InstKind::CallIndirect {
                    callee: ValueId(1),
                    args: vec![ValueId(2)],
                    param_tys: vec![Type::I64],
                    ret_ty: Type::I64,
                },
            });
            m.terminator = Some(Terminator::Return(vec![ValueId(3)]));
            let _ = AllocatorBinding::Ambient;
            let _ = FuncId(0);
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 15);
    }

    #[test]
    fn vm_dyn_dispatch_via_vtable() {
        use compiler_ir::{BinOp, FuncId, InstKind};
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let thunk_sym = interner.get_or_intern("thunk_sound");
        let trait_sym = interner.get_or_intern("Animal");
        let struct_sym = interner.get_or_intern("Dog");

        let mut module = Module::new();
        // Thunk: fn(data_ptr: u64) -> i64 { 7 + (data_ptr & 0) }
        // (uses data_ptr trivially so the unused-arg path is exercised)
        let thunk_id = module.declare_function(
            thunk_sym,
            "thunk_sound".to_string(),
            Linkage::Local,
            vec![Type::U64],
            Type::I64,
        );
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::I64,
        );
        // Register the vtable + method order, mirroring the AOT module.
        module
            .vtables
            .insert((trait_sym, struct_sym), vec![thunk_id]);
        module
            .trait_method_order
            .insert(trait_sym, vec![interner.get_or_intern("sound")]);

        {
            let func = module.function_mut(thunk_id);
            let entry = func.add_block();
            func.entry = entry;
            let b = func.block_mut(entry);
            b.instructions.push(Instruction {
                result: Some((ValueId(0), Type::I64)),
                kind: InstKind::Const(Const::I64(7)),
            });
            b.terminator = Some(Terminator::Return(vec![ValueId(0)]));
        }

        // main: vtable = VtableAddr; fn_ptr = *(vtable+0); fn_ptr(data_ptr=0)
        {
            let func = module.function_mut(main_id);
            let entry = func.add_block();
            func.entry = entry;
            let m = func.block_mut(entry);
            m.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: InstKind::VtableAddr { trait_sym, struct_sym },
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: InstKind::Const(Const::U64(0)),
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(2), Type::U64)),
                kind: InstKind::PtrRead {
                    ptr: ValueId(0),
                    offset: ValueId(1),
                    elem_ty: Type::U64,
                },
            });
            // data_ptr = 0 (empty struct sentinel)
            m.instructions.push(Instruction {
                result: Some((ValueId(3), Type::U64)),
                kind: InstKind::Const(Const::U64(0)),
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(4), Type::I64)),
                kind: InstKind::CallIndirectFn {
                    callee: ValueId(2),
                    args: vec![ValueId(3)],
                    param_tys: vec![Type::U64],
                    ret_ty: Type::I64,
                },
            });
            m.terminator = Some(Terminator::Return(vec![ValueId(4)]));
            let _ = (BinOp::Add, FuncId(0));
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn vm_mut_ref_propagates_across_call() {
        // fn inc(p: &mut i64) { *p = *p + 1 }   (p is LocalId(0), a U64 ptr)
        // fn main() -> i64 { var v = 41; inc(&mut v); v }
        use compiler_ir::{BinOp, FuncId, InstKind};
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let inc_sym = interner.get_or_intern("inc");

        let mut module = Module::new();
        let inc_id = module.declare_function(
            inc_sym,
            "inc".to_string(),
            Linkage::Local,
            vec![Type::U64],
            Type::Unit,
        );
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::I64,
        );

        // inc: *p = *p + 1
        {
            let func = module.function_mut(inc_id);
            let entry = func.add_block();
            func.entry = entry;
            let b = func.block_mut(entry);
            b.instructions.push(Instruction {
                result: Some((ValueId(0), Type::U64)),
                kind: InstKind::LoadLocal(LocalId(0)),
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(1), Type::I64)),
                kind: InstKind::LoadRef { ptr: ValueId(0), ty: Type::I64 },
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(2), Type::I64)),
                kind: InstKind::Const(Const::I64(1)),
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(3), Type::I64)),
                kind: InstKind::BinOp { op: BinOp::Add, lhs: ValueId(1), rhs: ValueId(2) },
            });
            b.instructions.push(Instruction {
                result: Some((ValueId(4), Type::U64)),
                kind: InstKind::LoadLocal(LocalId(0)),
            });
            b.instructions.push(Instruction {
                result: None,
                kind: InstKind::StoreRef { ptr: ValueId(4), value: ValueId(3), ty: Type::I64 },
            });
            b.terminator = Some(Terminator::Return(vec![]));
        }

        // main: var v = 41 (address-taken); inc(&mut v); return v
        {
            let func = module.function_mut(main_id);
            let v = func.add_local(Type::I64); // LocalId(0)
            func.address_taken_locals.insert(v);
            let entry = func.add_block();
            func.entry = entry;
            let m = func.block_mut(entry);
            m.instructions.push(Instruction {
                result: Some((ValueId(0), Type::I64)),
                kind: InstKind::Const(Const::I64(41)),
            });
            m.instructions.push(Instruction {
                result: None,
                kind: InstKind::StoreLocal { dst: v, src: ValueId(0) },
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(1), Type::U64)),
                kind: InstKind::AddressOf { local: v },
            });
            m.instructions.push(Instruction {
                result: None,
                kind: InstKind::Call { target: inc_id, args: vec![ValueId(1)] },
            });
            m.instructions.push(Instruction {
                result: Some((ValueId(2), Type::I64)),
                kind: InstKind::LoadLocal(v),
            });
            m.terminator = Some(Terminator::Return(vec![ValueId(2)]));
            let _ = FuncId(0);
        }

        let result = run_module(&module).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn vm_panic_resolves_message_via_interner() {
        // A failing `requires`-style guard panics; the VM should surface
        // the interned message text, not a `panic #N` placeholder.
        use compiler_ir::InstKind;
        let mut interner = DefaultStringInterner::default();
        let main_sym = interner.get_or_intern("main");
        let msg_sym = interner.get_or_intern("requires violated: b != 0");

        let mut module = Module::new();
        let main_id = module.declare_function(
            main_sym,
            "main".to_string(),
            Linkage::Export,
            vec![],
            Type::U64,
        );
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        let fail = func.add_block();
        func.entry = entry;
        // entry: cond = false; br cond, <unused>, fail
        let e = func.block_mut(entry);
        e.instructions.push(Instruction {
            result: Some((ValueId(0), Type::Bool)),
            kind: InstKind::Const(Const::Bool(false)),
        });
        e.terminator = Some(Terminator::Branch { cond: ValueId(0), then_blk: entry, else_blk: fail });
        let f = func.block_mut(fail);
        f.terminator = Some(Terminator::Panic { message: msg_sym });

        // Run with interner so the panic message resolves.
        crate::runtime_state::RT.with(|s| {
            *s.borrow_mut() = Some(RuntimeState::new());
        });
        let mut vm = Vm::with_interner(&module, &interner);
        vm.call_function(main_id, Vec::new(), None, Vec::new());
        let res = vm.run_loop();
        crate::runtime_state::RT.with(|s| {
            *s.borrow_mut() = None;
        });
        match res {
            VmResult::Diverged { message } => {
                assert_eq!(message, "requires violated: b != 0");
            }
            _ => panic!("expected divergence"),
        }
    }

    #[test]
    fn vm_str_raw_byte_layout_round_trip() {
        // Pin the AOT-compatible `[bytes][NUL][u64 len]` layout: a str value
        // points at the len field, byte_start = value - len - 1, and the
        // bytes are readable from the raw buffer (as `__builtin_str_to_ptr`
        // + PtrRead(U8) would do).
        crate::runtime_state::RT.with(|s| {
            *s.borrow_mut() = Some(RuntimeState::new());
        });
        let v = super::heap::alloc_str_bytes(b"hi!");
        assert_eq!(super::heap::string_len(v), 3);
        assert_eq!(super::heap::read_str(v), "hi!");
        let byte_start = v - 3 - 1;
        // Byte-level reads through the same path PtrRead(U8) uses.
        let b0 = super::heap::ptr_read(byte_start, 0, compiler_ir::Type::U8).unwrap();
        let b2 = super::heap::ptr_read(byte_start, 2, compiler_ir::Type::U8).unwrap();
        assert_eq!(unsafe { b0.u64 }, b'h' as u64);
        assert_eq!(unsafe { b2.u64 }, b'!' as u64);
        // Concatenation preserves bytes and length.
        let w = super::heap::alloc_str_bytes(b"yo");
        let cat = super::heap::concat_strings(v, w);
        assert_eq!(super::heap::read_str(cat), "hi!yo");
        crate::runtime_state::RT.with(|s| {
            *s.borrow_mut() = None;
        });
    }
}
