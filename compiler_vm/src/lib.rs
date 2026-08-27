//! IR VM — interprets compiler IR directly without walking the AST.
//!
//! The VM receives a fully lowered `compiler_ir::Module` and executes
//! it using flat local slots (`RawSlot`) and a call stack of
//! `CallFrame`s. It is the execution engine shared by the
//! interpreter's run-time fast path and the compile-time fold
//! (COMPILE-TIME-EVAL C6): both lower a type-checked program and run
//! it through [`Vm`], so a compile-time answer and a run-time answer
//! cannot differ by construction.
//!
//! The VM has no knowledge of the interpreter — its stdout, heap and
//! allocator stack all arrive through the [`host::VmHost`] interface.
//! Everything else (scalar types, arithmetic, comparison,
//! if/while/for/return, function calls, compound types, closures,
//! `dyn Trait` dispatch) lives in the IR, which is all this crate
//! sees.

mod call;
mod dispatch;
pub mod eligibility;
pub mod frame;
pub mod heap;
pub mod host;
pub mod slot;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use compiler_ir::{FuncId, LocalId, Module, Terminator, ValueId};
use string_interner::{DefaultStringInterner, DefaultSymbol, Symbol};

use frame::CallFrame;
use host::VmHost;
use slot::RawSlot;

/// Result of executing a module.
pub enum VmResult {
    /// Normal termination with an exit code.
    ExitCode(i64),
    /// Divergence (panic, assert failure, or unreachable hit).
    ///
    /// `site` is where it happened (DEBUG-OBS D3), when the lowering
    /// pass knew. Callers on the run-time path render it through
    /// `Module::render_diagnostic`; the compile-time fold keeps the
    /// bare message, since a `const` initialiser's failure is reported
    /// as a compile error with its own position.
    Diverged { message: String, site: Option<compiler_ir::SiteId> },
}

/// VM execution engine.
pub struct Vm<'a> {
    module: &'a Module,
    /// Call stack. The bottom frame is the entry function.
    frames: Vec<CallFrame>,
    /// Optional interner for resolving string symbols.
    interner: Option<&'a DefaultStringInterner>,
    /// Everything the VM cannot do itself: stdout, heap, allocator
    /// stack, allocation counters.
    host: &'a dyn VmHost,
    /// Materialised vtables, keyed by `(trait_sym, struct_sym)`. Each is a
    /// heap address holding `Vec<FuncId>` entries (one U64 per method, in
    /// trait declaration order), matching the AOT vtable layout so a
    /// `PtrRead(vtable_ptr, idx*8, U64)` recovers the dispatch FuncId.
    vtable_addrs: HashMap<(DefaultSymbol, DefaultSymbol), u64>,
    /// When the entry function returns a compound value, the full flat leaf
    /// list is preserved here so the caller can reconstruct it.
    main_return_slots: Vec<RawSlot>,
    /// COMPILE-TIME-EVAL C6: cap on loop back-edges, and how many have
    /// been taken. `None` (the run-time path) removes the cap; the
    /// compile-time fold sets one so a non-terminating `const`
    /// initialiser stops the compile instead of hanging it.
    step_budget: Option<u64>,
    steps: u64,
}

impl<'a> Vm<'a> {
    pub fn new(module: &'a Module, host: &'a dyn VmHost) -> Self {
        Self {
            module,
            frames: Vec::new(),
            interner: None,
            host,
            vtable_addrs: HashMap::new(),
            main_return_slots: Vec::new(),
            step_budget: None,
            steps: 0,
        }
    }

    pub fn with_interner(
        module: &'a Module,
        interner: &'a DefaultStringInterner,
        host: &'a dyn VmHost,
    ) -> Self {
        Self {
            module,
            frames: Vec::new(),
            interner: Some(interner),
            host,
            vtable_addrs: HashMap::new(),
            main_return_slots: Vec::new(),
            step_budget: None,
            steps: 0,
        }
    }

