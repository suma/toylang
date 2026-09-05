//! CODE-SIZE-WB-PRUNE — drop writeback return slots the body never writes.
//!
//! A `&mut self` method (and any function with a `&mut <compound>`
//! parameter) returns **every leaf** of that receiver alongside its
//! own return value, so the caller can store the mutated values back
//! (`design-docs/BACKEND.md`, `CallWithSelfWriteback`). The leaf count
//! is a property of the *type*, not of what the method touches, so a
//! method that writes two fields of a 52-leaf struct still pays for
//! all 52 — at **every** `return` site, because each one materialises
//! the whole tuple.
//!
//! Measured on `poc/logsearch`: a 52-leaf `&mut self` method costs
//! ~95 instructions per extra `return`, and `ArchiveWriter::write_seg`
//! has nine of them. See [`design-docs/CODE_SIZE.md`].
//!
//! This pass removes the slots that cannot carry new information:
//!
//! > if the body never writes leaf *k*, the caller already holds that
//! > value, so returning it is a copy of what it passed in.
//!
//! ## Why this is safe
//!
//! "Never writes" is decided by [`InstKind::writes_locals`], whose
//! match is exhaustive — a future instruction that writes a local has
//! to be classified there or the build breaks. On top of that a leaf
//! is kept whenever:
//!
//! - it is in `address_taken_locals` (something may store through the
//!   address), or
//! - the same local appears more than once in the writeback list (a
//!   shape we do not try to reason about), or
//! - the function's `self_writeback_locals` was never recorded, i.e.
//!   the shape came from the declaration-time pre-populate and no body
//!   was lowered (`extern`, a trait signature). Those keep every slot.
//!
//! One more gate, and it is the subtle one. Dropping slot *k* leaves
//! the caller's dest local for *k* undefined unless something else
//! already gave it a value. That holds for an ordinary method call,
//! where the dests are the receiver binding's own leaf locals (they
//! held the values that were just passed in), but **not** for a `dyn`
//! thunk, which allocates fresh locals purely to catch the writeback
//! and then stores them through `data_ptr`. So a callee is pruned only
//! when every dropped dest, at every call site, is a parameter leaf or
//! is written somewhere else in that caller — otherwise the callee
//! keeps its full shape.
//!
//! ## Why it runs after every body
//!
//! Call sites are emitted against the callee's declared shape, which
//! may be the forward-reference pre-populate. Rewriting both ends in
//! one post-pass is what keeps them in agreement; doing it during
//! lowering would need the callee's body before its callers.

use std::collections::{HashMap, HashSet};

use compiler_ir::{FuncId, InstKind, LocalId, Module, Terminator};

/// Run the pass over a fully lowered module. Returns the number of
/// writeback slots removed (for tests and `-v` reporting).
///
/// Iterated to a fixpoint: a `&mut self` method that calls another one
/// looks like it writes every leaf, because the call's `self_dests`
/// covers them all. Once the callee is narrowed the caller's dests
/// shrink with it, and the caller becomes prunable in turn. In
/// `poc/logsearch` this is the difference between narrowing
/// `write_frames` alone and narrowing the chain above it
/// (`write_seg` -> `flush_segment` -> `cmd_archive`).
pub fn prune_unwritten_writeback(module: &mut Module) -> usize {
    let mut total = 0usize;
    // The bound is a guard against a bug, not a real limit: each round
    // strictly removes at least one slot, so it terminates on its own.
    for _ in 0..32 {
        let n = prune_once(module);
        if n == 0 {
            break;
        }
        total += n;
    }
    total
}

