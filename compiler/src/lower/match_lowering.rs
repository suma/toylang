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
    Binding, EnumStorage, MatchScrutinee, PayloadSlot, TupleElementBinding,
    TupleElementShape,
};
use super::FunctionLower;
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
            let next_blk = self.fresh_block();
            // 1. Pattern shape check + sub-pattern equality checks.
            //    On any failure, jump to next_blk. On full success,
            //    advance the current block to where bindings happen.
            match &arm.pattern {
                Pattern::Wildcard => {
                    // No checks; current block keeps going.
                }
                Pattern::Literal(lit_ref) => {
                    let (scrut_v, scrut_ty) = match &scrut {
                        MatchScrutinee::Scalar { value, ty } => (*value, *ty),
                        MatchScrutinee::Enum { .. } => {
                            return Err(
                                "literal pattern is only valid against a scalar scrutinee"
                                    .to_string(),
                            );
                        }
                    };
                    self.emit_literal_eq_branch(lit_ref, scrut_v, scrut_ty, next_blk)?;
                }
                Pattern::EnumVariant(p_enum, p_variant, sub_patterns) => {
                    let scrut_storage = match &scrut {
                        MatchScrutinee::Enum(s) => s.clone(),
                        MatchScrutinee::Scalar { .. } => {
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
                        next_blk,
                    )?;
                }
                other => {
                    return Err(format!(
                        "compiler MVP `match` arms must be enum-variant, literal, or \
                         `_` patterns, got {other:?}"
                    ));
                }
            }
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
                if let (Some(local), Some(v)) = (result_local, body_v) {
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                self.terminate(Terminator::Jump(merge));
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
            match sp {
                Pattern::Literal(lit_ref) => match slot {
                    PayloadSlot::Scalar { local, ty } => {
                        let pv = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        self.emit_literal_eq_branch(lit_ref, pv, ty, next_blk)?;
                    }
                    PayloadSlot::Enum(_)
                    | PayloadSlot::Struct { .. }
                    | PayloadSlot::Tuple { .. } => {
                        return Err(
                            "literal sub-pattern is only valid against a scalar payload"
                                .to_string(),
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
                            next_blk,
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
                _ => {}
            }
        }
        // Sub-pattern bindings.
        for (i, sp) in sub_patterns.iter().enumerate() {
            let slot = scrut_storage.payloads[variant_idx][i].clone();
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
                        self.bindings.insert(*sym, Binding::Enum(copy));
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
                                fields: dst_fields,
                            },
                        );
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
        }
        Ok(())
    }

    pub(super) fn apply_arm_pattern_bindings_for_inference(
        &mut self,
        scrut: &MatchScrutinee,
        pattern: &Pattern,
    ) {
        if let Pattern::EnumVariant(_, variant_sym, sub_patterns) = pattern {
            if let MatchScrutinee::Enum(storage) = scrut {
                let enum_def = self.module.enum_def(storage.enum_id).clone();
                if let Some(variant_idx) =
                    enum_def.variants.iter().position(|v| v.name == *variant_sym)
                {
                    if variant_idx < storage.payloads.len() {
                        for (i, sp) in sub_patterns.iter().enumerate() {
                            if let Pattern::Name(sym) = sp {
                                if let Some(slot) =
                                    storage.payloads[variant_idx].get(i)
                                {
                                    if let PayloadSlot::Scalar { local, ty } = slot {
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
                    }
                }
            }
        }
    }

    /// Resolve the `match` scrutinee into a uniform shape: either an
    /// enum binding (we already know the tag local + payload locals),
    /// or a scalar value (we lower the scrutinee expression once and
    /// pin the result for arm comparisons). Other shapes (struct /
    /// tuple bindings) are not supported as scrutinees in the
    /// compiler MVP.
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
        if let Expr::Identifier(sym) = scrut_expr {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
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
                    Binding::Struct { .. } | Binding::Tuple { .. } | Binding::Array { .. } => {
                        return Err(format!(
                            "compiler MVP does not support `match` on struct / tuple / array \
                             binding `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                }
            }
            // Falls through to the scalar path (could be a const).
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
