//! Compound-value (enum / struct / tuple) storage I/O lowering.
//!
//! Centred on enums: because enum payloads can nest struct /
//! tuple shapes, the "write a value into pre-allocated storage"
//! family of helpers ends up mutually recursive across the three
//! compound kinds, so they all live here.
//!
//! - `allocate_enum_storage` / `allocate_payload_slot`: build
//!   the storage tree (`EnumStorage`) from an `EnumId`, recursing
//!   through nested enum payloads and through struct / tuple /
//!   scalar payload slots.
//! - `bind_enum` / `write_variant_into_storage`: lower an
//!   `Enum::Variant(args)` literal into a binding's storage.
//! - `load_enum_locals` / `flatten_enum_dests`: linearise the
//!   storage tree into a flat scalar slot list for the function-
//!   boundary ABI (multi-value return / multi-result `CallEnum`).
//! - `detect_enum_result`: peek-only check for whether an `if` /
//!   `match` / block always evaluates to the same enum, used by
//!   `lower_let` to decide whether to pre-allocate a target.
//! - `lower_into_enum_storage` / `write_enum_into_target`: thread
//!   an enum-producing expression into the supplied storage,
//!   recursing into if-chains and match arms via
//!   `lower_if_chain_into_enum` / `lower_match_into_enum`.
//! - `copy_enum_storage` / `copy_struct_fields` /
//!   `copy_tuple_elements`: deep-copy one storage tree into
//!   another (used by `var p = q` re-bind and by Name pattern
//!   bindings).
//! - `lower_into_struct_slot` / `lower_into_tuple_slot` /
//!   `store_value_into_tuple_element_shape`: struct / tuple
//!   counterparts of the pre-allocated write path, also called
//!   recursively from the enum payload code.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, MatchArm, Pattern, Stmt, StmtRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, EnumStorage, FieldBinding,
    FieldShape, PayloadSlot, TupleElementBinding, TupleElementShape,
};
use super::FunctionLower;
use crate::ir::{
    BlockId, Const, EnumId, InstKind, LocalId, StructId, Terminator, Type, ValueId,
};