fn prune_once(module: &mut Module) -> usize {
    // 1. Decide, per function, which of its writeback slots survive.
    //    `kept[f]` is the list of surviving indices into the old
    //    `self_writeback_types`; a function absent from the map keeps
    //    everything.
    let mut kept: HashMap<FuncId, (usize, Vec<usize>)> = HashMap::new();
    let mut removed = 0usize;

    for (idx, func) in module.functions.iter().enumerate() {
        let n = func.self_writeback_types.len();
        if n == 0 {
            continue;
        }
        // No recorded leaf locals -> we cannot prove anything.
        if func.self_writeback_locals.len() != n {
            continue;
        }

        let mut written: HashSet<LocalId> = func.address_taken_locals.iter().copied().collect();
        for blk in &func.blocks {
            for inst in &blk.instructions {
                inst.kind.writes_locals(|l| {
                    written.insert(l);
                });
            }
        }

        // A local that fills two slots is not something this pass
        // reasons about; keep every slot in that case.
        let mut seen: HashSet<LocalId> = HashSet::new();
        let duplicated = !func.self_writeback_locals.iter().all(|l| seen.insert(*l));

        let keep: Vec<usize> = (0..n)
            .filter(|i| duplicated || written.contains(&func.self_writeback_locals[*i]))
            .collect();
        if keep.len() < n {
            removed += n - keep.len();
            kept.insert(FuncId(idx as u32), (n, keep));
        }
    }

    if kept.is_empty() {
        return 0;
    }

    // 1a. A pruned function changes shape, so anything that can reach
    //     it other than by a direct call has to hold it back: a raw
    //     address, a closure, a vtable slot. Those call through a
    //     signature we do not rewrite here.
    {
        let mut escaped: HashSet<FuncId> = HashSet::new();
        for slots in module.vtables.values() {
            escaped.extend(slots.iter().copied());
        }
        for func in &module.functions {
            for blk in &func.blocks {
                for inst in &blk.instructions {
                    match &inst.kind {
                        InstKind::FuncAddr { target } | InstKind::MakeClosure { target, .. } => {
                            escaped.insert(*target);
                        }
                        _ => {}
                    }
                }
            }
        }
        kept.retain(|f, (n, keep)| {
            if escaped.contains(f) {
                removed -= *n - keep.len();
                false
            } else {
                true
            }
        });
    }
    if kept.is_empty() {
        return 0;
    }

    // 1b. Veto any callee whose pruning would leave a caller holding
    //     an undefined dest local (see the module docs: `dyn` thunks).
    let mut vetoed: HashSet<FuncId> = HashSet::new();
    for func in &module.functions {
        // Locals `0..param_leaves` are the flattened parameters, and
        // the entry block defines them all.
        let mut param_leaves = 0usize;
        for p in &func.params {
            let mut tys = Vec::new();
            compiler_ir::layout::flatten_compound_leaf_types(module, *p, &mut tys);
            param_leaves += tys.len().max(1);
        }
        let mut written: HashSet<LocalId> = func.address_taken_locals.iter().copied().collect();
        for blk in &func.blocks {
            for inst in &blk.instructions {
                inst.kind.writes_locals(|l| {
                    written.insert(l);
                });
            }
        }
        for blk in &func.blocks {
            for inst in &blk.instructions {
                let Some((target, dests)) = call_writeback_dests(&inst.kind) else { continue };
                let Some((old_len, keep)) = kept.get(&target) else { continue };
                // For the compound-return calls the writeback leaves
                // are the tail of `dests`; for the dedicated variants
                // they are all of it.
                if dests.len() < *old_len {
                    continue;
                }
                let tail = &dests[dests.len() - *old_len..];
                let keep_set: HashSet<usize> = keep.iter().copied().collect();
                for (i, d) in tail.iter().enumerate() {
                    if keep_set.contains(&i) {
                        continue;
                    }
                    // Written only by this very call? Then dropping
                    // the slot strands it.
                    let defined_elsewhere = (d.0 as usize) < param_leaves
                        || instruction_count_writing(func, *d) > 1;
                    if !defined_elsewhere {
                        vetoed.insert(target);
                        break;
                    }
                }
            }
        }
    }
    for f in &vetoed {
        if let Some((n, keep)) = kept.remove(f) {
            removed -= n - keep.len();
        }
    }
    if kept.is_empty() {
        return 0;
    }

    // 2. Narrow each pruned function's own signature and returns. The
    //    writeback values are the *trailing* operands of every
    //    `Return`, so the user-visible part is whatever comes before
    //    them and is left alone.
    for (func_id, (old_len, keep)) in &kept {
        let old_len = *old_len;
        let func = module.function_mut(*func_id);
        func.self_writeback_types = keep.iter().map(|i| func.self_writeback_types[*i]).collect();
        func.self_writeback_locals =
            keep.iter().map(|i| func.self_writeback_locals[*i]).collect();
        for blk in &mut func.blocks {
            if let Some(Terminator::Return(values)) = blk.terminator.as_mut() {
                // A `Return` shorter than the writeback tail is a
                // diverging path the writeback never reached; leave it.
                if values.len() < old_len {
                    continue;
                }
                let head = values.len() - old_len;
                let tail: Vec<_> = keep.iter().map(|i| values[head + *i]).collect();
                values.truncate(head);
                values.extend(tail);
            }
        }
    }

    // 3. Narrow every call site aimed at a pruned function. `self_dests`
    //    is in the same order as the callee's `self_writeback_types`,
    //    so the surviving indices select from it directly.
    for func in &mut module.functions {
        for blk in &mut func.blocks {
            for inst in &mut blk.instructions {
                let Some((target, dests)) = call_writeback_dests_mut(&mut inst.kind) else {
                    continue;
                };
                let Some((old_len, keep)) = kept.get(&target) else { continue };
                if dests.len() < *old_len {
                    continue;
                }
                let head = dests.len() - *old_len;
                let tail: Vec<LocalId> = keep.iter().map(|i| dests[head + *i]).collect();
                dests.truncate(head);
                dests.extend(tail);
            }
        }
    }

    removed
}

/// How many instructions in `func` write `local`. Used to tell "this
/// dest is only ever defined by the call we are about to narrow" from
/// "the caller already had a value here".
fn instruction_count_writing(func: &compiler_ir::Function, local: LocalId) -> usize {
    let mut n = 0usize;
    for blk in &func.blocks {
        for inst in &blk.instructions {
            inst.kind.writes_locals(|l| {
                if l == local {
                    n += 1;
                }
            });
        }
    }
    n
}

/// The call variants whose dest list ends in the callee's writeback
/// leaves. `CallStruct` / `CallTuple` / `CallEnum` concatenate the
/// return leaves and the writeback leaves into one `dests`, so the
/// writeback part is identified by length, not by position.
fn call_writeback_dests(kind: &InstKind) -> Option<(FuncId, &Vec<LocalId>)> {
    match kind {
        InstKind::CallWithSelfWriteback { target, self_dests, .. }
        | InstKind::CallWithSelfWritebackCompound { target, self_dests, .. } => {
            Some((*target, self_dests))
        }
        InstKind::CallStruct { target, dests, .. }
        | InstKind::CallTuple { target, dests, .. }
        | InstKind::CallEnum { target, dests, .. } => Some((*target, dests)),
        _ => None,
    }
}

fn call_writeback_dests_mut(kind: &mut InstKind) -> Option<(FuncId, &mut Vec<LocalId>)> {
    match kind {
        InstKind::CallWithSelfWriteback { target, self_dests, .. }
        | InstKind::CallWithSelfWritebackCompound { target, self_dests, .. } => {
            Some((*target, self_dests))
        }
        InstKind::CallStruct { target, dests, .. }
        | InstKind::CallTuple { target, dests, .. }
        | InstKind::CallEnum { target, dests, .. } => Some((*target, dests)),
        _ => None,
    }
}
