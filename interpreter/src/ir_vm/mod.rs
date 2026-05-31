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
mod lift;
pub mod slot;

use std::collections::HashMap;

use compiler_ir::{BlockId, Const, FuncId, InstKind, Instruction, LocalId, Module, Terminator, Type, ValueId};
use string_interner::Symbol;

use crate::object::Object;
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
    /// Value pool for the current function's SSA values.
    /// Flushed on every cross-block jump (simplest correct model).
    values: HashMap<ValueId, RawSlot>,
}

impl<'a> Vm<'a> {
    pub fn new(module: &'a Module) -> Self {
        Self {
            module,
            frames: Vec::new(),
            values: HashMap::new(),
        }
    }

    /// Execute the module starting from `main` (FuncId 0 by convention).
    pub fn run(&mut self) -> VmResult {
        let main_id = FuncId(0);
        // Verify main exists
        if main_id.0 as usize >= self.module.functions.len() {
            return VmResult::Diverged {
                message: "no main function".to_string(),
            };
        }
        self.call_function(main_id, Vec::new(), None);
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
            } else if let Some(term) = block.terminator.clone() {
                match term {
                    Terminator::Return(values) => {
                        let ret_slots: Vec<RawSlot> = values
                            .iter()
                            .map(|v| self.read_value(*v))
                            .collect();
                        let caller_dest = self.frames.last().and_then(|f| f.return_dest);
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
                    }
                    Terminator::Jump(target) => {
                        {
                            let frame = self.frames.last_mut().expect("frame vanished");
                            frame.block = target;
                            frame.pc = 0;
                        }
                        self.values.clear();
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
                        self.values.clear();
                    }
                    Terminator::Panic { message } => {
                        return VmResult::Diverged {
                            message: format!("panic #{}", message.to_usize()),
                        };
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
        *self.values.get(&id).expect("value not defined")
    }

    fn write_value(&mut self, id: ValueId, slot: RawSlot) {
        self.values.insert(id, slot);
    }

    fn read_local(&self, local: LocalId) -> RawSlot {
        let frame = self.frames.last().expect("no active frame");
        frame.read_local(local)
    }

    fn write_local(&mut self, local: LocalId, slot: RawSlot) {
        let frame = self.frames.last_mut().expect("no active frame");
        frame.write_local(local, slot);
    }

    /// Push a new call frame for `func_id` with `args` as the initial
    /// parameter locals. `return_dest` is the caller's `ValueId` that
    /// will receive the scalar return value.
    fn call_function(&mut self, func_id: FuncId, args: Vec<RawSlot>, return_dest: Option<ValueId>) {
        let func = &self.module.functions[func_id.0 as usize];
        let mut frame = CallFrame::new(func_id, func.params.len(), func.locals.len());
        frame.return_dest = return_dest;
        for (i, arg) in args.into_iter().enumerate() {
            frame.write_local(LocalId(i as u32), arg);
        }
        self.frames.push(frame);
    }
}

/// High-level entry: run a lowered IR module and return the exit code.
/// This is the IR VM path; the caller is responsible for AST → IR lowering.
pub fn run_module(module: &Module) -> Result<i64, String> {
    // Install a fresh runtime state for this execution.
    crate::runtime_state::RT.with(|s| {
        *s.borrow_mut() = Some(RuntimeState::new());
    });
    let result = {
        let mut vm = Vm::new(module);
        match vm.run() {
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
    use compiler_ir::{Const, Instruction, Linkage, Module, Terminator, Type, ValueId};
    use string_interner::{DefaultStringInterner, Symbol};

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
}