/// Where a compound-producing expression should leave its value.
///
/// The three kinds share their control-flow walkers: an `if` chain or
/// a `match` merges the same way whichever compound its branches
/// produce, and they differ only in what one branch writes into. That
/// sharing is the point — the struct and tuple sides used to have no
/// walker at all, so every branch of a struct-producing `if` wrote
/// into *its own* freshly-allocated locals and the merge read whichever
/// set the lowering happened to see last. A taken branch other than
/// that one left the read locals untouched, and the function returned
/// a zero-filled struct with no diagnostic (MATCH-STRUCT-ARM).
#[derive(Clone)]
pub(super) enum CompoundTarget {
    Enum(EnumStorage),
    Struct {
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    Tuple {
        elements: Vec<TupleElementBinding>,
    },
}

/// What `detect_struct_result` / `detect_tuple_result` learned about
/// one branch of a composite.
pub(super) enum BranchShape<T> {
    /// The branch produces the compound, and this identifies which.
    Produces(T),
    /// The branch never reaches the merge (`panic(..)`), so it neither
    /// supplies a shape nor rules the composite out.
    Diverges,
}

/// Which instance a detected struct- or enum-producing branch names.
///
/// Detection usually learns only the template's name and leaves the
/// instantiation to the `val`'s annotation, which is where it always
/// came from. Reading the shape out of an enum payload is the
/// exception: a `Result<Vec<u64>, E>` already carries the instantiated
/// `Vec<u64>`, and an unannotated `val v = mk()?` has nowhere else to
/// get it from.
pub(super) enum ShapeSource<Id> {
    Base(DefaultSymbol),
    Instance(Id),
}

/// How a tuple-producing branch tells us its shape. Tuples have no
/// name to look up — unlike a struct or an enum, the shape *is* the
/// identity — so detection has to carry back enough to allocate from.
pub(super) enum TupleShapeSource {
    /// A literal: the element types come from its elements.
    Literal(Vec<ExprRef>),
    /// An existing tuple binding: adopt its element list wholesale.
    Binding(Vec<TupleElementBinding>),
    /// A call: the callee's interned return shape.
    Interned(crate::ir::TupleId),
}

impl<'a> FunctionLower<'a> {
    /// Peek-only: does `expr_ref` always evaluate to a value of one
    /// struct, and which? Mirrors `detect_enum_result` — `lower_let`
    /// asks before deciding to pre-allocate a target, and a `None`
    /// leaves the existing paths to handle (or reject) the rhs.
    ///
    /// `&mut self` only because resolving a block's leading `val t:
    /// Result<P, str> = ..` instantiates that generic enum; the answer
    /// does not depend on when it happens, and instantiation is
    /// idempotent.
    pub(super) fn detect_struct_result(
        &mut self,
        expr_ref: &ExprRef,
    ) -> Option<BranchShape<ShapeSource<StructId>>> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::StructLiteral(name, _) if self.struct_defs.contains_key(&name) => {
                Some(BranchShape::Produces(ShapeSource::Base(name)))
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Struct { struct_id, .. }) => {
                    Some(BranchShape::Produces(ShapeSource::Instance(*struct_id)))
                }
                _ => None,
            },
            // A call is worth recognising here even though the enum
            // version stops short of it: `if c { mk(1u64) } else {
            // mk(2u64) }` is the shape people write, and the callee's
            // declared return type says which struct it is.
            Expr::Call(fn_name, _) => match self.lookup_fn_here(None, fn_name) {
                Some(func_id) => match self.module.function(func_id).return_type {
                    Type::Struct(struct_id) => {
                        Some(BranchShape::Produces(ShapeSource::Instance(struct_id)))
                    }
                    _ => None,
                },
                None => None,
            },
            // `Box::new(..)` / `Vec::new()` and friends. The name the
            // call is qualified by is the struct it builds; when that
            // struct is generic, `resolve_struct_instance` will ask
            // for the annotation, exactly as `val v: Vec<u8> =
            // Vec::new()` already does.
            Expr::AssociatedFunctionCall(struct_name, _, _)
                if self.struct_defs.contains_key(&struct_name) =>
            {
                Some(BranchShape::Produces(ShapeSource::Base(struct_name)))
            }
            // COMPOUND-BLOCK-RHS: a struct-returning method
            // (`if c { x.twin() } else { .. }`). Resolving the target
            // may instantiate a template, which the call's own lowering
            // needs anyway, and emits nothing.
            Expr::MethodCall(recv, method, args) => {
                match self.resolve_method_target(&recv, method, &args) {
                    Ok(Some((func_id, _))) => match self.module.function(func_id).return_type {
                        Type::Struct(struct_id) => {
                            Some(BranchShape::Produces(ShapeSource::Instance(struct_id)))
                        }
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                Some(BranchShape::Diverges)
            }
            Expr::IfElifElse(_, then_body, elif_pairs, else_body) => {
                let mut bodies = vec![then_body, else_body];
                bodies.extend(elif_pairs.iter().map(|(_, b)| *b));
                self.agree_on_struct(&bodies)
            }
            Expr::Match(scrutinee, arms) => {
                let mut found: Option<ShapeSource<StructId>> = None;
                for arm in &arms {
                    let shape = match self.detect_struct_result(&arm.body) {
                        Some(shape) => shape,
                        // Not a shape detection can see on its own —
                        // try the arm's own binding.
                        None => {
                            match self.arm_binding_payload_type(&scrutinee, &arm.pattern, &arm.body)?
                            {
                                Type::Struct(struct_id) => {
                                    BranchShape::Produces(ShapeSource::Instance(struct_id))
                                }
                                _ => return None,
                            }
                        }
                    };
                    found = self.merge_struct_shape(found, shape)?;
                }
                found.map(BranchShape::Produces)
            }
            Expr::Block(stmts) => {
                let last = *stmts.last()?;
                match self.program.statement.get(&last)? {
                    Stmt::Expression(e) => {
                        let saved = self.enter_block_pending(&stmts[..stmts.len() - 1]);
                        let out = self.detect_struct_result(&e);
                        self.pending_block_enums = saved;
                        out
                    }
                    // A block that leaves through `return` never
                    // reaches the merge, so it says nothing about which
                    // struct the others produce — exactly what
                    // `panic(...)` already meant here. Without this the
                    // single most common shape in the language
                    // (`match r { Ok(v) => v, Err(e) => { println(...)
                    // return 1u64 } }`) failed detection on its error
                    // arm.
                    //
                    // `break` / `continue` are deliberately not listed:
                    // the *type checker* does not treat them as
                    // divergent either, so such an arm is rejected
                    // before lowering ever sees it ("match arms have
                    // incompatible types"). Adding them here would be
                    // dead code claiming a capability the language does
                    // not have.
                    Stmt::Return(_) => Some(BranchShape::Diverges),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// COMPOUND-BLOCK-RHS: record the enums this block's leading
    /// statements bind, and hand back the map to restore afterwards.
    ///
    /// Detection runs before any of those statements is lowered, so
    /// `bindings` cannot answer for them; without this, a tail like
    /// `match t { Ok(v) => v, .. }` has no way to learn what `v` is
    /// and the whole rhs falls through to "val/var rhs produced no
    /// value". That is the shape `?` and `??` desugar to, so every
    /// compound-carrying `expr?` depended on it.
    fn enter_block_pending(&mut self, leading: &[StmtRef]) -> HashMap<DefaultSymbol, EnumId> {
        let saved = self.pending_block_enums.clone();
        for stmt_ref in leading {
            let Some(stmt) = self.program.statement.get(stmt_ref) else {
                continue;
            };
            let (name, annotation, rhs) = match stmt {
                Stmt::Val(name, annotation, rhs) => (name, annotation, Some(rhs)),
                Stmt::Var(name, annotation, rhs) => (name, annotation, rhs),
                _ => continue,
            };
            if let Some(enum_id) = self.pending_enum_id(annotation.as_ref(), rhs.as_ref()) {
                self.pending_block_enums.insert(name, enum_id);
            }
        }
        saved
    }

    /// The enum instance a leading `val` / `var` binds, when it is
    /// readable without inferring anything.
    ///
    /// Two sources, in order: the annotation — which `?` and `??`
    /// always spell on the temp they bind — and, failing that, a plain
    /// call's declared return type, which covers the hand-written
    /// `val t = mk()` above a `match t { .. }`. A method or
    /// associated-function rhs with no annotation is a deliberate
    /// miss: resolving it means monomorphising, which is real work for
    /// a question detection is allowed to decline.
    fn pending_enum_id(
        &mut self,
        annotation: Option<&TypeDecl>,
        rhs: Option<&ExprRef>,
    ) -> Option<EnumId> {
        if let Some(base) = self.annotation_enum_base(annotation)
            && let Ok(enum_id) = self.resolve_enum_instance(base, annotation)
        {
            return Some(enum_id);
        }
        let Some(Expr::Call(fn_name, _)) = self.program.expression.get(rhs?) else {
            return None;
        };
        let func_id = self.lookup_fn_here(None, fn_name)?;
        match self.module.function(func_id).return_type {
            Type::Enum(enum_id) => Some(enum_id),
            _ => None,
        }
    }

    /// The payload type a `match` arm's pattern binds `name` to, when
    /// the arm body is that bare name.
    ///
    /// `Result::Ok(s) => s` is how every `Result`-returning
    /// constructor is unwrapped, and `s` is not in `self.bindings` at
    /// detection time — arm bindings only exist once the arm is being
    /// lowered. The type is recoverable anyway: the scrutinee's enum
    /// says what that variant's payload at that position is.
    fn arm_binding_payload_type(
        &self,
        scrutinee: &ExprRef,
        pattern: &Pattern,
        body: &ExprRef,
    ) -> Option<Type> {
        let Expr::Identifier(want) = self.program.expression.get(body)? else {
            return None;
        };
        let Pattern::EnumVariant(_, variant_name, subs) = pattern else {
            return None;
        };
        let position = subs
            .iter()
            .position(|p| matches!(p, Pattern::Name(n) if *n == want))?;
        // The scrutinee has to be a binding whose storage we already
        // hold — or one this block is about to introduce, which is the
        // same thing one lowering step later.
        let Expr::Identifier(scrutinee_name) = self.program.expression.get(scrutinee)? else {
            return None;
        };
        let enum_id = match self.bindings.get(&scrutinee_name) {
            Some(Binding::Enum(storage)) => storage.enum_id,
            _ => *self.pending_block_enums.get(&scrutinee_name)?,
        };
        let def = self.module.enum_def(enum_id);
        let variant_idx = def.variants.iter().position(|v| v.name == *variant_name)?;
        def.variants[variant_idx].payload_types.get(position).copied()
    }

    /// Every branch must produce the same struct (or diverge), and at
    /// least one must produce.
    fn agree_on_struct(&mut self, bodies: &[ExprRef]) -> Option<BranchShape<ShapeSource<StructId>>> {
        let mut found: Option<ShapeSource<StructId>> = None;
        for body in bodies {
            let shape = self.detect_struct_result(body)?;
            found = self.merge_struct_shape(found, shape)?;
        }
        found.map(BranchShape::Produces)
    }

    /// Fold one branch's shape into what the earlier branches said.
    /// Branches must name the same struct; a resolved instance wins
    /// over a bare template name, since it is the strictly more
    /// informative answer and the two agree by construction.
    fn merge_struct_shape(
        &self,
        found: Option<ShapeSource<StructId>>,
        shape: BranchShape<ShapeSource<StructId>>,
    ) -> Option<Option<ShapeSource<StructId>>> {
        let produced = match shape {
            BranchShape::Diverges => return Some(found),
            BranchShape::Produces(source) => source,
        };
        let Some(seen) = found else {
            return Some(Some(produced));
        };
        if self.struct_shape_base(&seen) != self.struct_shape_base(&produced) {
            return None;
        }
        Some(Some(match (seen, produced) {
            (ShapeSource::Base(_), other) => other,
            (kept, _) => kept,
        }))
    }

    /// The template name behind either spelling of a struct shape.
    fn struct_shape_base(&self, source: &ShapeSource<StructId>) -> DefaultSymbol {
        match source {
            ShapeSource::Base(name) => *name,
            ShapeSource::Instance(id) => self.module.struct_def(*id).base_name,
        }
    }

    /// Tuple counterpart of `detect_struct_result`.
    pub(super) fn detect_tuple_result(
        &mut self,
        expr_ref: &ExprRef,
    ) -> Option<BranchShape<TupleShapeSource>> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::TupleLiteral(elems) => {
                Some(BranchShape::Produces(TupleShapeSource::Literal(elems)))
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Tuple { elements }) => Some(BranchShape::Produces(
                    TupleShapeSource::Binding(elements.clone()),
                )),
                _ => None,
            },
            Expr::Call(fn_name, _) => match self.lookup_fn_here(None, fn_name) {
                Some(func_id) => match self.module.function(func_id).return_type {
                    Type::Tuple(tuple_id) => {
                        Some(BranchShape::Produces(TupleShapeSource::Interned(tuple_id)))
                    }
                    _ => None,
                },
                None => None,
            },
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                Some(BranchShape::Diverges)
            }
            Expr::IfElifElse(_, then_body, elif_pairs, else_body) => {
                let mut bodies = vec![then_body, else_body];
                bodies.extend(elif_pairs.iter().map(|(_, b)| *b));
                self.agree_on_tuple(&bodies)
            }
            Expr::Match(scrutinee, arms) => {
                let mut found: Option<TupleShapeSource> = None;
                for arm in &arms {
                    let shape = match self.detect_tuple_result(&arm.body) {
                        Some(shape) => shape,
                        // Same arm-binding fallback the struct side
                        // has: `Result::Ok(t) => t` names a tuple
                        // through the scrutinee's variant payload,
                        // which is already interned.
                        None => {
                            match self.arm_binding_payload_type(&scrutinee, &arm.pattern, &arm.body)?
                            {
                                Type::Tuple(tuple_id) => {
                                    BranchShape::Produces(TupleShapeSource::Interned(tuple_id))
                                }
                                _ => return None,
                            }
                        }
                    };
                    match shape {
                        BranchShape::Diverges => {}
                        BranchShape::Produces(source) => {
                            if found.is_none() {
                                found = Some(source);
                            }
                        }
                    }
                }
                found.map(BranchShape::Produces)
            }
            Expr::Block(stmts) => {
                let last = *stmts.last()?;
                match self.program.statement.get(&last)? {
                    Stmt::Expression(e) => {
                        let saved = self.enter_block_pending(&stmts[..stmts.len() - 1]);
                        let out = self.detect_tuple_result(&e);
                        self.pending_block_enums = saved;
                        out
                    }
                    Stmt::Return(_) => Some(BranchShape::Diverges),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Every branch must be tuple-producing (or diverge); the first
    /// one that produces supplies the shape. Tuples are structural and
    /// the type checker has already agreed the branches share a type,
    /// so there is nothing further to compare.
    fn agree_on_tuple(&mut self, bodies: &[ExprRef]) -> Option<BranchShape<TupleShapeSource>> {
        let mut found: Option<TupleShapeSource> = None;
        for body in bodies {
            match self.detect_tuple_result(body)? {
                BranchShape::Diverges => {}
                BranchShape::Produces(source) => {
                    if found.is_none() {
                        found = Some(source);
                    }
                }
            }
        }
        found.map(BranchShape::Produces)
    }

    /// Thread a compound-producing expression into pre-allocated
    /// storage, whatever kind of compound that is.
    pub(super) fn lower_into_compound_target(
        &mut self,
        expr_ref: &ExprRef,
        target: &CompoundTarget,
    ) -> Result<(), String> {
        if let Some(decls) = self.program.drop_flags.clear_before_expr.get(expr_ref) {
            let decls = decls.clone();
            self.clear_drop_flags(&decls);
        }
        match target {
            CompoundTarget::Enum(storage) => self.lower_into_enum_storage(expr_ref, storage),
            CompoundTarget::Struct { struct_id, fields } => {
                self.lower_into_struct_fields(expr_ref, *struct_id, fields)
            }
            CompoundTarget::Tuple { elements } => {
                self.lower_into_tuple_elements(expr_ref, elements)
            }
        }
    }

    /// Struct counterpart of `lower_into_enum_storage`: composite
    /// shapes recurse so every branch converges on `fields`, and
    /// anything else is a leaf that `store_struct_value_into_fields`
    /// already knows how to build (literal, binding, field chain,
    /// function / associated-function / method call).
    pub(super) fn lower_into_struct_fields(
        &mut self,
        expr_ref: &ExprRef,
        struct_id: StructId,
        fields: &[FieldBinding],
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "struct-target expression missing".to_string())?;
        let target = CompoundTarget::Struct {
            struct_id,
            fields: fields.to_vec(),
        };
        match expr {
            Expr::Block(stmts) => self.lower_block_into_compound(&stmts, &target),
            Expr::IfElifElse(cond, then_body, elif_pairs, else_body) => self
                .lower_if_chain_into_compound(
                    &cond,
                    &then_body,
                    &elif_pairs,
                    &else_body,
                    &target,
                ),
            Expr::Match(scrutinee, arms) => {
                self.lower_match_into_compound(&scrutinee, &arms, &target)
            }
            // Same for a branch that panics: lowering it as an
            // expression emits the trap and marks the block
            // unreachable, which is all the merge needs.
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                self.lower_expr(expr_ref)?;
                Ok(())
            }
            _ => self.store_struct_value_into_fields(struct_id, fields, expr_ref),
        }
    }

    /// Tuple counterpart of `lower_into_struct_fields`.
    pub(super) fn lower_into_tuple_elements(
        &mut self,
        expr_ref: &ExprRef,
        elements: &[TupleElementBinding],
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "tuple-target expression missing".to_string())?;
        let target = CompoundTarget::Tuple {
            elements: elements.to_vec(),
        };
        match expr {
            Expr::Block(stmts) => self.lower_block_into_compound(&stmts, &target),
            Expr::IfElifElse(cond, then_body, elif_pairs, else_body) => self
                .lower_if_chain_into_compound(
                    &cond,
                    &then_body,
                    &elif_pairs,
                    &else_body,
                    &target,
                ),
            Expr::Match(scrutinee, arms) => {
                self.lower_match_into_compound(&scrutinee, &arms, &target)
            }
            // Same for a branch that panics: lowering it as an
            // expression emits the trap and marks the block
            // unreachable, which is all the merge needs.
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                self.lower_expr(expr_ref)?;
                Ok(())
            }
            _ => self.store_tuple_value_into_elements(elements, expr_ref),
        }
    }

