//! CONCURRENCY A2-b-2: `parallel for` becomes an outlined function.
//!
//! A1 shipped the syntax and ran the iterations in order on all four
//! lanes, so the answer is already pinned. This pass is the other
//! half: the body moves into a function of its own, what it read from
//! the enclosing scope travels in an environment, and the range is
//! handed to `InstKind::ParFor` — which AOT and the JIT turn into
//! `toy_par_for` (threads) and the IR VM runs as one chunk.
//!
//! **Why here and not in the frontend.** The frontend cannot mint a
//! declaration; its rewrites (`?`, `??`, `Display`) all swap one
//! expression for another. The lowering mints functions routinely —
//! monomorphisation, drop glue and closures each declare one and fill
//! the body from a work queue — and it has the types the capture
//! classification needs.
//!
//! **What travels, and what that costs.** Captures are *copied* into
//! the environment. A scalar copy is a scalar copy; a struct copy is
//! its leaves, which for a window (`Span<T>` / `Column<T>`) means the
//! address and the length, so `set` still writes to the one buffer
//! the caller owns. That is the whole trick, and it is also the whole
//! limit: **a body that writes to its own copy is refused**
//! ([`FunctionLower::reject_capture_writes`]), because the write
//! would be lost here and would be a race if it weren't. The check is
//! on the emitted IR rather than the source, so it sees a field
//! assignment, a `&mut` escape and a method's self-writeback alike.
//!
//! **Counts, not indices.** `ParFor` is given `0..count`; the body
//! adds the loop's own base back. The runtime therefore splits an
//! unsigned count whatever the loop variable's type is, and a range
//! whose start is negative needs no special case.

use frontend::ast::ExprRef;
use string_interner::DefaultSymbol;

use super::bindings::{self, Binding};
use super::FunctionLower;
use crate::ir::{BinOp, Const, FuncId, InstKind, LocalId, Terminator, Type, ValueId};

/// One outlined `parallel for` body awaiting lowering.
///
/// The shape mirrors [`crate::PendingClosureBody`]: the `FuncId` was
/// declared when the loop was lowered, and the body lowers later
/// under its own `FunctionLower`.
pub(crate) struct PendingParBody {
    pub(crate) func_id: FuncId,
    /// The loop variable. Bound inside the body to `base + k`.
    pub(crate) var_name: DefaultSymbol,
    /// Its IR type — also the type of the base slot in the env.
    pub(crate) var_ty: Type,
    pub(crate) body: ExprRef,
    /// In env-slot order. Each entry says how to rebuild one name.
    pub(crate) captures: Vec<ParCapture>,
}

/// What [`reject_capture_writes`] needs to judge one outlined body,
/// kept until after the module is complete.
///
/// The check runs at the very end because
/// [`crate::writeback_prune`] is what decides whether a `&mut self`
/// call actually wrote anything: `v.set(i, x)` returns every leaf of
/// `v` by ABI and writes none of them, and that tail is pruned. Ask
/// before the pruning and the most ordinary line in a parallel body
/// — writing one slot of a vector — looks like a mutation of the
/// vector.
pub(crate) struct ParCheck {
    pub(crate) func_id: FuncId,
    /// The prologue's block. Its writes are the copies themselves.
    pub(crate) entry: crate::ir::BlockId,
    /// Each capture leaf local, and the name it belongs to.
    pub(crate) capture_locals: Vec<(LocalId, DefaultSymbol)>,
}

/// One captured name, and how many env slots it occupies.
pub(crate) enum ParCapture {
    Scalar { name: DefaultSymbol, ty: Type },
    /// A struct binding, copied leaf by leaf. `struct_id` is enough
    /// to rebuild the field tree on the other side:
    /// `allocate_struct_fields` walks the same definition, so the
    /// leaves come back in the order they were written.
    Struct {
        name: DefaultSymbol,
        struct_id: crate::ir::StructId,
        leaf_tys: Vec<Type>,
    },
}

