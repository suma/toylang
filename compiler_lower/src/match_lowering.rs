//! `match` expression lowering.
//!
//! Lowers a `match` over either an enum scrutinee or a scalar
//! scrutinee (`i64` / `u64` / `bool` / `str`) into a chain of
//! tag / equality branches that converge on a result local.
//!
//! - `lower_match`: top-level entry. Classifies the scrutinee
//!   via `classify_match_scrutinee`, allocates a result local
//!   from the unified arm body type, and dispatches each arm
//!   through `dispatch_enum_variant_pattern` (enum) or
//!   `emit_literal_eq_branch` (scalar).
//! - `arm_body_type`: peek-only scan through the arms to find
//!   the first concrete body type (used to allocate the result
//!   local up front).
//! - `dispatch_enum_variant_pattern`: emits the per-variant
//!   tag / payload sub-pattern dispatch chain. Handles `Name`
//!   binding (re-bind the storage), `_` discards, and literal
//!   payload sub-patterns (deferred-error on nested enum /
//!   tuple sub-patterns since the MVP doesn't allow them).
//! - `apply_arm_pattern_bindings_for_inference`: temporarily
//!   binds pattern names so `arm_body_type` can peek through
//!   them when the arm body references payload locals.
//! - `classify_match_scrutinee`: decides between the enum and
//!   scalar code paths based on the scrutinee's resolved type.
//! - `emit_literal_eq_branch`: scalar-arm helper that emits one
//!   `cond = scrutinee == literal; brif cond, body, next` step.

use frontend::ast::{Expr, ExprRef, MatchArm, Pattern};
use string_interner::DefaultSymbol;

use super::bindings::{
    flatten_enum_storage_locals, flatten_struct_locals, Binding, EnumStorage, FieldBinding,
    FieldShape, MatchScrutinee, PayloadSlot, TupleElementBinding, TupleElementShape,
};
use super::{DropTarget, FunctionLower};
use crate::ir::{BinOp, BlockId, Const, InstKind, Terminator, Type, ValueId};