    /// A block produces its tail expression's value, so the leading
    /// statements are lowered as themselves and only the tail is
    /// threaded into the target.
    pub(super) fn lower_block_into_compound(
        &mut self,
        stmts: &[frontend::ast::StmtRef],
        target: &CompoundTarget,
    ) -> Result<(), String> {
        if stmts.is_empty() {
            return Err("empty block cannot produce a compound value".to_string());
        }
        // RETURN-DROP: the block's bindings are registered in the scope
        // open around it, and live to its end -- `val f = File::open(p)?`
        // is `{ val t = ..  match t { Ok(v) => v, .. } }`, whose owner
        // `t` must outlive the block. A function body lowered here (one
        // returning an enum) has no scope around it, and used to drop
        // nothing at all: it gets the function's scope. Anywhere else
        // the block may sit in a branch, so what it registers goes
        // behind a flag (`attach_drop_flag`) -- or the enclosing scope
        // would drop it on paths that never made it.
        let own_scope = self.drop_scopes.is_empty();
        if own_scope {
            self.enter_function_drop_scope();
        } else {
            self.compound_block_depth += 1;
        }
        let result = self.lower_block_into_compound_stmts(stmts, target);
        if !own_scope {
            self.compound_block_depth -= 1;
        }
        result
    }

    fn lower_block_into_compound_stmts(
        &mut self,
        stmts: &[frontend::ast::StmtRef],
        target: &CompoundTarget,
    ) -> Result<(), String> {
        // Where this block's own registrations begin in the top scope.
        let start = self.drop_scopes.last().map_or(0, Vec::len);
        for (i, stmt_ref) in stmts.iter().enumerate() {
            let is_last = i + 1 == stmts.len();
            let stmt = self
                .program
                .statement
                .get(stmt_ref)
                .ok_or_else(|| "missing block stmt".to_string())?;
            if is_last
                && let Stmt::Expression(e) = stmt
            {
                self.lower_into_compound_target(&e, target)?;
                // A tail naming a binding hands that value out: the
                // target holds it now.
                self.forget_escaping_binding(&e, start);
                return Ok(());
            }
            let _ = self.lower_stmt(stmt_ref)?;
            // A branch that leaves through a `return` never reaches
            // the merge, so it has no value to write into the target
            // and no tail expression to demand one from.
            if self.is_unreachable() {
                return Ok(());
            }
        }
        Err("block has no compound-producing tail expression".to_string())
    }