impl ParCapture {
    fn name(&self) -> DefaultSymbol {
        match self {
            ParCapture::Scalar { name, .. } | ParCapture::Struct { name, .. } => *name,
        }
    }

    fn slots(&self) -> usize {
        match self {
            ParCapture::Scalar { .. } => 1,
            ParCapture::Struct { leaf_tys, .. } => leaf_tys.len(),
        }
    }
}

impl FunctionLower<'_> {
    /// Lower `parallel for var in start..end { body }` by outlining.
    ///
    /// Returns `Ok(None)` like any other statement; everything it
    /// produces is the environment, the `ParFor` and a queued body.
    pub(super) fn lower_par_for(
        &mut self,
        var_name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
        body: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let var_ty = self.value_scalar(start).unwrap_or(Type::U64);
        if !var_ty.is_integer() {
            return Err(format!(
                "compiler MVP: a `parallel for` counts over an integer range; this one counts \
                 over `{}`",
                crate::spelling::spell_type(self.module, self.interner, var_ty)
            ));
        }
        let start_v = self
            .lower_expr(start)?
            .ok_or_else(|| "parallel for start produced no value".to_string())?;
        let end_v = self
            .lower_expr(end)?
            .ok_or_else(|| "parallel for end produced no value".to_string())?;

        let captures = self.collect_par_captures(var_name, body)?;

        // Declare the body. `(env, from, until) -> ()` is
        // `toy_par_for`'s callback ABI, and the IR VM calls it with
        // the same three arguments.
        let outer_name = self.module.function(self.func_id).export_name.clone();
        let counter = self.module.functions.len();
        let export_name = format!("{outer_name}__par_body_{counter}");
        let func_id = self.module.declare_function_anon(
            export_name,
            crate::ir::Linkage::Local,
            vec![Type::U64, Type::U64, Type::U64],
            Type::Unit,
        );
        // DEBUG-OBS: a backtrace through a parallel body should name
        // the loop, not the mangled function the lowering made.
        self.module
            .set_display_name(func_id, "parallel for".to_string());

        // The environment: every capture's leaves, then the loop's
        // base, in one 8-byte slot each.
        //
        // It lives in **this frame**, not the heap. `toy_par_for`
        // joins before it returns, so the frame outlives every
        // thread that reads it — and a loop in a hot path would
        // otherwise leak an environment per execution. The same
        // stack-slot machinery a `&dyn Trait` coercion uses
        // (`dyn_coerce_slots`), which is why no lane needed a new
        // instruction for it.
        let mut capture_vals: Vec<ValueId> = Vec::new();
        let mut capture_tys: Vec<Type> = Vec::new();
        for capture in &captures {
            match capture {
                ParCapture::Scalar { name, ty } => {
                    let v = self.load_capture_scalar(*name, *ty)?;
                    capture_vals.push(v);
                    capture_tys.push(*ty);
                }
                ParCapture::Struct { name, .. } => {
                    let Some(Binding::Struct { fields, .. }) = self.bindings.get(name).cloned()
                    else {
                        return Err(format!(
                            "internal error (CONCURRENCY A2-b-2): `{}` is no longer a struct \
                             binding",
                            self.interner.resolve(*name).unwrap_or("?")
                        ));
                    };
                    for (local, ty) in bindings::flatten_struct_locals(&fields) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .ok_or_else(|| "capture LoadLocal returned no value".to_string())?;
                        capture_vals.push(v);
                        capture_tys.push(ty);
                    }
                }
            }
        }
        capture_vals.push(start_v);
        capture_tys.push(var_ty);
        let slot_bytes = (capture_vals.len() * 8).max(8) as u32;
        let slot_idx = {
            let func = self.module.function_mut(self.func_id);
            let idx = func.dyn_coerce_slots.len() as u32;
            func.dyn_coerce_slots.push(slot_bytes);
            idx
        };
        let env = self
            .emit(InstKind::DynCoerceSlotAddr { slot_idx }, Some(Type::U64))
            .ok_or_else(|| "parallel for env returned no value".to_string())?;
        for (i, (value, ty)) in capture_vals.iter().zip(capture_tys.iter()).enumerate() {
            let offset = self
                .emit(InstKind::Const(Const::U64((i * 8) as u64)), Some(Type::U64))
                .ok_or_else(|| "parallel for env offset returned no value".to_string())?;
            self.emit(
                InstKind::PtrWrite {
                    ptr: env,
                    offset,
                    value: *value,
                    value_ty: *ty,
                },
                None,
            );
        }

        // How many iterations. An empty or backwards range is zero —
        // the same answer the sequential header gives on its first
        // test, and it keeps the runtime from seeing a wrapped count.
        let count_local = self.module.function_mut(self.func_id).add_local(Type::U64);
        let zero = self
            .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
            .ok_or_else(|| "parallel for zero returned no value".to_string())?;
        self.emit(
            InstKind::StoreLocal {
                dst: count_local,
                src: zero,
            },
            None,
        );
        let nonempty = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Lt,
                    lhs: start_v,
                    rhs: end_v,
                },
                Some(Type::Bool),
            )
            .ok_or_else(|| "parallel for range test returned no value".to_string())?;
        let count_blk = self.fresh_block();
        let call_blk = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: nonempty,
            then_blk: count_blk,
            else_blk: call_blk,
        });
        self.switch_to(count_blk);
        let diff = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Sub,
                    lhs: end_v,
                    rhs: start_v,
                },
                Some(var_ty),
            )
            .ok_or_else(|| "parallel for count returned no value".to_string())?;
        let diff_u64 = if var_ty == Type::U64 {
            diff
        } else {
            self.emit(
                InstKind::Cast {
                    value: diff,
                    from: var_ty,
                    to: Type::U64,
                },
                Some(Type::U64),
            )
            .ok_or_else(|| "parallel for count cast returned no value".to_string())?
        };
        self.emit(
            InstKind::StoreLocal {
                dst: count_local,
                src: diff_u64,
            },
            None,
        );
        self.terminate(Terminator::Jump(call_blk));

        self.switch_to(call_blk);
        let from = self
            .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
            .ok_or_else(|| "parallel for from returned no value".to_string())?;
        let until = self
            .emit(InstKind::LoadLocal(count_local), Some(Type::U64))
            .ok_or_else(|| "parallel for until returned no value".to_string())?;
        self.emit(
            InstKind::ParFor {
                body: func_id,
                env,
                from,
                until,
            },
            None,
        );

        self.scheduled.insert(func_id);
        self.pending_par_work.push(PendingParBody {
            func_id,
            var_name,
            var_ty,
            body: *body,
            captures,
        });
        Ok(None)
    }

    /// Read one captured scalar out of the enclosing scope.
    ///
    /// A `&mut x` parameter is a scalar behind a pointer; the body
    /// gets the value it holds now, which is all a read can want and
    /// all a write is refused for.
    fn load_capture_scalar(
        &mut self,
        name: DefaultSymbol,
        ty: Type,
    ) -> Result<ValueId, String> {
        match self.bindings.get(&name).cloned() {
            Some(Binding::Scalar { local, .. }) => self
                .emit(InstKind::LoadLocal(local), Some(ty))
                .ok_or_else(|| "capture LoadLocal returned no value".to_string()),
            Some(Binding::RefScalar { local, .. }) => {
                let ptr = self
                    .emit(InstKind::LoadLocal(local), Some(Type::U64))
                    .ok_or_else(|| "capture pointer load returned no value".to_string())?;
                self.emit(InstKind::LoadRef { ptr, ty }, Some(ty))
                    .ok_or_else(|| "capture LoadRef returned no value".to_string())
            }
            _ => Err(format!(
                "internal error (CONCURRENCY A2-b-2): `{}` is no longer a scalar binding",
                self.interner.resolve(name).unwrap_or("?")
            )),
        }
    }

    /// Everything the body reads from outside itself.
    ///
    /// The walk is [`FunctionLower::walk_closure_for_captures`], the
    /// one closures use, so "free name" means the same thing in both
    /// and the order is the order of first mention.
    fn collect_par_captures(
        &self,
        var_name: DefaultSymbol,
        body: &ExprRef,
    ) -> Result<Vec<ParCapture>, String> {
        use std::collections::HashSet;
        let mut bound: HashSet<DefaultSymbol> = HashSet::new();
        bound.insert(var_name);
        let mut found: Vec<(DefaultSymbol, Option<Type>)> = Vec::new();
        let mut seen: HashSet<DefaultSymbol> = HashSet::new();
        self.walk_closure_for_captures(body, &mut bound, &mut found, &mut seen);

        let mut captures = Vec::with_capacity(found.len());
        for (name, scalar_ty) in found {
            let spelled = self.interner.resolve(name).unwrap_or("?").to_string();
            if let Some(ty) = scalar_ty {
                if !fits_a_slot(ty) {
                    return Err(format!(
                        "compiler MVP: a `parallel for` body cannot capture `{spelled}` — its \
                         environment carries one 8-byte slot per value, and this one is wider"
                    ));
                }
                captures.push(ParCapture::Scalar { name, ty });
                continue;
            }
            match self.bindings.get(&name) {
                Some(Binding::Struct { struct_id, fields }) => {
                    let leaves = bindings::flatten_struct_locals(fields);
                    for (_, ty) in &leaves {
                        if !fits_a_slot(*ty) {
                            return Err(format!(
                                "compiler MVP: a `parallel for` body cannot capture \
                                 `{spelled}` — one of its fields is wider than the 8-byte \
                                 slot the environment gives it"
                            ));
                        }
                    }
                    captures.push(ParCapture::Struct {
                        name,
                        struct_id: *struct_id,
                        leaf_tys: leaves.into_iter().map(|(_, ty)| ty).collect(),
                    });
                }
                _ => {
                    // A tuple, an enum, an array, a range: shapes the
                    // env has no layout for. Naming the value beats
                    // failing later on an identifier the body plainly
                    // has in front of it.
                    return Err(format!(
                        "a `parallel for` body cannot capture `{spelled}`: its environment \
                         carries scalars and structs, and a window (`Span<T>` / `Column<T>`) \
                         is how a body reaches memory it should write"
                    ));
                }
            }
        }
        Ok(captures)
    }

    /// Lower one outlined body. Called from the work queue.
    pub(crate) fn lower_par_body(
        &mut self,
        work: &PendingParBody,
    ) -> Result<ParCheck, String> {
        // The three parameters, in signature order: codegen hands
        // block params to the first locals of the function.
        let env_local = self.module.function_mut(self.func_id).add_local(Type::U64);
        let from_local = self.module.function_mut(self.func_id).add_local(Type::U64);
        let until_local = self.module.function_mut(self.func_id).add_local(Type::U64);

        let entry = self.module.function_mut(self.func_id).add_block();
        self.module.function_mut(self.func_id).entry = entry;
        self.current_block = Some(entry);

        // Rebuild each capture from its env slots.
        let mut capture_locals: Vec<LocalId> = Vec::new();
        let mut slot = 0usize;
        for capture in &work.captures {
            match capture {
                ParCapture::Scalar { name, ty } => {
                    let local = self.read_env_slot(env_local, slot, *ty)?;
                    capture_locals.push(local);
                    self.bindings
                        .insert(*name, Binding::Scalar { local, ty: *ty });
                }
                ParCapture::Struct {
                    name,
                    struct_id,
                    leaf_tys,
                } => {
                    let fields = self.allocate_struct_fields(*struct_id);
                    let leaves = bindings::flatten_struct_locals(&fields);
                    if leaves.len() != leaf_tys.len() {
                        return Err(format!(
                            "internal error (CONCURRENCY A2-b-2): `{}` had {} leaves at the \
                             loop and {} in its body",
                            self.interner.resolve(*name).unwrap_or("?"),
                            leaf_tys.len(),
                            leaves.len()
                        ));
                    }
                    for (i, (local, ty)) in leaves.iter().enumerate() {
                        let v = self.load_env_slot(env_local, slot + i, *ty)?;
                        self.emit(InstKind::StoreLocal { dst: *local, src: v }, None);
                        capture_locals.push(*local);
                    }
                    self.bindings.insert(
                        *name,
                        Binding::Struct {
                            struct_id: *struct_id,
                            fields,
                        },
                    );
                }
            }
            slot += capture.slots();
        }
        // The loop's base sits in the slot after the captures.
        let base_local = self.read_env_slot(env_local, slot, work.var_ty)?;

        // Everything so far is the prologue, and it gets a block of
        // its own: the writes it makes to capture locals are the
        // copies arriving, not the body writing to them, and a whole
        // block is a boundary that survives later passes moving
        // instructions about.
        let setup = self.fresh_block();
        self.terminate(Terminator::Jump(setup));
        self.switch_to(setup);

        // `for k in from..until { var = base + k; body }`, written
        // out because the counter and the loop variable differ.
        let k_local = self.module.function_mut(self.func_id).add_local(Type::U64);
        let k0 = self
            .emit(InstKind::LoadLocal(from_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: from load returned no value".to_string())?;
        self.emit(
            InstKind::StoreLocal {
                dst: k_local,
                src: k0,
            },
            None,
        );
        let var_local = self
            .module
            .function_mut(self.func_id)
            .add_local(work.var_ty);
        self.bindings.insert(
            work.var_name,
            Binding::Scalar {
                local: var_local,
                ty: work.var_ty,
            },
        );

        let header = self.fresh_block();
        let body_blk = self.fresh_block();
        let step = self.fresh_block();
        let exit = self.fresh_block();
        self.terminate(Terminator::Jump(header));

        self.switch_to(header);
        let k = self
            .emit(InstKind::LoadLocal(k_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: k load returned no value".to_string())?;
        let until = self
            .emit(InstKind::LoadLocal(until_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: until load returned no value".to_string())?;
        let more = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Lt,
                    lhs: k,
                    rhs: until,
                },
                Some(Type::Bool),
            )
            .ok_or_else(|| "parallel body: header test returned no value".to_string())?;
        self.terminate(Terminator::Branch {
            cond: more,
            then_blk: body_blk,
            else_blk: exit,
        });

        self.switch_to(body_blk);
        // `var = base + k`, in the loop variable's own type.
        let k_now = self
            .emit(InstKind::LoadLocal(k_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: index load returned no value".to_string())?;
        let k_typed = if work.var_ty == Type::U64 {
            k_now
        } else {
            self.emit(
                InstKind::Cast {
                    value: k_now,
                    from: Type::U64,
                    to: work.var_ty,
                },
                Some(work.var_ty),
            )
            .ok_or_else(|| "parallel body: index cast returned no value".to_string())?
        };
        let base = self
            .emit(InstKind::LoadLocal(base_local), Some(work.var_ty))
            .ok_or_else(|| "parallel body: base load returned no value".to_string())?;
        let index = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Add,
                    lhs: base,
                    rhs: k_typed,
                },
                Some(work.var_ty),
            )
            .ok_or_else(|| "parallel body: index add returned no value".to_string())?;
        self.emit(
            InstKind::StoreLocal {
                dst: var_local,
                src: index,
            },
            None,
        );
        self.loop_stack
            .push((None, step, exit, self.with_scope_depth, self.drop_scopes.len()));
        let _ = self.lower_expr(&work.body)?;
        self.loop_stack.pop();
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(step));
        }

        self.switch_to(step);
        let cur = self
            .emit(InstKind::LoadLocal(k_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: step load returned no value".to_string())?;
        let one = self
            .emit(InstKind::Const(Const::U64(1)), Some(Type::U64))
            .ok_or_else(|| "parallel body: step const returned no value".to_string())?;
        let next = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Add,
                    lhs: cur,
                    rhs: one,
                },
                Some(Type::U64),
            )
            .ok_or_else(|| "parallel body: step add returned no value".to_string())?;
        self.emit(
            InstKind::StoreLocal {
                dst: k_local,
                src: next,
            },
            None,
        );
        self.terminate(Terminator::Jump(header));

        self.switch_to(exit);
        self.terminate(Terminator::Return(Vec::new()));

        let mut owners: Vec<(LocalId, DefaultSymbol)> = Vec::new();
        let mut at = 0usize;
        for capture in &work.captures {
            for local in capture_locals.iter().skip(at).take(capture.slots()) {
                owners.push((*local, capture.name()));
            }
            at += capture.slots();
        }
        Ok(ParCheck {
            func_id: self.func_id,
            entry,
            capture_locals: owners,
        })
    }

    /// One env slot into a fresh local, and the local back.
    fn read_env_slot(
        &mut self,
        env_local: LocalId,
        slot: usize,
        ty: Type,
    ) -> Result<LocalId, String> {
        let v = self.load_env_slot(env_local, slot, ty)?;
        let local = self.module.function_mut(self.func_id).add_local(ty);
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        Ok(local)
    }

    /// One env slot as a value. One 8-byte slot per capture leaf, in
    /// the order the loop wrote them, then the loop's base.
    fn load_env_slot(
        &mut self,
        env_local: LocalId,
        slot: usize,
        ty: Type,
    ) -> Result<ValueId, String> {
        let env = self
            .emit(InstKind::LoadLocal(env_local), Some(Type::U64))
            .ok_or_else(|| "parallel body: env load returned no value".to_string())?;
        let offset = self
            .emit(
                InstKind::Const(Const::U64((slot * 8) as u64)),
                Some(Type::U64),
            )
            .ok_or_else(|| "parallel body: env offset returned no value".to_string())?;
        self.emit(
            InstKind::PtrRead {
                ptr: env,
                offset,
                elem_ty: ty,
            },
            Some(ty),
        )
        .ok_or_else(|| "parallel body: env read returned no value".to_string())
    }

}

/// Refuse a body that writes to what it captured.
///
/// The copy in the environment is this chunk's alone, so a write to
/// it is lost when the loop ends — and if it weren't lost, the chunks
/// would be racing for it. Either way the parallel lane would stop
/// agreeing with the sequential one, which is the one thing A1 fixed
/// in advance.
///
/// Asking the IR rather than the source is what makes this catch the
/// spellings at once: `x = ..`, `s.field = ..`, and a method whose
/// `&mut self` writeback lands back in the leaves. Writing *through*
/// a captured window is not a write to the capture — `set` stores to
/// the address the copy holds, and the copy is untouched.
pub(crate) fn reject_capture_writes(
    module: &crate::ir::Module,
    interner: &string_interner::DefaultStringInterner,
    checks: &[ParCheck],
) -> Result<(), String> {
    use std::collections::HashMap;
    for check in checks {
        let owner: HashMap<LocalId, DefaultSymbol> =
            check.capture_locals.iter().copied().collect();
        let func = module.function(check.func_id);
        let mut written: Option<DefaultSymbol> = None;
        for (bi, block) in func.blocks.iter().enumerate() {
            if bi == check.entry.0 as usize {
                continue;
            }
            for inst in &block.instructions {
                inst.kind.writes_locals(|local| {
                    if written.is_none()
                        && let Some(name) = owner.get(&local)
                    {
                        written = Some(*name);
                    }
                });
            }
        }
        if let Some(name) = written {
            return Err(format!(
                "a `parallel for` body writes to `{}`, which it captured from around the \
                 loop. Each chunk runs on its own copy, so the write would be lost here and \
                 would be a race if it weren't: move it after the loop, or write through a \
                 window (`Span<T>` / `Column<T>`), whose `set` reaches the one buffer.",
                interner.resolve(name).unwrap_or("?")
            ));
        }
    }
    Ok(())
}

/// Whether one 8-byte env slot can carry a value of this type.
fn fits_a_slot(ty: Type) -> bool {
    ty.is_scalar()
}
