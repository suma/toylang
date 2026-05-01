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

use frontend::ast::{Expr, ExprRef, MatchArm, Pattern, Stmt};
use string_interner::DefaultSymbol;

use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, EnumStorage, FieldBinding,
    FieldShape, MatchScrutinee, PayloadSlot, TupleElementBinding, TupleElementShape,
};
use super::FunctionLower;
use crate::ir::{
    BlockId, Const, EnumId, InstKind, LocalId, StructId, Terminator, Type, ValueId,
};

impl<'a> FunctionLower<'a> {
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
                }
            }
        }
    }

    /// Flatten an EnumStorage into the dest list for `CallEnum`
    /// (tag first, then each variant's payloads in declaration
    /// order, recursing through nested enums).
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
    pub(super) fn detect_enum_result(&self, expr_ref: &ExprRef) -> Option<DefaultSymbol> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::QualifiedIdentifier(path)
                if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
            {
                Some(path[0])
            }
            Expr::AssociatedFunctionCall(en, _, _) if self.enum_defs.contains_key(&en) => {
                Some(en)
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Enum(storage)) => {
                    Some(self.module.enum_def(storage.enum_id).base_name)
                }
                _ => None,
            },
            Expr::IfElifElse(_, then_body, elif_pairs, else_body) => {
                let then_en = self.detect_enum_result(&then_body)?;
                for (_, body) in &elif_pairs {
                    if self.detect_enum_result(body)? != then_en {
                        return None;
                    }
                }
                if self.detect_enum_result(&else_body)? != then_en {
                    return None;
                }
                Some(then_en)
            }
            Expr::Match(_, arms) => {
                let first_en = arms.iter().find_map(|a| self.detect_enum_result(&a.body))?;
                for arm in &arms {
                    if self.detect_enum_result(&arm.body)? != first_en {
                        return None;
                    }
                }
                Some(first_en)
            }
            Expr::Block(stmts) => {
                let last = stmts.last()?;
                let stmt = self.program.statement.get(last)?;
                if let Stmt::Expression(e) = stmt {
                    self.detect_enum_result(&e)
                } else {
                    None
                }
            }
            _ => None,
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
                if en != expected_base {
                    return Err(format!(
                        "branch produces enum `{}` but the surrounding binding expects `{}`",
                        self.interner.resolve(en).unwrap_or("?"),
                        self.interner.resolve(expected_base).unwrap_or("?"),
                    ));
                }
                let enum_def = self.module.enum_def(target_enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == var)
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(expected_base).unwrap_or("?"),
                            self.interner.resolve(var).unwrap_or("?"),
                        )
                    })?;
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
                self.write_variant_into_storage(target, variant_idx, &args)?;
                Ok(())
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
                let stmts = stmts.clone();
                if stmts.is_empty() {
                    return Err("empty block cannot produce an enum value".to_string());
                }
                for (i, stmt_ref) in stmts.iter().enumerate() {
                    let is_last = i + 1 == stmts.len();
                    let stmt = self
                        .program
                        .statement
                        .get(stmt_ref)
                        .ok_or_else(|| "missing block stmt".to_string())?;
                    if is_last {
                        if let Stmt::Expression(e) = stmt {
                            return self.lower_into_enum_storage(&e, target);
                        }
                    }
                    let _ = self.lower_stmt(stmt_ref)?;
                }
                Err("block has no enum-producing tail expression".to_string())
            }
            Expr::IfElifElse(cond, then_body, elif_pairs, else_body) => self
                .lower_if_chain_into_enum(&cond, &then_body, &elif_pairs, &else_body, target),
            Expr::Match(scrutinee, arms) => {
                self.lower_match_into_enum(&scrutinee, &arms, target)
            }
            other => Err(format!(
                "compiler MVP cannot lower `{:?}` as an enum-producing expression in this position",
                other
            )),
        }
    }

    /// Common store: write the variant tag and (optionally) evaluate
    /// + store the payload args into the target's per-variant slots.
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
                _ => unreachable!("struct field shape mismatch"),
            }
        }
    }

    /// Lower an expression whose result is a struct value into the
    /// supplied target field bindings (the slot of an enum payload).
    /// Accepts the same RHS shapes that `lower_let`'s
    /// `Expr::StructLiteral` branch does, plus a bare identifier
    /// referring to an existing struct binding (deep-copied via
    /// `copy_struct_fields`).
    pub(super) fn lower_into_struct_slot(
        &mut self,
        expr_ref: &ExprRef,
        target_struct_id: StructId,
        target_fields: &[FieldBinding],
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "struct-target expression missing".to_string())?;
        match expr {
            Expr::StructLiteral(name, literal_fields) => {
                let expected_base = self.module.struct_def(target_struct_id).base_name;
                if name != expected_base {
                    return Err(format!(
                        "struct payload expects `{}`, got `{}` literal",
                        self.interner.resolve(expected_base).unwrap_or("?"),
                        self.interner.resolve(name).unwrap_or("?"),
                    ));
                }
                self.store_struct_literal_fields(
                    target_struct_id,
                    target_fields,
                    &literal_fields,
                )
            }
            Expr::Identifier(sym) => {
                let src_fields = match self.bindings.get(&sym).cloned() {
                    Some(Binding::Struct {
                        struct_id: src_id,
                        fields,
                    }) if src_id == target_struct_id => fields,
                    _ => {
                        return Err(format!(
                            "`{}` is not a struct binding of the expected payload type",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.copy_struct_fields(&src_fields, target_fields);
                Ok(())
            }
            other => Err(format!(
                "compiler MVP cannot lower `{:?}` as a struct-typed enum payload",
                other
            )),
        }
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
    /// Accepts a tuple literal of the matching shape, or a bare
    /// identifier referring to an existing tuple binding (deep-copied
    /// via `copy_tuple_elements`).
    pub(super) fn lower_into_tuple_slot(
        &mut self,
        expr_ref: &ExprRef,
        target_tuple_id: crate::ir::TupleId,
        target_elements: &[TupleElementBinding],
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "tuple-target expression missing".to_string())?;
        match expr {
            Expr::TupleLiteral(elems) => {
                if elems.len() != target_elements.len() {
                    return Err(format!(
                        "tuple payload expects {} elements, got {}",
                        target_elements.len(),
                        elems.len()
                    ));
                }
                for (i, e) in elems.iter().enumerate() {
                    let shape = target_elements[i].shape.clone();
                    self.store_value_into_tuple_element_shape(e, i, &shape)?;
                }
                let _ = target_tuple_id;
                Ok(())
            }
            Expr::Identifier(sym) => {
                let src_elements = match self.bindings.get(&sym).cloned() {
                    Some(Binding::Tuple { elements }) => elements,
                    _ => {
                        return Err(format!(
                            "`{}` is not a tuple binding of the expected payload type",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                if src_elements.len() != target_elements.len() {
                    return Err(format!(
                        "tuple payload shape mismatch: expected {} elements, got {}",
                        target_elements.len(),
                        src_elements.len()
                    ));
                }
                self.copy_tuple_elements(&src_elements, target_elements);
                Ok(())
            }
            other => Err(format!(
                "compiler MVP cannot lower `{:?}` as a tuple-typed enum payload",
                other
            )),
        }
    }

    /// Mirror of `lower_if_chain` for an enum-producing if-chain.
    /// Each branch's body lowers via `lower_into_enum_target` so all
    /// paths converge on the same target locals. There is no separate
    /// merge-block result load — the binding's locals already hold
    /// the merged value once cranelift seals the merge.
    pub(super) fn lower_if_chain_into_enum(
        &mut self,
        cond: &ExprRef,
        then_body: &ExprRef,
        elif_pairs: &Vec<(ExprRef, ExprRef)>,
        else_body: &ExprRef,
        target: &EnumStorage,
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
        self.lower_into_enum_storage(then_body, target)?;
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
            self.lower_into_enum_storage(elif_body, target)?;
            if !self.is_unreachable() {
                self.terminate(Terminator::Jump(merge));
            }
        }
        // else
        self.switch_to(else_blk);
        self.lower_into_enum_storage(else_body, target)?;
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
    pub(super) fn lower_match_into_enum(
        &mut self,
        scrutinee: &ExprRef,
        arms: &Vec<MatchArm>,
        target: &EnumStorage,
    ) -> Result<(), String> {
        let scrut = self.classify_match_scrutinee(scrutinee)?;
        let merge = self.fresh_block();
        for arm in arms.iter() {
            let saved_bindings = self.bindings.clone();
            let next_blk = self.fresh_block();
            // Pattern-match dispatch — same shape as lower_match's
            // first phase. We can't easily share code without a
            // bigger refactor, so we mirror it here for clarity.
            match &arm.pattern {
                Pattern::Wildcard => {}
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
            self.lower_into_enum_storage(&arm.body, target)?;
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
            self.terminate(Terminator::Panic {
                message: self.contract_msgs.requires_violation,
            });
        }
        self.switch_to(merge);
        Ok(())
    }
}