    /// `expr` is a block's tail that names a binding the block made
    /// (registered from `start` in the top scope): that binding's value
    /// leaves the block, so it is not dropped as the block's. A binding
    /// from further out is the move check's business -- it is handed
    /// over, and flagged, when it is the function's value.
    fn forget_escaping_binding(&mut self, expr: &ExprRef, start: usize) {
        let Some(frontend::ast::Expr::Identifier(sym)) = self.program.expression.get(expr) else {
            return;
        };
        let leaves = match self.bindings.get(&sym) {
            Some(Binding::Struct { fields, .. }) => crate::bindings::flatten_struct_locals(fields),
            Some(Binding::Enum(storage)) => crate::bindings::flatten_enum_storage_locals(storage),
            Some(Binding::Tuple { elements }) => crate::bindings::flatten_tuple_element_locals(elements),
            _ => return,
        };
        if let Some(top) = self.drop_scopes.last_mut()
            && start <= top.len()
        {
            let mut i = 0;
            top.retain(|t| {
                let keep = i < start || !t.field_locals.iter().any(|l| leaves.contains(l));
                i += 1;
                keep
            });
        }
    }

    pub(super) fn allocate_enum_storage(&mut self, enum_id: EnumId) -> EnumStorage {
        let enum_def = self.module.enum_def(enum_id).clone();
        let tag_local = self
            .module
            .function_mut(self.func_id)
            .add_local(Type::U64);
        let mut payloads: Vec<Vec<PayloadSlot>> =
            Vec::with_capacity(enum_def.variants.len());
        for variant in &enum_def.variants {
            let mut per_variant: Vec<PayloadSlot> =
                Vec::with_capacity(variant.payload_types.len());
            for ty in &variant.payload_types {
                per_variant.push(self.allocate_payload_slot(*ty));
            }
            payloads.push(per_variant);
        }
        EnumStorage {
            enum_id,
            tag_local,
            payloads,
        }
    }

