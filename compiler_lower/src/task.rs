//! CONCURRENCY B2: a `spawn` starts its body as a task.
//!
//! After `outline_spawn_bodies` (frontend), a spawn is the call
//! `val __spawn_handle: u64 = __spawn_run_N(captures.., slot)`, where
//! `__spawn_run_N` runs the body and stores its value in `slot` (the
//! task's result `Vec`). Here that one call becomes
//! [`InstKind::TaskSpawn`] on a trampoline:
//!
//! ```text
//! fn tramp(env: u64) {            // built directly in IR
//!     __spawn_run_N(env[0], env[1], ..)   // one 8-byte slot per leaf
//! }
//! ```
//!
//! The arguments are already leaves by the time they are lowered, so
//! they go into a slot of this frame one per 8 bytes, as a `parallel
//! for`'s captures do, and the trampoline reads them back in order.
//! The runtime copies the slot before the thread starts (this frame
//! may return first). The IR VM, a sequential lane, calls the
//! trampoline on the spot and answers 0 — "already done".
//!
//! What the arguments *are* — moved owned values, copied scalars, the
//! slot's address — was settled in the frontend; this file only
//! carries them across.

use super::FunctionLower;
use crate::ir::{Const, FuncId, InstKind, Type, ValueId};

impl FunctionLower<'_> {
    /// Is `fn_name` the run function of a `spawn`?
    pub(super) fn is_spawn_run(&self, fn_name: string_interner::DefaultSymbol) -> bool {
        self.program.spawn_blocks.values().any(|site| site.run == fn_name)
    }

    /// The call `__spawn_run_N(args..)`, with `args` already lowered
    /// to leaves against `target`'s signature, as a task start.
    pub(super) fn lower_task_spawn(
        &mut self,
        target: FuncId,
        args: Vec<ValueId>,
    ) -> Result<Option<ValueId>, String> {
        let mut leaf_tys: Vec<Type> = Vec::new();
        for ty in self.module.function(target).params.clone() {
            crate::ir::layout::flatten_compound_leaf_types(self.module, ty, &mut leaf_tys);
        }
        if leaf_tys.len() != args.len() {
            return Err(format!(
                "internal error (CONCURRENCY B2): a spawn passes {} values to a body that takes {}",
                args.len(),
                leaf_tys.len()
            ));
        }

        let size = (args.len() * 8).max(8);
        let slot_idx = {
            let func = self.module.function_mut(self.func_id);
            let idx = func.dyn_coerce_slots.len() as u32;
            func.dyn_coerce_slots.push(size as u32);
            idx
        };
        let env = self
            .emit(InstKind::DynCoerceSlotAddr { slot_idx }, Some(Type::U64))
            .ok_or_else(|| "spawn env returned no value".to_string())?;
        for (i, (value, ty)) in args.iter().zip(leaf_tys.iter()).enumerate() {
            let offset = self
                .emit(InstKind::Const(Const::U64((i * 8) as u64)), Some(Type::U64))
                .ok_or_else(|| "spawn env offset returned no value".to_string())?;
            self.emit(InstKind::PtrWrite { ptr: env, offset, value: *value, value_ty: *ty }, None);
        }
        let size = self
            .emit(InstKind::Const(Const::U64(size as u64)), Some(Type::U64))
            .ok_or_else(|| "spawn env size returned no value".to_string())?;

        let tramp = self.build_spawn_trampoline(target, &leaf_tys);
        Ok(self.emit(InstKind::TaskSpawn { body: tramp, env, size }, Some(Type::U64)))
    }

    /// `fn(env: u64) { target(env[0], env[1], ..) }`, built directly:
    /// it is a handful of instructions with no source behind it.
    fn build_spawn_trampoline(&mut self, target: FuncId, leaf_tys: &[Type]) -> FuncId {
        let outer = self.module.function(self.func_id).export_name.clone();
        let counter = self.module.functions.len();
        let tramp = self.module.declare_function_anon(
            format!("{outer}__spawn_{counter}"),
            crate::ir::Linkage::Local,
            vec![Type::U64],
            Type::Unit,
        );
        // DEBUG-OBS: the body's own frame names it; this one is the
        // thread's entry and says so.
        self.module.set_display_name(tramp, "spawn".to_string());
        self.scheduled.insert(tramp);

        let mut next = 0u32;
        let mut fresh = || {
            let v = ValueId(next);
            next += 1;
            v
        };
        let mut instructions: Vec<crate::ir::Instruction> = Vec::new();
        let inst = |kind, result| crate::ir::Instruction { result, kind, frame: None };
        let f = self.module.function_mut(tramp);
        let env_local = f.add_local(Type::U64);
        let env = fresh();
        instructions.push(inst(InstKind::LoadLocal(env_local), Some((env, Type::U64))));
        let mut call_args = Vec::with_capacity(leaf_tys.len());
        for (i, ty) in leaf_tys.iter().enumerate() {
            let offset = fresh();
            instructions.push(inst(
                InstKind::Const(Const::U64((i * 8) as u64)),
                Some((offset, Type::U64)),
            ));
            let v = fresh();
            instructions.push(inst(InstKind::PtrRead { ptr: env, offset, elem_ty: *ty }, Some((v, *ty))));
            call_args.push(v);
        }
        let done = fresh();
        let ret_ty = self.module.function(target).return_type;
        let frame = Some(self.module.intern_frame(
            &self.module.frame_name(target),
            Default::default(),
        ));
        instructions.push(crate::ir::Instruction {
            result: Some((done, ret_ty)),
            kind: InstKind::Call { target, args: call_args },
            frame,
        });
        let f = self.module.function_mut(tramp);
        f.entry = crate::ir::BlockId(0);
        f.blocks.push(crate::ir::Block {
            id: crate::ir::BlockId(0),
            instructions,
            terminator: Some(crate::ir::Terminator::Return(Vec::new())),
        });
        tramp
    }
}