impl<'a> FunctionLower<'a> {
    /// Lower `match scrutinee { arm, ... }`. Compiler MVP scope:
    /// - Scrutinee resolves to either an `Enum` binding or a scalar
    ///   value (any expression that produces `i64` / `u64` / `bool`).
    /// - Top-level patterns: `Wildcard`, `EnumVariant(...)` (only
    ///   against an enum scrutinee), `Literal(_)` (only against a
    ///   scalar scrutinee).
    /// - Variant sub-patterns: `Name(sym)` binds the payload, `_`
    ///   discards, `Literal(_)` adds an equality check on the
    ///   payload slot. Nested enum / tuple sub-patterns are deferred
    ///   (no enum-of-enum payloads in this MVP anyway).
    /// - Optional `if` guard runs after the pattern matches and any
    ///   `Name` sub-patterns are in scope.
    /// - Arms must agree on result type (same as `if` chain).
    pub(super) fn lower_match(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
    ) -> Result<Option<ValueId>, String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        // Pick the result type by scanning every arm body for the
        // first non-divergent scalar — same trick as `lower_if_chain`,
        // but with arm-pattern-aware inference so a body that's just
        // a `Name` sub-pattern (e.g. `Pick::A(n) => n`) still resolves
        // to the payload's declared type. Without this, the simplest
        // "extract the payload" matches would degrade to `Unit` and
        // silently produce no value.
        let mut result_ty = Type::Unit;
        for arm in arms.iter() {
            if let Some(ty) = self.arm_body_type(&scrut, arm) {
                result_ty = ty;
                break;
            }
        }
        let result_local = if result_ty.produces_value() {
            Some(self.module.function_mut(self.func_id).add_local(result_ty))
        } else {
            None
        };
        let merge = self.fresh_block();
        for arm in arms.iter() {
            // Snapshot the binding map so a `Name` sub-pattern
            // introduced by this arm doesn't leak into a subsequent
            // arm's lowering scope. Restoring is purely a lowering-side
            // concern: cranelift `def_var`s only happen in the body
            // block, which is reached only when the pattern actually
            // matched.
            let saved_bindings = self.bindings.clone();
            // DROP-GLUE: each arm's pattern bindings collect here and
            // are emitted at arm exit. They must NOT go through
            // `drop_scopes` — a scope's drops are emitted on the
            // enclosing block's linear exit, which every arm shares,
            // so a different arm's path could fire them with
            // uninitialized locals.
            self.arm_drop_targets.clear();
            let next_blk = self.fresh_block();
            // 1. Pattern shape check + sub-pattern equality checks.
            //    On any failure, jump to next_blk. On full success,
            //    advance the current block to where bindings happen.
            self.dispatch_arm_pattern(&arm.pattern, &scrut, next_blk)?;
            // 2. Optional guard: evaluated with the arm's bindings in
            //    scope. False routes to the next arm; true falls into
            //    the body block.
            if let Some(guard_ref) = &arm.guard {
                let body_blk = self.fresh_block();
                let gv = self
                    .lower_expr(guard_ref)?
                    .ok_or_else(|| "match guard produced no value".to_string())?;
                self.terminate(Terminator::Branch {
                    cond: gv,
                    then_blk: body_blk,
                    else_blk: next_blk,
                });
                self.switch_to(body_blk);
            }
            // 3. Body. Lower in the current block (no extra branch
            //    needed when there's no guard — bindings live in the
            //    current block already).
            let body_v = self.lower_expr(&arm.body)?;
            if !self.is_unreachable() {
                // The arm's pattern bindings die here, at arm exit,
                // before the merge jump.
                let arm_drops = std::mem::take(&mut self.arm_drop_targets);
                for target in arm_drops.into_iter().rev() {
                    self.emit_drop_call(&target)?;
                }
                if let (Some(local), Some(v)) = (result_local, body_v) {
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                self.terminate(Terminator::Jump(merge));
            } else {
                // The body diverged; the arm's drops never run.
                self.arm_drop_targets.clear();
            }
            // 4. Roll back bindings and continue with the next arm.
            self.bindings = saved_bindings;
            self.switch_to(next_blk);
        }
        // After the last arm we are sitting in the trailing fallthrough
        // block. The type-checker has already verified exhaustiveness
        // (wildcard or variant set), so this block is unreachable in
        // well-typed programs — terminate it with a panic so cranelift
        // sees a real terminator and the runtime gets a clear message
        // if exhaustiveness ever drifts.
        if !self.is_unreachable() {
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
            });
        }
        self.switch_to(merge);
        if let Some(local) = result_local {
            Ok(self.emit(InstKind::LoadLocal(local), Some(result_ty)))
        } else {
            Ok(None)
        }
    }

    /// PATTERN-COMPOUND-LOWER: the shape check and bindings for one
    /// arm's pattern. Emitted code branches to `fail_blk` on the first
    /// check that cannot match; on success the current block is where
    /// the arm's guard and body go.
    ///
    /// Shared by `lower_match` and `lower_match_into_enum`, which used
    /// to mirror a subset of this by hand.
    pub(super) fn dispatch_arm_pattern(
        &mut self,
        pattern: &Pattern,
        scrut: &MatchScrutinee,
        fail_blk: BlockId,
    ) -> Result<(), String> {
        match pattern {
            Pattern::Wildcard => {
                // No checks; current block keeps going.
            }
            // PATTERN-EXTEND: `n @ pat` — name the whole scrutinee,
            // then let `pat` decide. Binding before the check is safe:
            // a failed check branches to `fail_blk`, and the caller
            // restores the binding map before the next arm.
            Pattern::Binding(sym, inner) => {
                self.bind_whole_scrutinee(*sym, scrut);
                self.dispatch_arm_pattern(inner, scrut, fail_blk)?;
            }
            Pattern::Literal(lit_ref) => {
                let (scrut_v, scrut_ty) = match scrut {
                    MatchScrutinee::Scalar { value, ty } => (*value, *ty),
                    MatchScrutinee::Enum { .. }
                    | MatchScrutinee::Struct { .. }
                    | MatchScrutinee::Tuple { .. } => {
                        return Err(
                            "literal pattern is only valid against a scalar scrutinee"
                                .to_string(),
                        );
                    }
                };
                self.emit_literal_eq_branch(lit_ref, scrut_v, scrut_ty, fail_blk)?;
            }
            Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                let scrut_storage = match scrut {
                    MatchScrutinee::Enum(s) => s.clone(),
                    MatchScrutinee::Scalar { .. }
                    | MatchScrutinee::Struct { .. }
                    | MatchScrutinee::Tuple { .. } => {
                        return Err(
                            "enum-variant pattern is only valid against an enum scrutinee"
                                .to_string(),
                        );
                    }
                };
                self.dispatch_enum_variant_pattern(
                    &scrut_storage,
                    *p_enum,
                    *p_variant,
                    sub_patterns,
                    fail_blk,
                )?;
            }
            Pattern::Name(sym) => {
                self.bind_whole_scrutinee(*sym, scrut);
            }
            // PATTERN-COMPOUND-LOWER: `Point { x: 0i64, y }` and
            // `(0i64, y)`. Each field / element pattern either
            // compares against the scrutinee's own local or binds
            // a name to it, so the arm reads the value in place.
            Pattern::Struct(_, field_patterns, _) => {
                let fields = match scrut {
                    MatchScrutinee::Struct { fields, .. } => fields.clone(),
                    _ => {
                        return Err(
                            "struct pattern is only valid against a struct scrutinee"
                                .to_string(),
                        );
                    }
                };
                self.dispatch_struct_pattern(&fields, field_patterns, fail_blk)?;
            }
            Pattern::Tuple(sub_patterns) => {
                let elements = match scrut {
                    MatchScrutinee::Tuple { elements } => elements.clone(),
                    _ => {
                        return Err(
                            "tuple pattern is only valid against a tuple scrutinee".to_string(),
                        );
                    }
                };
                self.dispatch_tuple_pattern(&elements, sub_patterns, fail_blk)?;
            }
        }
        Ok(())
    }

    /// Bind a name to the entire scrutinee — what a top-level `Name`
    /// pattern does, and the half of `n @ pat` that is not a check.
    ///
    /// Enum scrutinees bind through a copied storage, the same
    /// treatment a payload `Name` sub-pattern gets, so the arm body
    /// can outlive the match without aliasing it. A compound binding
    /// aliases the scrutinee's locals instead: the scrutinee is a
    /// binding of the enclosing scope and outlives the arm.
    fn bind_whole_scrutinee(&mut self, sym: DefaultSymbol, scrut: &MatchScrutinee) {
        match scrut {
            MatchScrutinee::Scalar { value, ty } => {
                let local = self.module.function_mut(self.func_id).add_local(*ty);
                self.emit(InstKind::StoreLocal { dst: local, src: *value }, None);
                self.bindings
                    .insert(sym, Binding::Scalar { local, ty: *ty });
            }
            MatchScrutinee::Enum(storage) => {
                let copy = self.allocate_enum_storage(storage.enum_id);
                self.copy_enum_storage(storage, &copy);
                self.bindings.insert(sym, Binding::Enum(copy.clone()));
                if self.ir_contains_drop(crate::ir::Type::Enum(copy.enum_id)) {
                    let leaves = flatten_enum_storage_locals(&copy);
                    self.arm_drop_targets.push(DropTarget {
                        ty: crate::ir::Type::Enum(copy.enum_id),
                        field_locals: leaves,
                    });
                }
            }
            MatchScrutinee::Struct { struct_id, fields } => {
                self.bindings.insert(
                    sym,
                    Binding::Struct { struct_id: *struct_id, fields: fields.clone() },
                );
            }
            MatchScrutinee::Tuple { elements } => {
                self.bindings
                    .insert(sym, Binding::Tuple { elements: elements.clone() });
            }
        }
    }

    /// Best-effort body-type inference for one match arm, with
    /// pattern-introduced bindings temporarily applied so
    /// `value_scalar` can resolve identifier references that the
    /// arm's `Name` sub-patterns would bring into scope. Restores
    /// the binding map before returning.
    pub(super) fn arm_body_type(
        &mut self,
        scrut: &MatchScrutinee,
        arm: &MatchArm,
    ) -> Option<Type> {
        let saved = self.bindings.clone();
        self.apply_arm_pattern_bindings_for_inference(scrut, &arm.pattern);
        let ty = self.value_scalar(&arm.body);
        self.bindings = saved;
        ty
    }

    /// Insert dummy `Scalar` bindings into `self.bindings` for every
    /// `Name` sub-pattern an arm pattern would introduce, using the
    /// scrutinee's payload local table as the source of truth for
    /// type / local. Used only by `arm_body_type` — the caller is
    /// expected to snapshot and restore.
    /// Lower the pattern dispatch for one `EnumVariant` arm: tag
    /// equality check, optional literal sub-pattern checks, and
    /// payload bindings (Name and nested EnumVariant). Mismatch on
    /// any check branches to `next_blk`. After this returns, the
    /// current block is the block where the arm body should be
    /// lowered (with all `Name` bindings introduced into
    /// `self.bindings`). For the recursive case (nested
    /// `EnumVariant` sub-pattern), the inner call further branches
    /// on the inner storage's tag.
    pub(super) fn dispatch_enum_variant_pattern(
        &mut self,
        scrut_storage: &EnumStorage,
        p_enum: DefaultSymbol,
        p_variant: DefaultSymbol,
        sub_patterns: &Vec<Pattern>,
        next_blk: BlockId,
    ) -> Result<(), String> {
        let scrut_def = self.module.enum_def(scrut_storage.enum_id).clone();
        if p_enum != scrut_def.base_name {
            return Err(format!(
                "match arm pattern enum `{}` does not match scrutinee enum `{}`",
                self.interner.resolve(p_enum).unwrap_or("?"),
                self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
            ));
        }
        let variant_idx = scrut_def
            .variants
            .iter()
            .position(|v| v.name == p_variant)
            .ok_or_else(|| {
                format!(
                    "match arm references unknown variant `{}::{}`",
                    self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
                    self.interner.resolve(p_variant).unwrap_or("?"),
                )
            })?;
        if sub_patterns.len() != scrut_def.variants[variant_idx].payload_types.len() {
            return Err(format!(
                "match arm for `{}::{}` has {} sub-pattern(s), expected {}",
                self.interner.resolve(scrut_def.base_name).unwrap_or("?"),
                self.interner.resolve(p_variant).unwrap_or("?"),
                sub_patterns.len(),
                scrut_def.variants[variant_idx].payload_types.len(),
            ));
        }
        // Tag dispatch.
        let tag_v = self
            .emit(InstKind::LoadLocal(scrut_storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let want = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        let tag_eq = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Eq,
                    lhs: tag_v,
                    rhs: want,
                },
                Some(Type::Bool),
            )
            .expect("Eq returns a value");
        let after_tag = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond: tag_eq,
            then_blk: after_tag,
            else_blk: next_blk,
        });
        self.switch_to(after_tag);
        // Sub-pattern checks (literal equality + nested EnumVariant
        // tag checks). Done before bindings so a failed check
        // doesn't leave stray bindings in scope.
        for (i, sp) in sub_patterns.iter().enumerate() {
            let slot = scrut_storage.payloads[variant_idx][i].clone();
            self.check_payload_sub_pattern(sp, slot, next_blk)?;
        }
        // Sub-pattern bindings.
        for (i, sp) in sub_patterns.iter().enumerate() {
            let slot = scrut_storage.payloads[variant_idx][i].clone();
            self.bind_payload_sub_pattern(sp, slot)?;
        }
        Ok(())
    }

    /// The binding half of one enum payload sub-pattern. A `Name`
    /// takes ownership of a fresh copy of the payload — the scrutinee
    /// itself may never be dropped (a function parameter, say), so the
    /// arm's binding is what the drop glue can reach.
    fn bind_payload_sub_pattern(
        &mut self,
        sp: &Pattern,
        slot: PayloadSlot,
    ) -> Result<(), String> {
        match sp {
            Pattern::Name(sym) => match slot {
                PayloadSlot::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(local), Some(ty))
                        .expect("LoadLocal returns a value");
                    let dst = self
                        .module
                        .function_mut(self.func_id)
                        .add_local(ty);
                    self.emit(InstKind::StoreLocal { dst, src: v }, None);
                    self.bindings
                        .insert(*sym, Binding::Scalar { local: dst, ty });
                }
                PayloadSlot::Enum(inner_storage) => {
                    // Bind the name to a fresh EnumStorage that's
                    // a deep copy of the matched payload.
                    let inner = (*inner_storage).clone();
                    let copy = self.allocate_enum_storage(inner.enum_id);
                    self.copy_enum_storage(&inner, &copy);
                    self.bindings.insert(*sym, Binding::Enum(copy.clone()));
                    // DROP-GLUE: the payload binding owns what it
                    // names (the scrutinee may never be dropped —
                    // e.g. a function parameter), so it must free
                    // it at the arm's scope exit. Collected into
                    // `arm_drop_targets` so the drop fires on this
                    // arm's path only. A transfer can never
                    // suppress this (no statement is in flight
                    // during match lowering) — an over-
                    // approximation that is safe because `free` is
                    // idempotent everywhere.
                    if self.ir_contains_drop(crate::ir::Type::Enum(copy.enum_id)) {
                        let leaves = flatten_enum_storage_locals(&copy);
                        self.arm_drop_targets.push(DropTarget {
                            ty: crate::ir::Type::Enum(copy.enum_id),
                            field_locals: leaves,
                        });
                    }
                }
                PayloadSlot::Struct {
                    struct_id,
                    fields: src_fields,
                } => {
                    // Same idea for a struct payload: allocate a
                    // fresh struct binding and deep-copy each
                    // field's leaf locals across.
                    let dst_fields = self.allocate_struct_fields(struct_id);
                    self.copy_struct_fields(&src_fields, &dst_fields);
                    self.bindings.insert(
                        *sym,
                        Binding::Struct {
                            struct_id,
                            fields: dst_fields.clone(),
                        },
                    );
                    if self.ir_contains_drop(crate::ir::Type::Struct(struct_id)) {
                        let leaves = flatten_struct_locals(&dst_fields);
                        self.arm_drop_targets.push(DropTarget {
                            ty: crate::ir::Type::Struct(struct_id),
                            field_locals: leaves,
                        });
                    }
                }
                PayloadSlot::Tuple {
                    elements: src_elements,
                    ..
                } => {
                    // Same shape for tuple payloads: fresh per-
                    // element locals + element-wise copy. The new
                    // binding is reachable as a regular tuple
                    // binding, supporting `t.0` access in arm
                    // bodies.
                    let mut dst_elements: Vec<TupleElementBinding> =
                        Vec::with_capacity(src_elements.len());
                    for el in &src_elements {
                        let shape = match &el.shape {
                            TupleElementShape::Scalar { ty, .. } => {
                                let local = self
                                    .module
                                    .function_mut(self.func_id)
                                    .add_local(*ty);
                                TupleElementShape::Scalar { local, ty: *ty }
                            }
                            TupleElementShape::Struct { struct_id, .. } => {
                                let fields =
                                    self.allocate_struct_fields(*struct_id);
                                TupleElementShape::Struct {
                                    struct_id: *struct_id,
                                    fields,
                                }
                            }
                            TupleElementShape::Tuple { tuple_id, .. } => {
                                let elements = self
                                    .allocate_tuple_elements(*tuple_id)
                                    .unwrap_or_default();
                                TupleElementShape::Tuple {
                                    tuple_id: *tuple_id,
                                    elements,
                                }
                            }
                        };
                        dst_elements.push(TupleElementBinding {
                            index: el.index,
                            shape,
                        });
                    }
                    self.copy_tuple_elements(&src_elements, &dst_elements);
                    self.bindings.insert(
                        *sym,
                        Binding::Tuple { elements: dst_elements },
                    );
                }
            },
            // PATTERN-EXTEND: `n @ pat` names the payload and then
            // hands `pat` whatever it binds in turn.
            Pattern::Binding(sym, inner) => {
                self.bind_payload_sub_pattern(&Pattern::Name(*sym), slot.clone())?;
                self.bind_payload_sub_pattern(inner, slot)?;
            }
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::EnumVariant(..) => {
                // Wildcard discards; literals were checked
                // above; nested EnumVariant patterns introduced
                // their own bindings via the recursive call.
            }
            other => {
                return Err(format!(
                    "compiler MVP only supports `Name`, `_`, literal, and \
                     nested `EnumVariant` sub-patterns inside enum variants, got {other:?}"
                ));
            }
        }
        Ok(())
    }

    /// The check half of one enum payload sub-pattern: literal
    /// equality and nested variant tags, branching to `fail_blk` on a
    /// mismatch. Bindings are a separate pass, so a failed check never
    /// leaves a stray name in scope.
    fn check_payload_sub_pattern(
        &mut self,
        sp: &Pattern,
        slot: PayloadSlot,
        fail_blk: BlockId,
    ) -> Result<(), String> {
        match sp {
            Pattern::Literal(lit_ref) => match slot {
                PayloadSlot::Scalar { local, ty } => {
                    let pv = self
                        .emit(InstKind::LoadLocal(local), Some(ty))
                        .expect("LoadLocal returns a value");
                    self.emit_literal_eq_branch(lit_ref, pv, ty, fail_blk)?;
                }
                PayloadSlot::Enum(_)
                | PayloadSlot::Struct { .. }
                | PayloadSlot::Tuple { .. } => {
                    return Err(
                        "literal sub-pattern is only valid against a scalar payload".to_string(),
                    );
                }
            },
            Pattern::EnumVariant(inner_enum, inner_variant, inner_subs) => match slot {
                PayloadSlot::Enum(inner_storage) => {
                    self.dispatch_enum_variant_pattern(
                        &inner_storage,
                        *inner_enum,
                        *inner_variant,
                        inner_subs,
                        fail_blk,
                    )?;
                }
                PayloadSlot::Scalar { .. }
                | PayloadSlot::Struct { .. }
                | PayloadSlot::Tuple { .. } => {
                    return Err(
                        "nested enum-variant sub-pattern requires an enum-typed payload"
                            .to_string(),
                    );
                }
            },
            // PATTERN-EXTEND: `n @ pat` at a payload position — the
            // check is `pat`'s; the name is bound in the second pass.
            Pattern::Binding(_, inner) => {
                self.check_payload_sub_pattern(inner, slot, fail_blk)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn apply_arm_pattern_bindings_for_inference(
        &mut self,
        scrut: &MatchScrutinee,
        pattern: &Pattern,
    ) {
        // PATTERN-EXTEND: `n @ pat` names the whole scrutinee, and an
        // arm body of just `n` is exactly the case this inference
        // exists for. Compounds alias what the scrutinee already owns;
        // a scalar has no local to alias, so one is allocated and left
        // undefined — nothing reads it, since the real dispatch
        // allocates and stores its own.
        let mut pattern = pattern;
        while let Pattern::Binding(sym, inner) = pattern {
            let binding = match scrut {
                MatchScrutinee::Scalar { ty, .. } => {
                    let local = self.module.function_mut(self.func_id).add_local(*ty);
                    Binding::Scalar { local, ty: *ty }
                }
                MatchScrutinee::Enum(storage) => Binding::Enum(storage.clone()),
                MatchScrutinee::Struct { struct_id, fields } => Binding::Struct {
                    struct_id: *struct_id,
                    fields: fields.clone(),
                },
                MatchScrutinee::Tuple { elements } => {
                    Binding::Tuple { elements: elements.clone() }
                }
            };
            self.bindings.insert(*sym, binding);
            pattern = inner;
        }
        if let Pattern::EnumVariant(_, variant_sym, sub_patterns) = pattern
            && let MatchScrutinee::Enum(storage) = scrut {
                let enum_def = self.module.enum_def(storage.enum_id).clone();
                if let Some(variant_idx) =
                    enum_def.variants.iter().position(|v| v.name == *variant_sym)
                    && variant_idx < storage.payloads.len() {
                        for (i, sp) in sub_patterns.iter().enumerate() {
                            // PATTERN-EXTEND: `Some(n @ 3i64)` binds
                            // `n` to the payload just as `Some(n)` does.
                            let sp = match sp {
                                Pattern::Binding(sym, _) => &Pattern::Name(*sym),
                                other => other,
                            };
                            if let Pattern::Name(sym) = sp
                                && let Some(slot) =
                                    storage.payloads[variant_idx].get(i)
                                    && let PayloadSlot::Scalar { local, ty } = slot {
                                        self.bindings.insert(
                                            *sym,
                                            Binding::Scalar { local: *local, ty: *ty },
                                        );
                                    }
                                    // Enum-typed Name bindings would
                                    // require allocating a fresh
                                    // EnumStorage for inference, which
                                    // value_scalar can't see anyway —
                                    // skip.
                        }
                    }
            }
        // PATTERN-COMPOUND-LOWER: the same for compound scrutinees.
        // Without this the arm body's type cannot be inferred — a body
        // of `y * 100i64` is unreadable until `y` has a binding — and
        // the match ends up with no result local at all.
        match (pattern, scrut) {
            (Pattern::Struct(_, field_patterns, _), MatchScrutinee::Struct { fields, .. }) => {
                let fields = fields.clone();
                let field_patterns = field_patterns.clone();
                self.bind_struct_pattern_for_inference(&fields, &field_patterns);
            }
            (Pattern::Tuple(sub_patterns), MatchScrutinee::Tuple { elements }) => {
                let elements = elements.clone();
                let sub_patterns = sub_patterns.clone();
                self.bind_tuple_pattern_for_inference(&elements, &sub_patterns);
            }
            _ => {}
        }
    }

    /// Name bindings a struct pattern introduces, for arm-body type
    /// inference only — no code is emitted, so a literal field pattern
    /// contributes nothing here.
    fn bind_struct_pattern_for_inference(
        &mut self,
        fields: &[FieldBinding],
        field_patterns: &[(DefaultSymbol, Pattern)],
    ) {
        for (field_sym, sub) in field_patterns {
            let Some(field_name) = self.interner.resolve(*field_sym).map(str::to_string) else {
                continue;
            };
            let Some(shape) = fields.iter().find(|f| f.name == field_name).map(|f| f.shape.clone())
            else {
                continue;
            };
            self.bind_field_shape_for_inference(&shape, sub);
        }
    }

    fn bind_tuple_pattern_for_inference(
        &mut self,
        elements: &[TupleElementBinding],
        sub_patterns: &[Pattern],
    ) {
        for (element, sub) in elements.iter().zip(sub_patterns.iter()) {
            let shape = match &element.shape {
                TupleElementShape::Scalar { local, ty } => {
                    FieldShape::Scalar { local: *local, ty: *ty }
                }
                TupleElementShape::Struct { struct_id, fields } => FieldShape::Struct {
                    struct_id: *struct_id,
                    fields: fields.clone(),
                },
                TupleElementShape::Tuple { tuple_id, elements } => FieldShape::Tuple {
                    tuple_id: *tuple_id,
                    elements: elements.clone(),
                },
            };
            self.bind_field_shape_for_inference(&shape, sub);
        }
    }

    fn bind_field_shape_for_inference(&mut self, shape: &FieldShape, pattern: &Pattern) {
        match pattern {
            Pattern::Name(sym) => self.bind_name_to_field_shape(*sym, shape),
            // PATTERN-EXTEND: `n @ pat` names the field and `pat` may
            // name more of it.
            Pattern::Binding(sym, inner) => {
                self.bind_name_to_field_shape(*sym, shape);
                self.bind_field_shape_for_inference(shape, inner);
            }
            Pattern::Struct(_, nested, _) => {
                if let FieldShape::Struct { fields, .. } = shape {
                    let fields = fields.clone();
                    self.bind_struct_pattern_for_inference(&fields, nested);
                }
            }
            Pattern::Tuple(nested) => {
                if let FieldShape::Tuple { elements, .. } = shape {
                    let elements = elements.clone();
                    self.bind_tuple_pattern_for_inference(&elements, nested);
                }
            }
            _ => {}
        }
    }

    /// Alias one name to a field / element the scrutinee already owns.
    /// The scrutinee is a binding of the enclosing scope and outlives
    /// the arm, so nothing is copied.
    fn bind_name_to_field_shape(&mut self, sym: DefaultSymbol, shape: &FieldShape) {
        let binding = match shape {
            FieldShape::Scalar { local, ty } => Binding::Scalar { local: *local, ty: *ty },
            FieldShape::Struct { struct_id, fields } => Binding::Struct {
                struct_id: *struct_id,
                fields: fields.clone(),
            },
            FieldShape::Tuple { elements, .. } => {
                Binding::Tuple { elements: elements.clone() }
            }
        };
        self.bindings.insert(sym, binding);
    }

    /// Resolve the `match` scrutinee into a uniform shape: either an
    /// enum binding (we already know the tag local + payload locals),
    /// or a scalar value (we lower the scrutinee expression once and
    /// pin the result for arm comparisons). Other shapes (struct /
    /// tuple bindings) are not supported as scrutinees in the
    /// compiler MVP.
    /// PATTERN-COMPOUND-LOWER: check a struct pattern's field
    /// patterns against the scrutinee's own field locals, jumping to
    /// `fail_blk` on the first one that cannot match and binding the
    /// names as it goes.
    ///
    /// Field order in the pattern is free and `..` simply leaves
    /// fields unmentioned, so the walk is driven by the pattern and
    /// looks each field up by name. The type checker has already
    /// confirmed every name exists.
    fn dispatch_struct_pattern(
        &mut self,
        fields: &[FieldBinding],
        field_patterns: &[(DefaultSymbol, Pattern)],
        fail_blk: BlockId,
    ) -> Result<(), String> {
        for (field_sym, sub) in field_patterns {
            let field_name = self
                .interner
                .resolve(*field_sym)
                .ok_or_else(|| "struct pattern field name missing".to_string())?
                .to_string();
            let binding = fields
                .iter()
                .find(|f| f.name == field_name)
                .ok_or_else(|| format!("struct pattern names unknown field `{field_name}`"))?
                .shape
                .clone();
            self.dispatch_field_shape_pattern(&binding, sub, fail_blk)?;
        }
        Ok(())
    }

    /// The same for a tuple pattern, which is positional.
    fn dispatch_tuple_pattern(
        &mut self,
        elements: &[TupleElementBinding],
        sub_patterns: &[Pattern],
        fail_blk: BlockId,
    ) -> Result<(), String> {
        if sub_patterns.len() != elements.len() {
            return Err(format!(
                "tuple pattern has {} element(s), the value has {}",
                sub_patterns.len(),
                elements.len()
            ));
        }
        for (element, sub) in elements.iter().zip(sub_patterns.iter()) {
            let shape = match &element.shape {
                TupleElementShape::Scalar { local, ty } => {
                    FieldShape::Scalar { local: *local, ty: *ty }
                }
                TupleElementShape::Struct { struct_id, fields } => FieldShape::Struct {
                    struct_id: *struct_id,
                    fields: fields.clone(),
                },
                TupleElementShape::Tuple { tuple_id, elements } => FieldShape::Tuple {
                    tuple_id: *tuple_id,
                    elements: elements.clone(),
                },
            };
            self.dispatch_field_shape_pattern(&shape, sub, fail_blk)?;
        }
        Ok(())
    }

    /// One field / element: compare, bind, or recurse.
    fn dispatch_field_shape_pattern(
        &mut self,
        shape: &FieldShape,
        pattern: &Pattern,
        fail_blk: BlockId,
    ) -> Result<(), String> {
        match (pattern, shape) {
            (Pattern::Wildcard, _) => Ok(()),
            (Pattern::Name(sym), _) => {
                self.bind_name_to_field_shape(*sym, shape);
                Ok(())
            }
            // PATTERN-EXTEND: `n @ pat` names this field, then keeps
            // checking it against `pat`.
            (Pattern::Binding(sym, inner), _) => {
                self.bind_name_to_field_shape(*sym, shape);
                self.dispatch_field_shape_pattern(shape, inner, fail_blk)
            }
            (Pattern::Literal(lit_ref), FieldShape::Scalar { local, ty }) => {
                let value = self
                    .emit(InstKind::LoadLocal(*local), Some(*ty))
                    .ok_or_else(|| "field load produced no value".to_string())?;
                self.emit_literal_eq_branch(lit_ref, value, *ty, fail_blk)
            }
            (Pattern::Struct(_, nested, _), FieldShape::Struct { fields, .. }) => {
                self.dispatch_struct_pattern(fields, nested, fail_blk)
            }
            (Pattern::Tuple(nested), FieldShape::Tuple { elements, .. }) => {
                self.dispatch_tuple_pattern(elements, nested, fail_blk)
            }
            (pattern, _) => Err(format!(
                "compiler MVP cannot match {pattern:?} against this field shape"
            )),
        }
    }

    pub(super) fn classify_match_scrutinee(
        &mut self,
        scrutinee: &ExprRef,
    ) -> Result<MatchScrutinee, String> {
        let scrut_expr = self
            .program
            .expression
            .get(scrutinee)
            .ok_or_else(|| "match scrutinee missing".to_string())?;
        // Identifier shortcut: enum bindings reuse the existing
        // tag/payload locals; scalar bindings produce a single
        // LoadLocal. Non-identifier expressions go through the
        // generic scalar path below.
        if let Expr::Identifier(sym) = scrut_expr
            && let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Enum(storage) => {
                        return Ok(MatchScrutinee::Enum(storage));
                    }
                    Binding::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        return Ok(MatchScrutinee::Scalar { value: v, ty });
                    }
                    Binding::RefScalar { local, pointee_ty, .. } => {
                        // `match &mut <ref>` — auto-dereference the
                        // pointer first so the scalar match path
                        // operates on the pointee's value.
                        let ptr = self
                            .emit(InstKind::LoadLocal(local), Some(Type::U64))
                            .expect("LoadLocal returns a value");
                        let v = self
                            .emit(InstKind::LoadRef { ptr, ty: pointee_ty }, Some(pointee_ty))
                            .expect("LoadRef returns a value");
                        return Ok(MatchScrutinee::Scalar { value: v, ty: pointee_ty });
                    }
                    // PATTERN-COMPOUND-LOWER: match by field / element.
                    Binding::Struct { struct_id, fields } => {
                        return Ok(MatchScrutinee::Struct { struct_id, fields });
                    }
                    Binding::Tuple { elements, .. } => {
                        return Ok(MatchScrutinee::Tuple { elements });
                    }
                    Binding::Array { .. }
                    | Binding::FunctionPtr { .. }
                    | Binding::DynTraitObj { .. } => {
                        return Err(format!(
                            "compiler MVP does not support `match` on array / \
                             function-value / dyn-trait binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                }
            }
            // Falls through to the scalar path (could be a const).
        // ITER-PROTOCOL-AOT: `match obj.method(...)` where the
        // method returns an enum. Required by the iterator-protocol
        // desugaring (`for x in iter { ... }` lowers to
        // `match __iter.next() { Some(x) => ..., None => break }`).
        // Allocates fresh enum storage, emits a CallEnum that
        // populates it (with `&mut self` writeback if needed), and
        // returns it as the scrutinee. Identifier-bound enums
        // already short-circuited above; this arm covers
        // expression scrutinees.
        if let Expr::MethodCall(recv, method_sym, method_args) = scrut_expr.clone()
            && let Some((target_id, recv_binding)) =
                self.resolve_method_target(&recv, method_sym, &method_args)?
            {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Enum(enum_id) = target_ret {
                    let storage = self.allocate_enum_storage(enum_id);
                    let mut dests = Self::flatten_enum_dests(&storage);
                    // Receiver leaves first (matches
                    // `populate_method_writeback_types` order).
                    let mut all_args: Vec<crate::ir::ValueId> = Vec::new();
                    match &recv_binding {
                        Binding::Struct { fields, .. } => {
                            for (local, ty) in
                                super::bindings::flatten_struct_locals(fields)
                            {
                                let v = self
                                    .emit(InstKind::LoadLocal(local), Some(ty))
                                    .expect("LoadLocal returns");
                                all_args.push(v);
                            }
                        }
                        Binding::Enum(stg) => {
                            let stg = stg.clone();
                            let vs = self.load_enum_locals(&stg);
                            all_args.extend(vs);
                        }
                        _ => unreachable!(
                            "resolve_method_target only returns struct / enum receivers"
                        ),
                    }
                    for a in &method_args {
                        let v = self
                            .lower_expr(a)?
                            .ok_or_else(|| "method argument produced no value".to_string())?;
                        all_args.push(v);
                    }
                    let needs_writeback = !self
                        .module
                        .function(target_id)
                        .self_writeback_types
                        .is_empty();
                    if needs_writeback {
                        match &recv_binding {
                            Binding::Struct { fields, .. } => {
                                for (l, _) in
                                    super::bindings::flatten_struct_locals(fields)
                                {
                                    dests.push(l);
                                }
                            }
                            Binding::Enum(stg) => {
                                Self::flatten_enum_dests_into(stg, &mut dests);
                            }
                            _ => {}
                        }
                        let arg_dests =
                            self.collect_compound_writeback_dests_slice(&method_args)?;
                        dests.extend(arg_dests);
                    }
                    self.emit(
                        InstKind::CallEnum {
                            target: target_id,
                            args: all_args,
                            dests,
                        },
                        None,
                    );
                    return Ok(MatchScrutinee::Enum(storage));
                }
            }
        // AOT-MATCH-SCRUTINEE-EXPAND: `match func(...)` where the
        // function returns an enum. Same shape as the method-call arm
        // above, minus the receiver: a free function has no `self`, so
        // there are no receiver leaves to pass and no `&mut self`
        // writeback to route back. Argument writeback still applies
        // when the callee takes `&mut T`.
        //
        // Reached by `while val Some(x) = next(i)`, which the parser
        // desugars into a `match` over the call.
        if let Expr::Call(fn_name, args_ref) = scrut_expr.clone() {
            // `resolve_call_target` rather than a bare table lookup so
            // closure bindings and generic instantiation resolve the
            // same way they do at any other call site. It is
            // idempotent, so falling through to the scalar path below
            // for a non-enum return does not declare anything twice.
            let target_id = self.resolve_call_target(fn_name, &args_ref)?;
            if let Type::Enum(enum_id) = self.module.function(target_id).return_type {
                let storage = self.allocate_enum_storage(enum_id);
                let mut dests = Self::flatten_enum_dests(&storage);
                if !self
                    .module
                    .function(target_id)
                    .self_writeback_types
                    .is_empty()
                {
                    dests.extend(self.collect_compound_writeback_dests(&args_ref)?);
                }
                let arg_values = self.lower_call_args(&args_ref)?;
                self.emit(
                    InstKind::CallEnum {
                        target: target_id,
                        args: arg_values,
                        dests,
                    },
                    None,
                );
                return Ok(MatchScrutinee::Enum(storage));
            }
        }
        // Generic scalar scrutinee: lower the expression once.
        let ty = self.value_scalar(scrutinee).ok_or_else(|| {
            "compiler MVP requires `match` scrutinee to be either an enum binding \
             or an expression that produces a scalar value"
                .to_string()
        })?;
        if !matches!(ty, Type::I64 | Type::U64 | Type::Bool) {
            return Err(format!(
                "compiler MVP `match` on scalar scrutinee only supports \
                 i64 / u64 / bool, got {ty}"
            ));
        }
        let v = self
            .lower_expr(scrutinee)?
            .ok_or_else(|| "match scrutinee produced no value".to_string())?;
        Ok(MatchScrutinee::Scalar { value: v, ty })
    }

    /// Emit `lit == cmp` and a Branch to `else_blk` on inequality;
    /// the `then_blk` is freshly created and switched to so the
    /// caller continues building inside the equal-path. The literal
    /// expression must lower to a scalar value of the same `ty` as
    /// the comparand — the type-checker guarantees this in
    /// well-typed programs, so we report any mismatch as an internal
    /// drift rather than a user-facing recovery point.
    pub(super) fn emit_literal_eq_branch(
        &mut self,
        lit_ref: &ExprRef,
        cmp: ValueId,
        ty: Type,
        else_blk: BlockId,
    ) -> Result<(), String> {
        let lit_ty = self
            .value_scalar(lit_ref)
            .ok_or_else(|| "literal pattern lowering: missing literal type".to_string())?;
        if lit_ty != ty {
            return Err(format!(
                "literal pattern type `{lit_ty}` does not match scrutinee type `{ty}`"
            ));
        }
        let lit_v = self
            .lower_expr(lit_ref)?
            .ok_or_else(|| "literal pattern produced no value".to_string())?;
        let cond = self
            .emit(
                InstKind::BinOp {
                    op: BinOp::Eq,
                    lhs: cmp,
                    rhs: lit_v,
                },
                Some(Type::Bool),
            )
            .expect("Eq returns a value");
        let then_blk = self.fresh_block();
        self.terminate(Terminator::Branch {
            cond,
            then_blk,
            else_blk,
        });
        self.switch_to(then_blk);
        Ok(())
    }

}