    /// Allocate one payload slot of the given type. Scalar types
    /// occupy a single local; enum types recursively allocate a full
    /// nested `EnumStorage`. The function-boundary flattening in
    /// codegen mirrors the same recursion via
    /// `flatten_struct_to_cranelift_tys`.
    pub(super) fn allocate_payload_slot(&mut self, ty: Type) -> PayloadSlot {
        match ty {
            Type::Enum(inner_id) => {
                PayloadSlot::Enum(Box::new(self.allocate_enum_storage(inner_id)))
            }
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                PayloadSlot::Struct { struct_id, fields }
            }
            Type::Tuple(tuple_id) => {
                let elements = self
                    .allocate_tuple_elements(tuple_id)
                    .unwrap_or_default();
                PayloadSlot::Tuple { tuple_id, elements }
            }
            Type::Unit => PayloadSlot::Unit,
            _ => {
                let local = self.module.function_mut(self.func_id).add_local(ty);
                PayloadSlot::Scalar { local, ty }
            }
        }
    }

    /// Allocate the storage for an enum binding (one tag local + one
    /// payload local per element across **all** variants), then
    /// initialise the tag to `variant_idx` and the chosen variant's
    /// payload slots from `args`. Other variants' payload slots stay
    /// uninitialised — the match lowering only ever loads them after
    /// confirming the tag dispatch, so an uninit read can't escape.
    pub(super) fn bind_enum(
        &mut self,
        binding_name: DefaultSymbol,
        enum_id: EnumId,
        variant_idx: usize,
        args: &[ExprRef],
    ) -> Result<(), String> {
        let storage = self.allocate_enum_storage(enum_id);
        self.bindings
            .insert(binding_name, Binding::Enum(storage.clone()));
        self.write_variant_into_storage(&storage, variant_idx, args)?;
        Ok(())
    }

    /// Store `variant_idx` into the storage's tag local, then
    /// evaluate each payload arg and store it into the matching
    /// slot. For enum-typed payloads, the arg is also expected to
    /// be an enum producer (literal, identifier, or composite); we
    /// recurse into `lower_into_enum_target` to write the nested
    /// EnumStorage. Other variants' slots stay uninit.
    pub(super) fn write_variant_into_storage(
        &mut self,
        storage: &EnumStorage,
        variant_idx: usize,
        args: &[ExprRef],
    ) -> Result<(), String> {
        let tag_v = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: storage.tag_local,
                src: tag_v,
            },
            None,
        );
        for (i, arg_ref) in args.iter().enumerate() {
            let slot = storage.payloads[variant_idx][i].clone();
            match slot {
                PayloadSlot::Scalar { local, .. } => {
                    let v = self.lower_expr(arg_ref)?.ok_or_else(|| {
                        format!("enum payload arg #{i} produced no value")
                    })?;
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                // A `()` payload stores nothing, but the argument is
                // still an expression and may do something on the way
                // to producing no value.
                PayloadSlot::Unit => {
                    self.lower_expr(arg_ref)?;
                }
                PayloadSlot::Enum(inner_storage) => {
                    self.lower_into_enum_storage(arg_ref, &inner_storage)?;
                }
                PayloadSlot::Struct {
                    struct_id: slot_struct_id,
                    fields: slot_fields,
                } => {
                    self.lower_into_struct_slot(arg_ref, slot_struct_id, &slot_fields)?;
                }
                PayloadSlot::Tuple {
                    tuple_id: slot_tuple_id,
                    elements: slot_elements,
                } => {
                    self.lower_into_tuple_slot(arg_ref, slot_tuple_id, &slot_elements)?;
                }
            }
        }
        Ok(())
    }

    /// Read every local that backs an enum binding into a flat
    /// vector of values, suitable as the operand list for a
    /// multi-value `Return` or a `CallEnum` argument expansion.
    /// Recurses through nested `Enum` payload slots so the order
    /// matches `flatten_struct_to_cranelift_tys` exactly.
    pub(super) fn load_enum_locals(&mut self, storage: &EnumStorage) -> Vec<ValueId> {
        let mut out = Vec::new();
        self.load_enum_locals_into(storage, &mut out);
        out
    }

    pub(super) fn load_enum_locals_into(&mut self, storage: &EnumStorage, out: &mut Vec<ValueId>) {
        let tag_v = self
            .emit(InstKind::LoadLocal(storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        out.push(tag_v);
        for variant in &storage.payloads {
            for slot in variant {
                match slot {
                    PayloadSlot::Scalar { local, ty } => {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        out.push(v);
                    }
                    PayloadSlot::Enum(inner) => {
                        self.load_enum_locals_into(inner, out);
                    }
                    PayloadSlot::Struct { fields, .. } => {
                        let leaves = flatten_struct_locals(fields);
                        for (local, ty) in leaves {
                            let v = self
                                .emit(InstKind::LoadLocal(local), Some(ty))
                                .expect("LoadLocal returns a value");
                            out.push(v);
                        }
                    }
                    PayloadSlot::Tuple { elements, .. } => {
                        for (local, ty) in flatten_tuple_element_locals(elements) {
                            let v = self
                                .emit(InstKind::LoadLocal(local), Some(ty))
                                .expect("LoadLocal returns a value");
                            out.push(v);
                        }
                    }
                    // No local, so no value to load (UNIT-TYPE-ARG).
                    PayloadSlot::Unit => {}
                }
            }
        }
    }

    /// Flatten an EnumStorage into the dest list for `CallEnum`
    /// (tag first, then each variant's payloads in declaration
    /// order, recursing through nested enums).
    /// ENUM-ARG-NEST: emit an enum-returning call whose multi-return
    /// slots land straight in `storage`'s leaf locals.
    ///
    /// An enum never flows through SSA as one value, so a call that
    /// produces one needs somewhere to put its leaves before the call
    /// is emitted. On a `val` RHS that somewhere is the new binding;
    /// here it is whatever storage the caller already allocated — an
    /// argument slot or another enum's payload — which is what lets
    /// `node(leaf(), 1i64, leaf())` and `Option::Some(mk(2i64))` be
    /// written without a binding per intermediate value.
    pub(super) fn emit_enum_call_into_storage(
        &mut self,
        storage: &EnumStorage,
        target_id: crate::ir::FuncId,
        args_items: &[ExprRef],
    ) -> Result<(), String> {
        let mut dests = Self::flatten_enum_dests(storage);
        if !self.module.function(target_id).self_writeback_types.is_empty() {
            dests.extend(self.collect_compound_writeback_dests_for(args_items, Some(target_id), 0)?);
        }
        let (arg_values, ptr_arg_reloads) =
            self.lower_call_arg_items(args_items, Some(target_id))?;
        self.emit(
            InstKind::CallEnum {
                target: target_id,
                args: arg_values,
                dests,
            },
            None,
        );
        for r in ptr_arg_reloads {
            r.apply(self);
        }
        Ok(())
    }

    /// ENUM-ASSOC-FN-PRODUCER: the `FuncId` of an associated function
    /// whose return type is `target_enum_id`, or `None` when
    /// `owner::fn_name` is not one.
    ///
    /// The instance matters: `Span::try_from_raw_parts` is declared
    /// `-> Option<Self>`, so which `Option` it returns depends on
    /// which `Span<T>` it belongs to. The target enum's own payload
    /// names that struct instance — an `Option<Span<u8>>` slot holds a
    /// `Span<u8>` — which is the only thing here that can pick it.
    fn resolve_enum_producing_assoc_fn(
        &mut self,
        owner: DefaultSymbol,
        fn_name: DefaultSymbol,
        target_enum_id: EnumId,
        args: &[ExprRef],
    ) -> Result<Option<crate::ir::FuncId>, String> {
        if !self.struct_defs.contains_key(&owner) {
            return Ok(None);
        }
        let from_payload = self
            .module
            .enum_def(target_enum_id)
            .variants
            .iter()
            .flat_map(|v| v.payload_types.iter())
            .find_map(|t| match t {
                Type::Struct(id)
                    if self.module.struct_def(*id).base_name == owner =>
                {
                    Some(*id)
                }
                _ => None,
            });
        let struct_id = match from_payload {
            Some(id) => id,
            None => self.resolve_struct_instance(owner, None)?,
        };
        let Some(func_id) =
            self.resolve_struct_method_func_id(owner, fn_name, struct_id, args)?
        else {
            return Ok(None);
        };
        if self.module.function(func_id).return_type == Type::Enum(target_enum_id) {
            Ok(Some(func_id))
        } else {
            Ok(None)
        }
    }

    pub(super) fn flatten_enum_dests(storage: &EnumStorage) -> Vec<LocalId> {
        let mut out = Vec::new();
        Self::flatten_enum_dests_into(storage, &mut out);
        out
    }

    pub(super) fn flatten_enum_dests_into(storage: &EnumStorage, out: &mut Vec<LocalId>) {
        out.push(storage.tag_local);
        for variant in &storage.payloads {
            for slot in variant {
                match slot {
                    PayloadSlot::Scalar { local, .. } => out.push(*local),
                    PayloadSlot::Enum(inner) => Self::flatten_enum_dests_into(inner, out),
                    PayloadSlot::Struct { fields, .. } => {
                        for (local, _) in flatten_struct_locals(fields) {
                            out.push(local);
                        }
                    }
                    PayloadSlot::Tuple { elements, .. } => {
                        for (local, _) in flatten_tuple_element_locals(elements) {
                            out.push(local);
                        }
                    }
                    // No local, so no destination (UNIT-TYPE-ARG).
                    PayloadSlot::Unit => {}
                }
            }
        }
    }

    /// Detect whether an expression evaluates to a value of some
    /// **known enum type**, walking through if-chains, match arms, and
    /// `{ ...; tail }` blocks. Returns the enum's symbol when every
    /// branch / arm / tail produces the same enum, otherwise `None`.
    /// This is the gate that picks the composite enum-result lowering
    /// path in `lower_let`; we only commit to the parallel
    /// `lower_into_enum_target` walk when we know all sub-trees end
    /// in enum producers.
    pub(super) fn detect_enum_result(
        &mut self,
        expr_ref: &ExprRef,
    ) -> Option<BranchShape<ShapeSource<EnumId>>> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::QualifiedIdentifier(path)
                if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
            {
                Some(BranchShape::Produces(ShapeSource::Base(path[0])))
            }
            Expr::AssociatedFunctionCall(en, name, _)
                if self.enum_defs.contains_key(&en)
                    // FROM-INTO-ENUM-ERR: `Enum::Variant(args)` and
                    // `Enum::method(args)` parse to the same shape —
                    // only a declared variant name is a construction.
                    // An associated function (`MyErr::from(e)`) falls
                    // through so the let-rhs dispatch reaches the
                    // enum-associated-call intercept.
                    && self.enum_variant_index(&en, &name).is_some() =>
            {
                Some(BranchShape::Produces(ShapeSource::Base(en)))
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Enum(storage)) => {
                    Some(BranchShape::Produces(ShapeSource::Instance(storage.enum_id)))
                }
                // A name this block binds a line or two above the tail
                // (COMPOUND-BLOCK-RHS); it will be a real binding by
                // the time the tail is lowered.
                _ => self
                    .pending_block_enums
                    .get(&sym)
                    .map(|id| BranchShape::Produces(ShapeSource::Instance(*id))),
            },
            // A branch that traps says nothing about which enum the
            // others produce — the same meaning it has on the struct
            // and tuple sides.
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                Some(BranchShape::Diverges)
            }
            Expr::IfElifElse(_, then_body, elif_pairs, else_body) => {
                let mut bodies = vec![then_body, else_body];
                bodies.extend(elif_pairs.iter().map(|(_, b)| *b));
                self.agree_on_enum(&bodies)
            }
            Expr::Match(scrutinee, arms) => {
                let mut found: Option<ShapeSource<EnumId>> = None;
                for arm in &arms {
                    let shape = match self.detect_enum_result(&arm.body) {
                        Some(shape) => shape,
                        // The arm's own binding, as on the struct and
                        // tuple sides: `Result::Ok(o) => o` where the
                        // payload is itself an enum (`Option<T>`).
                        None => {
                            match self.arm_binding_payload_type(&scrutinee, &arm.pattern, &arm.body)?
                            {
                                Type::Enum(enum_id) => {
                                    BranchShape::Produces(ShapeSource::Instance(enum_id))
                                }
                                _ => return None,
                            }
                        }
                    };
                    found = self.merge_enum_shape(found, shape)?;
                }
                found.map(BranchShape::Produces)
            }
            Expr::Block(stmts) => {
                let last = *stmts.last()?;
                match self.program.statement.get(&last)? {
                    Stmt::Expression(e) => {
                        let saved = self.enter_block_pending(&stmts[..stmts.len() - 1]);
                        let out = self.detect_enum_result(&e);
                        self.pending_block_enums = saved;
                        out
                    }
                    Stmt::Return(_) => Some(BranchShape::Diverges),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Enum counterpart of `agree_on_struct`.
    fn agree_on_enum(&mut self, bodies: &[ExprRef]) -> Option<BranchShape<ShapeSource<EnumId>>> {
        let mut found: Option<ShapeSource<EnumId>> = None;
        for body in bodies {
            let shape = self.detect_enum_result(body)?;
            found = self.merge_enum_shape(found, shape)?;
        }
        found.map(BranchShape::Produces)
    }

    /// Enum counterpart of `merge_struct_shape`.
    fn merge_enum_shape(
        &self,
        found: Option<ShapeSource<EnumId>>,
        shape: BranchShape<ShapeSource<EnumId>>,
    ) -> Option<Option<ShapeSource<EnumId>>> {
        let produced = match shape {
            BranchShape::Diverges => return Some(found),
            BranchShape::Produces(source) => source,
        };
        let Some(seen) = found else {
            return Some(Some(produced));
        };
        if self.enum_shape_base(&seen) != self.enum_shape_base(&produced) {
            return None;
        }
        Some(Some(match (seen, produced) {
            (ShapeSource::Base(_), other) => other,
            (kept, _) => kept,
        }))
    }

    /// The template name behind either spelling of an enum shape.
    fn enum_shape_base(&self, source: &ShapeSource<EnumId>) -> DefaultSymbol {
        match source {
            ShapeSource::Base(name) => *name,
            ShapeSource::Instance(id) => self.module.enum_def(*id).base_name,
        }
    }

    /// Lower an expression whose result is an enum value of
    /// `enum_name`, writing the chosen variant into the supplied
    /// `tag_local` + `payload_locals` instead of allocating fresh
    /// storage. Mirrors `lower_let`'s direct-construction paths but
    /// re-uses the caller-provided locals. For composite expressions
    /// (if-chains, match, blocks), each branch's tail recurses into
    /// the same target so all paths converge on the same locals —
    /// cranelift's SSA construction takes care of the merge.
    pub(super) fn lower_into_enum_storage(
        &mut self,
        expr_ref: &ExprRef,
        target: &EnumStorage,
    ) -> Result<(), String> {
        let target_enum_id = target.enum_id;
        let expected_base = self.module.enum_def(target_enum_id).base_name;
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "enum-target expression missing".to_string())?;
        match expr {
            Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                if path[0] != expected_base {
                    return Err(format!(
                        "branch produces enum `{}` but the surrounding binding expects `{}`",
                        self.interner.resolve(path[0]).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let enum_def = self.module.enum_def(target_enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == path[1])
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(expected_base).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                        )
                    })?;
                if !enum_def.variants[variant_idx].payload_types.is_empty() {
                    return Err(format!(
                        "enum variant `{}::{}` is a tuple variant; supply its arguments \
                         via `{}::{}(...)`",
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(path[1]).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(path[1]).unwrap_or("?"),
                    ));
                }
                self.write_variant_into_storage(target, variant_idx, &[])?;
                Ok(())
            }
            Expr::AssociatedFunctionCall(en, var, args) => {
                // `Enum::Variant(payload)` — a construction of the
                // very enum this slot holds.
                let enum_def = self.module.enum_def(target_enum_id).clone();
                let variant_idx = if en == expected_base {
                    enum_def.variants.iter().position(|v| v.name == var)
                } else {
                    None
                };
                if let Some(variant_idx) = variant_idx {
                    let expected = enum_def.variants[variant_idx].payload_types.len();
                    if args.len() != expected {
                        return Err(format!(
                            "enum variant `{}::{}` expects {} payload value(s), got {}",
                            self.interner.resolve(expected_base).unwrap_or("?"),
                            self.interner.resolve(var).unwrap_or("?"),
                            expected,
                            args.len(),
                        ));
                    }
                    return self.write_variant_into_storage(target, variant_idx, &args);
                }
                // ENUM-ASSOC-FN-PRODUCER: otherwise this is an
                // associated function that *returns* the enum —
                // `Span::try_from_raw_parts(p, n) -> Option<Self>`, the
                // one-call constructor the fallible-window API is built
                // around. The old arm assumed any `A::b(..)` here was a
                // variant construction, so it reported "branch produces
                // enum `Span` but the surrounding binding expects
                // `Option`", naming a struct as an enum and the wrong
                // type as the product.
                if let Some(func_id) =
                    self.resolve_enum_producing_assoc_fn(en, var, target_enum_id, &args)?
                {
                    return self.emit_enum_call_into_storage(target, func_id, &args);
                }
                Err(format!(
                    "`{}::{}` is neither a variant of `{}` nor an associated function \
                     returning it",
                    self.interner.resolve(en).unwrap_or("?"),
                    self.interner.resolve(var).unwrap_or("?"),
                    self.interner.resolve(expected_base).unwrap_or("?"),
                ))
            }
            Expr::Identifier(sym) => {
                let src = match self.bindings.get(&sym).cloned() {
                    Some(Binding::Enum(s)) if s.enum_id == target_enum_id => s,
                    _ => {
                        return Err(format!(
                            "`{}` is not an enum binding of the expected type",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.copy_enum_storage(&src, target);
                Ok(())
            }
            Expr::Block(stmts) => {
                self.lower_block_into_compound(&stmts, &CompoundTarget::Enum(target.clone()))
            }
            Expr::IfElifElse(cond, then_body, elif_pairs, else_body) => self
                .lower_if_chain_into_compound(
                    &cond,
                    &then_body,
                    &elif_pairs,
                    &else_body,
                    &CompoundTarget::Enum(target.clone()),
                ),
            Expr::Match(scrutinee, arms) => self.lower_match_into_compound(
                &scrutinee,
                &arms,
                &CompoundTarget::Enum(target.clone()),
            ),
            Expr::BuiltinCall(frontend::ast::BuiltinFunction::Panic, _) => {
                self.lower_expr(expr_ref)?;
                Ok(())
            }
            // ENUM-ARG-NEST: an enum-returning call filling this slot
            // (`Option::Some(mk(2i64))`, or an `if` arm that calls a
            // constructor). The struct counterpart has had this since
            // COMPOUND-BLOCK-RHS; the enum side only knew about
            // literals, bindings and branches, so any call had to be
            // hoisted to its own `val` first.
            Expr::Call(fn_name, args_ref) => {
                let target_id = self
                    .lookup_fn_here(None, fn_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown function `{}` in enum-producing position",
                            self.interner.resolve(fn_name).unwrap_or("?")
                        )
                    })?;
                let ret = self.module.function(target_id).return_type;
                if ret != Type::Enum(target_enum_id) {
                    return Err(format!(
                        "`{}` returns {}, but this slot holds `{}`",
                        self.interner.resolve(fn_name).unwrap_or("?"),
                        super::spelling::spell_type(self.module, self.interner, ret),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let items: Vec<ExprRef> = match self.program.expression.get(&args_ref) {
                    Some(Expr::ExprList(items)) => items,
                    _ => return Err("call args missing".to_string()),
                };
                self.emit_enum_call_into_storage(target, target_id, &items)
            }
            // Same for an enum-returning *method* — `val o = it.next()`
            // worked, `Option::Some(it.next())` did not.
            Expr::MethodCall(recv, method_sym, method_args) => {
                let Some(call) =
                    self.prepare_compound_method_call(&recv, method_sym, &method_args)?
                else {
                    return Err(format!(
                        "`{}` is not a struct- or enum-receiver method returning a compound value",
                        self.interner.resolve(method_sym).unwrap_or("?"),
                    ));
                };
                let reload = call.reload;
                if call.ret != Type::Enum(target_enum_id) {
                    return Err(format!(
                        "method `{}` returns {}, but this slot holds `{}`",
                        self.interner.resolve(method_sym).unwrap_or("?"),
                        super::spelling::spell_type(self.module, self.interner, call.ret),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let mut dests = Self::flatten_enum_dests(target);
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallEnum {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
                reload.apply(self);
                Ok(())
            }
            other => Err(format!(
                "compiler MVP cannot lower {} as an enum-producing expression in this position",
                crate::spelling::describe_expr(self.interner, &other)
            )),
        }
    }

    /// Common store: write the variant tag and (optionally) evaluate
    /// + store the payload args into the target's per-variant slots.
    ///
    /// (deprecated — kept temporarily during the refactor)
    #[allow(dead_code)]
    pub(super) fn write_enum_into_target(
        &mut self,
        variant_idx: usize,
        args: &[ExprRef],
        tag_local: LocalId,
        payload_locals: &[Vec<(LocalId, Type)>],
    ) -> Result<(), String> {
        let tag_v = self
            .emit(
                InstKind::Const(Const::U64(variant_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: tag_local,
                src: tag_v,
            },
            None,
        );
        for (i, arg_ref) in args.iter().enumerate() {
            let v = self
                .lower_expr(arg_ref)?
                .ok_or_else(|| format!("enum payload arg #{i} produced no value"))?;
            let (dst, _) = payload_locals[variant_idx][i];
            self.emit(InstKind::StoreLocal { dst, src: v }, None);
        }
        Ok(())
    }

    /// Copy every local backing a source enum binding into the
    /// target's matching slot. Recurses through nested enum payloads
    /// so a `val a = b` between two `Option<Option<T>>` bindings
    /// duplicates the full storage tree.
    pub(super) fn copy_enum_storage(&mut self, src: &EnumStorage, dst: &EnumStorage) {
        debug_assert_eq!(src.enum_id, dst.enum_id);
        let v = self
            .emit(InstKind::LoadLocal(src.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        self.emit(
            InstKind::StoreLocal {
                dst: dst.tag_local,
                src: v,
            },
            None,
        );
        for (variant_idx, variant_slots) in src.payloads.iter().enumerate() {
            for (i, src_slot) in variant_slots.iter().enumerate() {
                let dst_slot = &dst.payloads[variant_idx][i];
                match (src_slot, dst_slot) {
                    (
                        PayloadSlot::Scalar { local: sl, ty },
                        PayloadSlot::Scalar { local: dl, .. },
                    ) => {
                        let v = self
                            .emit(InstKind::LoadLocal(*sl), Some(*ty))
                            .expect("LoadLocal returns a value");
                        self.emit(
                            InstKind::StoreLocal { dst: *dl, src: v },
                            None,
                        );
                    }
                    (PayloadSlot::Enum(s), PayloadSlot::Enum(d)) => {
                        let s = (**s).clone();
                        let d = (**d).clone();
                        self.copy_enum_storage(&s, &d);
                    }
                    (
                        PayloadSlot::Struct { fields: sf, .. },
                        PayloadSlot::Struct { fields: df, .. },
                    ) => {
                        let sf = sf.clone();
                        let df = df.clone();
                        self.copy_struct_fields(&sf, &df);
                    }
                    (
                        PayloadSlot::Tuple { elements: se, .. },
                        PayloadSlot::Tuple { elements: de, .. },
                    ) => {
                        let se = se.clone();
                        let de = de.clone();
                        self.copy_tuple_elements(&se, &de);
                    }
                    // A `()` payload (`Result<(), E>`'s `Ok`) has no leaf to
                    // move, so copying it is doing nothing -- but the arm
                    // has to exist: without it, binding such a value to a
                    // `val` reached the `unreachable!` below.
                    (PayloadSlot::Unit, PayloadSlot::Unit) => {}
                    _ => unreachable!("payload slot shape mismatch"),
                }
            }
        }
    }

    /// Recursively copy each leaf scalar local from `src` field
    /// bindings to the matching `dst` slots. Same shape as
    /// `copy_enum_storage` but for struct field trees, used both by
    /// enum-payload struct slots and by potential future struct
    /// reassign paths.
    pub(super) fn copy_struct_fields(&mut self, src: &[FieldBinding], dst: &[FieldBinding]) {
        for (sb, db) in src.iter().zip(dst.iter()) {
            match (&sb.shape, &db.shape) {
                (
                    FieldShape::Scalar { local: sl, ty },
                    FieldShape::Scalar { local: dl, .. },
                ) => {
                    let v = self
                        .emit(InstKind::LoadLocal(*sl), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(InstKind::StoreLocal { dst: *dl, src: v }, None);
                }
                (
                    FieldShape::Struct { fields: sf, .. },
                    FieldShape::Struct { fields: df, .. },
                ) => {
                    let sf = sf.clone();
                    let df = df.clone();
                    self.copy_struct_fields(&sf, &df);
                }
                (
                    FieldShape::Tuple { elements: se, .. },
                    FieldShape::Tuple { elements: de, .. },
                ) => {
                    let se = se.clone();
                    let de = de.clone();
                    self.copy_tuple_elements(&se, &de);
                }
                // JIT-enum-1: an enum field duplicates tag + every
                // variant's payload, the same as copying a whole enum
                // binding.
                (FieldShape::Enum(src_storage), FieldShape::Enum(dst_storage)) => {
                    let src_storage = (**src_storage).clone();
                    let dst_storage = (**dst_storage).clone();
                    self.copy_enum_storage(&src_storage, &dst_storage);
                }
                _ => unreachable!("struct field shape mismatch"),
            }
        }
    }

    /// Lower an expression whose result is a struct value into the
    /// supplied target field bindings (the slot of an enum payload).
    /// Shares `store_struct_value_into_fields` with struct literals'
    /// struct-typed fields, so a payload accepts the same rhs shapes
    /// a field does — literal, existing binding, or struct-returning
    /// call (`Option::Some(String::new())`).
    pub(super) fn lower_into_struct_slot(
        &mut self,
        expr_ref: &ExprRef,
        target_struct_id: StructId,
        target_fields: &[FieldBinding],
    ) -> Result<(), String> {
        self.store_struct_value_into_fields(target_struct_id, target_fields, expr_ref)
            .map_err(|e| format!("enum payload: {e}"))
    }

    /// Element-wise copy between two tuple slot bindings. The shape
    /// match is checked by the caller (we always pair slots from the
    /// same enum-storage tree, so element types and counts agree).
    /// Phase Q2 recurses through compound element shapes so a
    /// `((a, b), c)` value duplicates all leaf scalars.
    pub(super) fn copy_tuple_elements(
        &mut self,
        src: &[TupleElementBinding],
        dst: &[TupleElementBinding],
    ) {
        for (s, d) in src.iter().zip(dst.iter()) {
            match (&s.shape, &d.shape) {
                (
                    TupleElementShape::Scalar { local: sl, ty },
                    TupleElementShape::Scalar { local: dl, .. },
                ) => {
                    let v = self
                        .emit(InstKind::LoadLocal(*sl), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(InstKind::StoreLocal { dst: *dl, src: v }, None);
                }
                (
                    TupleElementShape::Struct { fields: sf, .. },
                    TupleElementShape::Struct { fields: df, .. },
                ) => {
                    let sf = sf.clone();
                    let df = df.clone();
                    self.copy_struct_fields(&sf, &df);
                }
                (
                    TupleElementShape::Tuple { elements: se, .. },
                    TupleElementShape::Tuple { elements: de, .. },
                ) => {
                    let se = se.clone();
                    let de = de.clone();
                    self.copy_tuple_elements(&se, &de);
                }
                _ => unreachable!("tuple element shape mismatch"),
            }
        }
    }

    /// Lower an expression whose result is the value for a single
    /// tuple element, dispatching on the target's `TupleElementShape`.
    /// Scalar elements take a direct lower + StoreLocal; struct /
    /// nested-tuple elements route through the matching slot helper.
    pub(super) fn store_value_into_tuple_element_shape(
        &mut self,
        expr_ref: &ExprRef,
        index: usize,
        shape: &TupleElementShape,
    ) -> Result<(), String> {
        match shape {
            TupleElementShape::Scalar { local, .. } => {
                let v = self.lower_expr(expr_ref)?.ok_or_else(|| {
                    format!("tuple element #{index} produced no value")
                })?;
                self.emit(InstKind::StoreLocal { dst: *local, src: v }, None);
                Ok(())
            }
            TupleElementShape::Struct { struct_id, fields } => {
                self.lower_into_struct_slot(expr_ref, *struct_id, fields)
            }
            TupleElementShape::Tuple { tuple_id, elements } => {
                self.lower_into_tuple_slot(expr_ref, *tuple_id, elements)
            }
        }
    }

    /// Lower an expression whose result is a tuple value into the
    /// supplied target element bindings (the slot of an enum payload).
    /// Shares `store_tuple_value_into_elements` with tuple-typed struct
    /// fields, so a payload accepts the same rhs shapes a field does —
    /// literal, existing binding, tuple-typed field, or tuple-returning
    /// call.
    pub(super) fn lower_into_tuple_slot(
        &mut self,
        expr_ref: &ExprRef,
        target_tuple_id: crate::ir::TupleId,
        target_elements: &[TupleElementBinding],
    ) -> Result<(), String> {
        let _ = target_tuple_id;
        self.store_tuple_value_into_elements(target_elements, expr_ref)
            .map_err(|e| format!("tuple payload: {e}"))
    }

    /// Mirror of `lower_if_chain` for an enum-producing if-chain.
    /// Each branch's body lowers via `lower_into_enum_target` so all
    /// paths converge on the same target locals. There is no separate
    /// merge-block result load — the binding's locals already hold
    /// the merged value once cranelift seals the merge.
    pub(super) fn lower_if_chain_into_compound(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
        target: &CompoundTarget,
    ) -> Result<(), String> {
        let merge = self.fresh_block();
        let mut cond_blocks: Vec<BlockId> = Vec::with_capacity(elif_pairs.len());
        for _ in 0..elif_pairs.len() {
            cond_blocks.push(self.fresh_block());
        }
        let then_blk = self.fresh_block();
        let else_blk = self.fresh_block();

        let c = self
            .lower_expr(cond)?
            .ok_or_else(|| "if condition produced no value".to_string())?;
        let next_after_cond = if !cond_blocks.is_empty() {
            cond_blocks[0]
        } else {
            else_blk
        };
        self.terminate(Terminator::Branch {
            cond: c,
            then_blk,
            else_blk: next_after_cond,
        });

        // then
        self.switch_to(then_blk);
        self.lower_into_compound_target(then_body, target)?;
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(merge));
        }
        // each elif
        for (i, (elif_cond, elif_body)) in elif_pairs.iter().enumerate() {
            let cond_blk = cond_blocks[i];
            self.switch_to(cond_blk);
            let body_blk = self.fresh_block();
            let next = if i + 1 < cond_blocks.len() {
                cond_blocks[i + 1]
            } else {
                else_blk
            };
            let c = self
                .lower_expr(elif_cond)?
                .ok_or_else(|| "elif condition produced no value".to_string())?;
            self.terminate(Terminator::Branch {
                cond: c,
                then_blk: body_blk,
                else_blk: next,
            });
            self.switch_to(body_blk);
            self.lower_into_compound_target(elif_body, target)?;
            if !self.is_unreachable() {
                self.terminate(Terminator::Jump(merge));
            }
        }
        // else
        self.switch_to(else_blk);
        self.lower_into_compound_target(else_body, target)?;
        if !self.is_unreachable() {
            self.terminate(Terminator::Jump(merge));
        }
        self.switch_to(merge);
        Ok(())
    }

    /// Mirror of `lower_match` for an enum-producing match. Uses the
    /// existing pattern-matching helpers but writes each arm's
    /// tail-position enum into the supplied target rather than
    /// merging through a scalar result_local. Restrictions match the
    /// scalar `lower_match`: enum-binding scrutinee with EnumVariant
    /// patterns, scalar scrutinee with literal patterns, and so on.
    pub(super) fn lower_match_into_compound(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
        target: &CompoundTarget,
    ) -> Result<(), String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        let merge = self.fresh_block();
        for arm in arms.iter() {
            let saved_bindings = self.bindings.clone();
            let next_blk = self.fresh_block();
            // Pattern-match dispatch — the same one `lower_match`
            // uses. This used to be a hand-mirrored subset.
            self.dispatch_arm_pattern(&arm.pattern, &scrut, next_blk)?;
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
            self.lower_into_compound_target(&arm.body, target)?;
            if !self.is_unreachable() {
                self.terminate(Terminator::Jump(merge));
            }
            self.bindings = saved_bindings;
            self.switch_to(next_blk);
        }
        // Trailing fallthrough is an exhaustiveness hole — same
        // treatment as scalar `lower_match`: panic so the runtime
        // gets a clear signal if the type-checker missed a case.
        if !self.is_unreachable() {
            let site = self.current_site();
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
                site,
            });
        }
        self.switch_to(merge);
        Ok(())
    }
}