    /// Cap how many loop back-edges this run may take
    /// (CHECK-NONTERMINATION, COMPILE-TIME-EVAL C6). `None` removes
    /// the cap, which is what every run-time path wants.
    pub fn set_step_budget(&mut self, budget: Option<u64>) {
        self.steps = 0;
        self.step_budget = budget;
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
                site: None,
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
            // A body-less callee (extern / `Import` linkage) cannot be
            // executed: diverge cleanly so the caller falls back to the
            // tree-walker for this program. Indexing an empty `blocks`
            // would otherwise panic.
            let Some(block) = func.blocks.get(block_id.0 as usize) else {
                return VmResult::Diverged {
                    site: None,
                    message: format!(
                        "ir_vm: cannot execute body-less function `{}` (extern?)",
                        func.export_name
                    ),
                };
            };
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
                            // entry returned — compute exit code
                            let code = if ret_slots.is_empty() {
                                0
                            } else {
                                unsafe { ret_slots[0].i64 }
                            };
                            self.main_return_slots = ret_slots;
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
                        if let Some(message) = self.charge_back_edge(target) {
                            return VmResult::Diverged { message, site: None };
                        }
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
                        if let Some(message) = self.charge_back_edge(target) {
                            return VmResult::Diverged { message, site: None };
                        }
                        {
                            let frame = self.frames.last_mut().expect("frame vanished");
                            frame.block = target;
                            frame.pc = 0;
                        }
                    }
                    Terminator::Panic { message, site } => {
                        let text = self
                            .interner
                            .and_then(|i| i.resolve(message))
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("panic #{}", message.to_usize()));
                        return VmResult::Diverged { message: text, site };
                    }
                    // ALLOC-CONTRACT-SUGAR: the numbers, not a fixed
                    // string. Same wording the tree-walker and the
                    // compiled binary produce.
                    Terminator::PanicAllocBudget { stat, entry, current, limit, site } => {
                        let entry = unsafe { self.read_value(entry).u64 };
                        let current = unsafe { self.read_value(current).u64 };
                        let limit = unsafe { self.read_value(limit).u64 };
                        return VmResult::Diverged {
                            message: compiler_ir::format_alloc_budget_violation(
                                stat, entry, current, limit,
                            ),
                            site,
                        };
                    }
                    Terminator::Unreachable => {
                        return VmResult::Diverged {
                            message: "unreachable".to_string(),
                            site: None,
                        };
                    }
                }
            } else {
                // Block ended without terminator — shouldn't happen for valid IR.
                return VmResult::Diverged {
                    message: "unterminated block".to_string(),
                    site: None,
                };
            }
        }
    }

    /// Count one loop back-edge — a jump to a block created no later
    /// than the current one — and fail the run once the step budget is
    /// spent. COMPILE-TIME-EVAL C6: the fold runs user code at compile
    /// time, so a `while true { }` in a `const` initialiser must stop
    /// the compile rather than hang it. The message matches the
    /// tree-walker's `StepBudgetExceeded` wording.
    ///
    /// Loop bodies jump back to a header block created before them, so
    /// a strictly-forward jump is never a back-edge; a jump to the
    /// current block itself (a hand-written self-loop) is counted too.
    fn charge_back_edge(&mut self, target: compiler_ir::BlockId) -> Option<String> {
        let max = self.step_budget?;
        let current = {
            let frame = self.frames.last().expect("no active frame");
            frame.block
        };
        if target.0 > current.0 {
            return None;
        }
        self.steps += 1;
        if self.steps > max {
            Some(format!("step budget exceeded ({max} loop iterations)"))
        } else {
            None
        }
    }

    fn read_value(&self, id: ValueId) -> RawSlot {
        let frame = self.frames.last().expect("no active frame");
        *frame.values.get(&id).expect("value not defined")
    }

    /// Static type of a defined SSA value, if recorded.
    pub(crate) fn value_type(&self, id: ValueId) -> Option<compiler_ir::Type> {
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
            return self.host.ptr_read(addr, 0, ty).unwrap_or_default();
        }
        frame.read_local(local)
    }

    fn write_local(&mut self, local: LocalId, slot: RawSlot) {
        let frame = self.frames.last().expect("no active frame");
        if let Some(&addr) = frame.addr_cells.get(&local) {
            let ty = self.local_ty(frame.func_id, local);
            self.host.ptr_write(addr, 0, slot, ty);
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

    pub(crate) fn current_frame(&self) -> &CallFrame {
        self.frames.last().expect("no active frame")
    }

    pub(crate) fn current_frame_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("no active frame")
    }

    pub(crate) fn interner(&self) -> Option<&DefaultStringInterner> {
        self.interner
    }

    pub(crate) fn host(&self) -> &dyn VmHost {
        self.host
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
            let base = self.host.alloc_at(size, 0);
            frame.array_bases.push(base);
        }
        // Back each address-taken local with a heap cell so `AddressOf`
        // yields a stable pointer and `&mut T` mutations propagate.
        for local in &func.address_taken_locals {
            let addr = self.host.alloc_at(8, 0);
            frame.addr_cells.insert(*local, addr);
        }
        self.frames.push(frame);
        // Write params via the cell-aware path so address-taken params
        // land in their backing cells.
        for (i, arg) in args.into_iter().enumerate() {
            self.write_local(LocalId(i as u32), arg);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn module(&self) -> &Module {
        self.module
    }

    /// Address of an address-taken local's backing heap cell. Allocates one
    /// lazily if the local wasn't pre-registered (defensive; `call_function`
    /// normally pre-allocates all `address_taken_locals`).
    pub(crate) fn addr_of_local(&mut self, local: LocalId) -> u64 {
        if let Some(&addr) = self.current_frame().addr_cells.get(&local) {
            return addr;
        }
        let addr = self.host.alloc_at(8, 0);
        self.current_frame_mut().addr_cells.insert(local, addr);
        addr
    }

    /// Materialise (and cache) the vtable for `(trait_sym, struct_sym)` as a
    /// heap buffer of FuncId entries, returning its address. Mirrors the AOT
    /// `toy_vtable_<trait>_<struct>` global: method `i` lives at offset `i*8`
    /// as a U64 holding the dispatch FuncId.
    pub(crate) fn vtable_addr(&mut self, trait_sym: DefaultSymbol, struct_sym: DefaultSymbol) -> u64 {
        if let Some(addr) = self.vtable_addrs.get(&(trait_sym, struct_sym)) {
            return *addr;
        }
        let func_ids = self
            .module
            .vtables
            .get(&(trait_sym, struct_sym))
            .cloned()
            .unwrap_or_default();
        let addr = self.host.alloc_at((func_ids.len().max(1) as u64) * 8, 0);
        for (i, fid) in func_ids.iter().enumerate() {
            self.host.ptr_write(
                addr,
                (i * 8) as u64,
                RawSlot::from_u64(fid.0 as u64),
                compiler_ir::Type::U64,
            );
        }
        self.vtable_addrs.insert((trait_sym, struct_sym), addr);
        addr
    }

    /// Materialise (and cache per-frame) the `&dyn Trait` coercion buffer for
    /// `slot_idx`, returning its heap address. The buffer is sized from the
    /// current function's `dyn_coerce_slots[slot_idx]` byte size.
    pub(crate) fn dyn_coerce_addr(&mut self, slot_idx: u32) -> u64 {
        if let Some(addr) = self.current_frame().dyn_coerce_addrs.get(&slot_idx) {
            return *addr;
        }
        let func_id = self.current_frame().func_id;
        let size = self.module.functions[func_id.0 as usize]
            .dyn_coerce_slots
            .get(slot_idx as usize)
            .copied()
            .unwrap_or(0);
        let addr = self.host.alloc_at(size.max(1) as u64, 0);
        self.current_frame_mut().dyn_coerce_addrs.insert(slot_idx, addr);
        addr
    }
}

/// High-level entry: run a lowered IR module and return the exit code.
/// This is the IR VM path; the caller is responsible for AST → IR lowering.
pub fn run_module(module: &Module, host: &dyn VmHost) -> Result<i64, String> {
    run_module_with_interner(module, None, host)
}

/// Run a lowered IR module with an optional interner for string resolution.
pub fn run_module_with_interner(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
    host: &dyn VmHost,
) -> Result<i64, String> {
    run_module_capturing(module, interner, host, false).map(|(code, _, _)| code)
}

/// Like [`run_module_with_interner`] but, when `want_str` is set, also reads
/// the `main` exit value as a `str` (the heap-backed bytes are read **before**
/// the host's run state is torn down). `want_str` must only be set when `main`
/// genuinely returns `str` — otherwise a scalar value would be misread as a
/// string handle (garbage length).
///
/// Also returns the full flat leaf list (`main_return_slots`) so compound
/// main returns can be reconstructed by the caller.
pub fn run_module_capturing(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
    host: &dyn VmHost,
    want_str: bool,
) -> Result<(i64, String, Vec<RawSlot>), String> {
    // Find main by export_name (works for both single-function and multi-function modules).
    let main_id = module
        .functions
        .iter()
        .enumerate()
        .find(|(_, f)| f.export_name == "main")
        .map(|(i, _)| FuncId(i as u32))
        .ok_or("no main function")?;

    {
        let mut vm = match interner {
            Some(i) => Vm::with_interner(module, i, host),
            None => Vm::new(module, host),
        };
        vm.call_function(main_id, Vec::new(), None, Vec::new());
        match vm.run_loop() {
            VmResult::ExitCode(code) => {
                // Read the value as a str (only when requested) while the
                // heap is still alive.
                let s = if want_str {
                    host.read_str(code as u64)
                } else {
                    String::new()
                };
                Ok((code, s, vm.main_return_slots))
            }
            // DEBUG-OBS D3: the run-time path renders the position
            // into the diagnostic here, so the IR VM no longer depends
            // on a tree-walker replay to say where a panic happened.
            //
            // The `panic: ` prefix goes on here rather than at the
            // terminator because the compile-time fold shares that
            // code and reports a failed `const` initialiser in its own
            // words (`[E0017] ... evaluating it failed: <message>`).
            VmResult::Diverged { message, site } => {
                Err(module.render_diagnostic(site, &format!("panic: {message}")))
            }
        }
    }
}

/// Run one function with pre-evaluated argument slots and return its
/// return slots.
///
/// COMPILE-TIME-EVAL C6: the compile-time fold evaluates `const fn`
/// calls (and `const` initialisers, through a synthetic wrapper) via
/// this — the same engine the run-time path uses. `budget` caps loop
/// back-edges; `None` removes the cap.
pub fn run_function(
    module: &Module,
    interner: Option<&DefaultStringInterner>,
    host: &dyn VmHost,
    func_id: FuncId,
    args: Vec<RawSlot>,
    budget: Option<u64>,
) -> Result<Vec<RawSlot>, String> {
    let mut vm = match interner {
        Some(i) => Vm::with_interner(module, i, host),
        None => Vm::new(module, host),
    };
    vm.set_step_budget(budget);
    vm.call_function(func_id, args, None, Vec::new());
    match vm.run_loop() {
        VmResult::ExitCode(_) => Ok(vm.main_return_slots),
        // The compile-time fold reports its own position (the `const`
        // initialiser), so the bare message is what it wants.
        VmResult::Diverged { message, .. } => Err(message),
    }
}