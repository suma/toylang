//! Struct / tuple literal lowering and tuple-access read paths.
//!
//! Allocates per-field / per-element locals up front, evaluates
//! literal values into them, and exposes the helpers used by
//! field-access / assignment paths to walk through nested
//! struct / tuple shapes.
//!
//! - `store_struct_literal_fields`: walk a struct literal's
//!   `(field_sym, value_expr)` list against a `FieldBinding`
//!   tree, storing each value into its matching local. Recurses
//!   on nested struct literals.
//! - `allocate_struct_fields`: build the `FieldBinding` tree
//!   for a `StructId`, walking the field declarations and
//!   allocating per-field locals (or nested compound shapes).
//! - `allocate_tuple_elements`: allocate one `LocalId` per
//!   tuple element (used by tuple literals where every element
//!   is a scalar).
//! - `infer_tuple_element_type`: peek-only inference of one
//!   tuple element's `Type` from its source expression. Used
//!   by `allocate_tuple_element_shape` and by other compound
//!   storage helpers.
//! - `allocate_tuple_element_shape`: full per-element shape
//!   allocation (scalar local / nested struct field tree /
//!   nested tuple element list).
//! - `resolve_tuple_chain_elements`: chase a chain of `Expr::
//!   TupleAccess` nodes down to the deepest tuple binding's
//!   element list (used to assign into / read from a nested
//!   tuple).
//! - `lower_tuple_access`: top-level read of `tup.N`. Mirrors
//!   `lower_field_access` for the tuple case.
//! - `lower_struct_literal_tail` / `lower_tuple_literal_tail`:
//!   tail-position helpers used by `lower_expr` to either bind
//!   into a known target or stash a pending compound value.

use frontend::ast::{Expr, ExprRef};
use string_interner::DefaultSymbol;

use super::bindings::{
    Binding, FieldBinding, FieldChainResult, FieldShape, TupleElementBinding,
    TupleElementShape,
};
use super::types::intern_tuple;
use super::FunctionLower;
use crate::ir::{InstKind, StructId, Type, ValueId};

impl<'a> FunctionLower<'a> {
    /// Walk a struct literal's `(field_sym, value_expr)` list against
    /// a `FieldBinding` tree, evaluating each value and storing it
    /// into the matching local. Recurses on nested struct literals so
    /// `Outer { inner: Inner { x: 1 } }` flows the inner values into
    /// the inner's per-field locals.
    pub(super) fn store_struct_literal_fields(
        &mut self,
        struct_id: StructId,
        field_bindings: &[FieldBinding],
        literal_fields: &[(DefaultSymbol, ExprRef)],
    ) -> Result<(), String> {
        let outer_base = self.module.struct_def(struct_id).base_name;
        for (field_sym, value_ref) in literal_fields {
            let field_str = self
                .interner
                .resolve(*field_sym)
                .ok_or_else(|| "field name missing in interner".to_string())?
                .to_string();
            let fb = field_bindings
                .iter()
                .find(|f| f.name == field_str)
                .ok_or_else(|| {
                    format!(
                        "struct `{}` has no field `{}`",
                        self.interner.resolve(outer_base).unwrap_or("?"),
                        field_str
                    )
                })?
                .clone();
            match fb.shape {
                FieldShape::Scalar { local, .. } => {
                    let v = self
                        .lower_expr(value_ref)?
                        .ok_or_else(|| "struct field rhs produced no value".to_string())?;
                    self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                }
                FieldShape::Struct { struct_id: inner_id, fields: inner_fields } => {
                    // Field type is itself a struct. A nested literal
                    // stores field-by-field; a call / identifier rhs
                    // writes into the same leaf locals (which is the
                    // only way to fill a `String`-typed field, since
                    // `String` is built by associated functions).
                    self.store_struct_value_into_fields(
                        inner_id,
                        &inner_fields,
                        value_ref,
                    )
                    .map_err(|e| {
                        format!(
                            "struct field `{}.{}`: {e}",
                            self.interner.resolve(outer_base).unwrap_or("?"),
                            field_str,
                        )
                    })?;
                }
                FieldShape::Tuple { elements: inner_elements, .. } => {
                    // Field type is a tuple. Same four rhs shapes the
                    // struct case takes, routed through the tuple
                    // counterpart.
                    self.store_tuple_value_into_elements(&inner_elements, value_ref)
                        .map_err(|e| {
                            format!(
                                "tuple-typed struct field `{}.{}`: {e}",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str,
                            )
                        })?;
                }
                // JIT-enum-1: field type is an enum. `lower_into_enum_storage`
                // is the same writer a `val`-bound enum uses, so a field
                // accepts every rhs a binding does — a constructor, another
                // enum binding, an if-chain or a match.
                FieldShape::Enum(storage) => {
                    self.lower_into_enum_storage(value_ref, &storage)
                        .map_err(|e| {
                            format!(
                                "enum-typed struct field `{}.{}`: {e}",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str,
                            )
                        })?;
                }
            }
        }
        Ok(())
    }

    /// Store a struct-typed value into leaf locals that are already
    /// allocated — a struct literal's struct-typed field, or an enum
    /// payload slot.
    ///
    /// Four rhs shapes are accepted. A nested struct literal stores
    /// field-by-field; an identifier deep-copies the source binding's
    /// leaves; a struct-returning call (plain or associated) writes
    /// its multi-value return **straight into the target leaves** via
    /// `CallStruct`, so no temporary binding is needed. The call
    /// shapes are what make `Named { name: String::new() }`
    /// compilable: `String` has no literal form, so a struct with a
    /// `String` field could not be built at all before.
    ///
    /// Note the target leaves are not registered for auto-drop here —
    /// struct *fields* never are, only whole bindings whose own type
    /// implements `Drop`. Registering the call result separately would
    /// drop a value the enclosing struct still owns.
    pub(super) fn store_struct_value_into_fields(
        &mut self,
        target_struct_id: StructId,
        target_fields: &[FieldBinding],
        value_ref: &ExprRef,
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(value_ref)
            .ok_or_else(|| "struct-typed initialiser missing".to_string())?;
        match expr {
            Expr::StructLiteral(name, literal_fields) => {
                let expected = self.module.struct_def(target_struct_id).base_name;
                if name != expected {
                    return Err(format!(
                        "expected a `{}` literal, got `{}`",
                        self.interner.resolve(expected).unwrap_or("?"),
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
                    Some(Binding::Struct { struct_id, fields })
                        if self.struct_shapes_match(struct_id, target_struct_id) =>
                    {
                        fields
                    }
                    _ => {
                        return Err(format!(
                            "`{}` is not a struct binding of the expected type",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.copy_struct_fields(&src_fields, target_fields);
                Ok(())
            }
            // A struct-typed field reached by a chain (`o.i`,
            // `pair.0.inner`). The tuple counterpart below always
            // accepted this shape; the struct one did not, which is
            // what made `Outer { i: o.i, .. }` — and every
            // struct-typed field a struct update fills from its base
            // — a lowering error.
            Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _) => {
                match self.resolve_field_chain(value_ref)? {
                    FieldChainResult::Struct { struct_id, fields }
                        if self.struct_shapes_match(struct_id, target_struct_id) =>
                    {
                        self.copy_struct_fields(&fields, target_fields);
                        Ok(())
                    }
                    FieldChainResult::Struct { struct_id, .. } => Err(format!(
                        "field is a `{}`, but this slot holds a `{}`",
                        self.interner
                            .resolve(self.module.struct_def(struct_id).base_name)
                            .unwrap_or("?"),
                        self.interner
                            .resolve(self.module.struct_def(target_struct_id).base_name)
                            .unwrap_or("?"),
                    )),
                    _ => Err("field is not struct-typed".to_string()),
                }
            }
            Expr::AssociatedFunctionCall(struct_name, fn_name, args)
                if self.struct_defs.contains_key(&struct_name) =>
            {
                // The target's own type args pick the monomorphisation
                // when the callee names the same struct (`Vec::new()`
                // in a `Vec<u8>`-typed slot has nothing else to go on —
                // there is no annotation at a field site).
                let target_def = self.module.struct_def(target_struct_id);
                let self_struct_id = if target_def.base_name == struct_name {
                    target_struct_id
                } else {
                    self.resolve_struct_instance(struct_name, None)?
                };
                // Non-generic impls live in `method_func_ids`;
                // generic ones (`impl<T> Vec<T> { fn new() -> Self }`)
                // are templates that need instantiating against the
                // slot's own type args. Same two-registry lookup
                // `lower_let_struct_associated_call` does for
                // `val v: Vec<u8> = Vec::new()`.
                let func_id = self
                    .resolve_struct_method_func_id(
                        struct_name,
                        fn_name,
                        self_struct_id,
                        &args,
                    )?
                    .ok_or_else(|| {
                        format!(
                            "no associated function `{}::{}` to build this value with",
                            self.interner.resolve(struct_name).unwrap_or("?"),
                            self.interner.resolve(fn_name).unwrap_or("?"),
                        )
                    })?;
                let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
                for a in &args {
                    arg_values.extend(self.lower_arg_values(a)?);
                }
                let extra = self.collect_compound_writeback_dests_slice(&args)?;
                self.emit_struct_call_into_fields(
                    func_id,
                    arg_values,
                    extra,
                    target_struct_id,
                    target_fields,
                )
            }
            Expr::Call(fn_name, args_ref) => {
                let func_id = self
                    .module
                    .lookup_function(None, fn_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown function `{}`",
                            self.interner.resolve(fn_name).unwrap_or("?")
                        )
                    })?;
                let arg_values = self.lower_call_args(&args_ref)?;
                let extra = self.collect_compound_writeback_dests(&args_ref, None)?;
                self.emit_struct_call_into_fields(
                    func_id,
                    arg_values,
                    extra,
                    target_struct_id,
                    target_fields,
                )
            }
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
                let Type::Struct(ret_struct_id) = call.ret else {
                    return Err(format!(
                        "method `{}` returns {}, but this slot holds a struct",
                        self.interner.resolve(method_sym).unwrap_or("?"),
                        crate::spelling::spell_type(self.module, self.interner, call.ret),
                    ));
                };
                if !self.struct_shapes_match(ret_struct_id, target_struct_id) {
                    return Err(format!(
                        "method `{}` returns `{}`, but this slot holds `{}`",
                        self.interner.resolve(method_sym).unwrap_or("?"),
                        self.interner
                            .resolve(self.module.struct_def(ret_struct_id).base_name)
                            .unwrap_or("?"),
                        self.interner
                            .resolve(self.module.struct_def(target_struct_id).base_name)
                            .unwrap_or("?"),
                    ));
                }
                let mut dests: Vec<crate::ir::LocalId> =
                    super::bindings::flatten_struct_locals(target_fields)
                        .into_iter()
                        .map(|(l, _)| l)
                        .collect();
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallStruct {
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
                "compiler MVP cannot build a struct-typed value from {} — use a struct literal, an existing binding, or a struct-returning function / associated-function / method call",
                crate::spelling::describe_expr(self.interner, &other)
            )),
        }
    }

    /// Tuple counterpart to [`Self::store_struct_value_into_fields`],
    /// with the same four rhs shapes: a tuple literal, an existing
    /// tuple binding, a tuple-typed field / element, and a
    /// tuple-returning call (plain, associated, or method).
    pub(super) fn store_tuple_value_into_elements(
        &mut self,
        target_elements: &[TupleElementBinding],
        value_ref: &ExprRef,
    ) -> Result<(), String> {
        let expr = self
            .program
            .expression
            .get(value_ref)
            .ok_or_else(|| "tuple-typed initialiser missing".to_string())?;
        match expr {
            Expr::TupleLiteral(elems) => {
                if elems.len() != target_elements.len() {
                    return Err(format!(
                        "expects {} element(s), got {}",
                        target_elements.len(),
                        elems.len(),
                    ));
                }
                for (i, e) in elems.iter().enumerate() {
                    let shape = target_elements[i].shape.clone();
                    self.store_value_into_tuple_element_shape(e, i, &shape)?;
                }
                Ok(())
            }
            Expr::Identifier(sym) => {
                let src = match self.bindings.get(&sym).cloned() {
                    Some(Binding::Tuple { elements }) => elements,
                    _ => {
                        return Err(format!(
                            "`{}` is not a tuple binding",
                            self.interner.resolve(sym).unwrap_or("?")
                        ));
                    }
                };
                self.copy_tuple_elements_checked(&src, target_elements)
            }
            Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _) => {
                match self.resolve_field_chain(value_ref)? {
                    FieldChainResult::Tuple { elements } => {
                        self.copy_tuple_elements_checked(&elements, target_elements)
                    }
                    _ => Err("field is not tuple-typed".to_string()),
                }
            }
            Expr::AssociatedFunctionCall(struct_name, fn_name, args)
                if self.struct_defs.contains_key(&struct_name) =>
            {
                let struct_id = self.resolve_struct_instance(struct_name, None)?;
                let func_id = self
                    .resolve_struct_method_func_id(struct_name, fn_name, struct_id, &args)?
                    .ok_or_else(|| {
                        format!(
                            "no associated function `{}::{}` to build this value with",
                            self.interner.resolve(struct_name).unwrap_or("?"),
                            self.interner.resolve(fn_name).unwrap_or("?"),
                        )
                    })?;
                let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
                for a in &args {
                    arg_values.extend(self.lower_arg_values(a)?);
                }
                let extra = self.collect_compound_writeback_dests_slice(&args)?;
                self.emit_tuple_call_into_elements(
                    func_id,
                    arg_values,
                    extra,
                    target_elements,
                )
            }
            Expr::Call(fn_name, args_ref) => {
                let func_id = self
                    .module
                    .lookup_function(None, fn_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown function `{}`",
                            self.interner.resolve(fn_name).unwrap_or("?")
                        )
                    })?;
                let arg_values = self.lower_call_args(&args_ref)?;
                let extra = self.collect_compound_writeback_dests(&args_ref, None)?;
                self.emit_tuple_call_into_elements(
                    func_id,
                    arg_values,
                    extra,
                    target_elements,
                )
            }
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
                if !matches!(call.ret, Type::Tuple(_)) {
                    return Err(format!(
                        "method `{}` returns {}, but this slot holds a tuple",
                        self.interner.resolve(method_sym).unwrap_or("?"),
                        crate::spelling::spell_type(self.module, self.interner, call.ret),
                    ));
                }
                let mut dests: Vec<crate::ir::LocalId> =
                    super::bindings::flatten_tuple_element_locals(target_elements)
                        .into_iter()
                        .map(|(l, _)| l)
                        .collect();
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallTuple {
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
                "compiler MVP cannot build a tuple-typed value from {} — use a tuple literal, an existing binding, or a tuple-returning function / associated-function / method call",
                crate::spelling::describe_expr(self.interner, &other)
            )),
        }
    }

    /// `copy_tuple_elements` with the shape check its callers outside
    /// the enum-storage tree need. Inside that tree both sides come
    /// from the same allocation walk and always agree; here the source
    /// is whatever the user named, and a mismatch would otherwise trip
    /// the `unreachable!` in the copy.
    fn copy_tuple_elements_checked(
        &mut self,
        src: &[TupleElementBinding],
        dst: &[TupleElementBinding],
    ) -> Result<(), String> {
        if src.len() != dst.len() {
            return Err(format!(
                "expects {} element(s), got {}",
                dst.len(),
                src.len()
            ));
        }
        for (s, d) in src.iter().zip(dst.iter()) {
            let same = matches!(
                (&s.shape, &d.shape),
                (TupleElementShape::Scalar { .. }, TupleElementShape::Scalar { .. })
                    | (TupleElementShape::Struct { .. }, TupleElementShape::Struct { .. })
                    | (TupleElementShape::Tuple { .. }, TupleElementShape::Tuple { .. })
            );
            if !same {
                return Err(format!("element #{} has a different shape", s.index));
            }
        }
        self.copy_tuple_elements(src, dst);
        Ok(())
    }

    /// Emit `CallTuple` for a tuple-returning callee whose result
    /// lands directly in `target_elements`' leaf locals. Tuple counterpart
    /// to [`Self::emit_struct_call_into_fields`].
    fn emit_tuple_call_into_elements(
        &mut self,
        func_id: crate::ir::FuncId,
        args: Vec<ValueId>,
        extra_dests: Vec<crate::ir::LocalId>,
        target_elements: &[TupleElementBinding],
    ) -> Result<(), String> {
        let ret = self.module.function(func_id).return_type;
        let Type::Tuple(_) = ret else {
            return Err(format!(
                "callee does not return a tuple (got {})",
                crate::spelling::spell_type(self.module, self.interner, ret)
            ));
        };
        let mut dests: Vec<crate::ir::LocalId> =
            super::bindings::flatten_tuple_element_locals(target_elements)
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        dests.extend(extra_dests);
        self.emit(
            InstKind::CallTuple {
                target: func_id,
                args,
                dests,
            },
            None,
        );
        Ok(())
    }

    /// Emit `CallStruct` for a struct-returning callee whose result
    /// lands directly in `target_fields`' leaf locals. `extra_dests`
    /// carries the compound `&mut` writeback slots the callee declares
    /// (empty for the constructor shapes this path usually sees), in
    /// the same order `lower_let_call_struct` appends them.
    fn emit_struct_call_into_fields(
        &mut self,
        func_id: crate::ir::FuncId,
        args: Vec<ValueId>,
        extra_dests: Vec<crate::ir::LocalId>,
        target_struct_id: StructId,
        target_fields: &[FieldBinding],
    ) -> Result<(), String> {
        let ret = self.module.function(func_id).return_type;
        let Type::Struct(ret_struct_id) = ret else {
            return Err(format!(
                "callee does not return a struct (got {})",
                crate::spelling::spell_type(self.module, self.interner, ret)
            ));
        };
        if !self.struct_shapes_match(ret_struct_id, target_struct_id) {
            return Err(format!(
                "callee returns `{}`, but this slot holds `{}`",
                self.interner
                    .resolve(self.module.struct_def(ret_struct_id).base_name)
                    .unwrap_or("?"),
                self.interner
                    .resolve(self.module.struct_def(target_struct_id).base_name)
                    .unwrap_or("?"),
            ));
        }
        // Leaf order is declaration order on both sides (both trees
        // come from `allocate_struct_fields` over the same shape), so
        // the multi-result call lands field-for-field.
        let mut dests: Vec<crate::ir::LocalId> =
            super::bindings::flatten_struct_locals(target_fields)
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        dests.extend(extra_dests);
        self.emit(
            InstKind::CallStruct {
                target: func_id,
                args,
                dests,
            },
            None,
        );
        Ok(())
    }

    /// Two `StructId`s denote the same type when they are the same
    /// instance, or when they agree on base name *and* type args —
    /// separate lowering paths can intern the same monomorphisation
    /// twice.
    fn struct_shapes_match(&self, a: StructId, b: StructId) -> bool {
        if a == b {
            return true;
        }
        let da = self.module.struct_def(a);
        let db = self.module.struct_def(b);
        da.base_name == db.base_name && da.type_args == db.type_args
    }

    /// Allocate a `FieldBinding` tree for a struct, recursively
    /// expanding nested struct fields into their own per-field
    /// locals. Used everywhere a struct binding shape is created
    /// (val rhs of a struct literal, struct param expansion at
    /// function entry, struct-returning call destinations, the
    /// pending-struct-value channel for tail-position struct
    /// literals).
    pub(super) fn allocate_struct_fields(&mut self, struct_id: StructId) -> Vec<FieldBinding> {
        let out = self.allocate_struct_fields_inner(struct_id);
        // CODE-SIZE-SELF-ABI S3b: only the outermost allocation is a
        // binding of its own -- the recursive ones are its fields, and
        // they live inside the same slot.
        if self.struct_alloc_depth == 0 {
            self.make_resident_if_wide(struct_id, &out);
        }
        out
    }

    /// CODE-SIZE-SELF-ABI S3b: give a wide local compound its own stack
    /// slot, so it is passed by address rather than copied.
    ///
    /// Declines for anything that would be wrong or pointless:
    ///
    /// * **parameters**, which either arrive as leaves the entry block
    ///   defines or already come in as an address -- either way the
    ///   caller owns the storage;
    /// * narrow structs, whose leaves ride in registers, where a slot
    ///   would turn free reads into loads;
    /// * a struct with no byte layout to describe.
    fn make_resident_if_wide(&mut self, struct_id: StructId, fields: &[FieldBinding]) {
        if self.binding_params {
            return;
        }
        let leaf_locals = super::bindings::flatten_struct_locals(fields);
        if leaf_locals.len() <= crate::program::PTR_SELF_LEAF_THRESHOLD {
            return;
        }
        let Some(layout) = crate::program::struct_leaf_layout(self.module, Type::Struct(struct_id))
        else {
            return;
        };
        if layout.len() != leaf_locals.len() {
            return;
        }
        let bytes: u64 = layout
            .last()
            .map(|(off, ty)| off + crate::program::scalar_byte_size(*ty).unwrap_or(8))
            .unwrap_or(0);
        let slot_idx = {
            let func = self.module.function_mut(self.func_id);
            let idx = func.dyn_coerce_slots.len() as u32;
            func.dyn_coerce_slots.push(bytes.max(1) as u32);
            idx
        };
        let leaves: Vec<(crate::ir::LocalId, u64, Type)> = leaf_locals
            .iter()
            .zip(layout.iter())
            .map(|((local, ty), (offset, _))| (*local, *offset, *ty))
            .collect();
        self.module
            .function_mut(self.func_id)
            .resident_compounds
            .push(crate::ir::ResidentCompound { slot_idx, leaves });
    }

    fn allocate_struct_fields_inner(&mut self, struct_id: StructId) -> Vec<FieldBinding> {
        self.struct_alloc_depth += 1;
        let out = self.allocate_struct_fields_body(struct_id);
        self.struct_alloc_depth -= 1;
        out
    }

    fn allocate_struct_fields_body(&mut self, struct_id: StructId) -> Vec<FieldBinding> {
        let def = self.module.struct_def(struct_id).clone();
        let mut out: Vec<FieldBinding> = Vec::with_capacity(def.fields.len());
        for (field_name, field_ty) in &def.fields {
            let shape = match *field_ty {
                Type::Struct(inner) => {
                    let sub = self.allocate_struct_fields(inner);
                    FieldShape::Struct {
                        struct_id: inner,
                        fields: sub,
                    }
                }
                Type::Tuple(tuple_id) => {
                    // Tuple defs are interned at struct-template
                    // lowering time, so this should always succeed
                    // — fall back to an empty list defensively.
                    let elements = self
                        .allocate_tuple_elements(tuple_id)
                        .unwrap_or_default();
                    FieldShape::Tuple { tuple_id, elements }
                }
                // JIT-enum-1: an enum field gets the same storage a
                // whole enum binding does. Without this arm it fell
                // into the scalar branch below and became one local,
                // which is where "struct field rhs produced no value"
                // came from.
                Type::Enum(enum_id) => {
                    FieldShape::Enum(Box::new(self.allocate_enum_storage(enum_id)))
                }
                scalar => {
                    let local = self.module.function_mut(self.func_id).add_local(scalar);
                    FieldShape::Scalar { local, ty: scalar }
                }
            };
            out.push(FieldBinding {
                name: field_name.clone(),
                shape,
            });
        }
        out
    }

    /// Tuple counterpart to `allocate_struct_fields`. Allocates one
    /// local per tuple element and returns the matching binding list
    /// in declaration order. Phase Q2 allows nested compound elements
    /// (tuple-of-tuple, tuple-of-struct) by recursing through the
    /// `TupleElementShape` tree the same way `allocate_struct_fields`
    /// does for `FieldShape`.
    pub(super) fn allocate_tuple_elements(
        &mut self,
        tuple_id: crate::ir::TupleId,
    ) -> Result<Vec<TupleElementBinding>, String> {
        let elements = self
            .module
            .tuple_defs
            .get(tuple_id.0 as usize)
            .cloned()
            // DIAG-DEBUG-FMT-OK: an internal invariant, not a
            // diagnostic — a `TupleId` with no def means the module is
            // inconsistent, and the raw id is what a debugger wants.
            .ok_or_else(|| format!("internal error: missing tuple def for {tuple_id:?}"))?;
        let mut out: Vec<TupleElementBinding> = Vec::with_capacity(elements.len());
        for (i, ty) in elements.iter().enumerate() {
            let shape = self.allocate_tuple_element_shape(*ty)?;
            out.push(TupleElementBinding { index: i, shape });
        }
        Ok(out)
    }

    /// Determine the static `Type` of a tuple element expression,
    /// interning any new tuple shapes encountered. Falls back to
    /// `value_scalar` for the scalar / identifier paths and recurses
    /// for `TupleLiteral` / `StructLiteral` so a nested literal like
    /// `((1, 2), 3)` resolves all the way down. Returns `None` if
    /// the element shape can't be resolved (forces the caller to
    /// emit a clear error).
    pub(super) fn infer_tuple_element_type(&mut self, expr_ref: &ExprRef) -> Option<Type> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            Expr::TupleLiteral(elems) => {
                let mut element_tys: Vec<Type> = Vec::with_capacity(elems.len());
                for e in &elems {
                    element_tys.push(self.infer_tuple_element_type(e)?);
                }
                let id = intern_tuple(self.module, element_tys);
                Some(Type::Tuple(id))
            }
            Expr::StructLiteral(name, _) => {
                let id = self.resolve_struct_instance(name, None).ok()?;
                Some(Type::Struct(id))
            }
            // DATA-ORIENTED Phase 3: enum elements. A unit variant
            // arrives as `Enum::Variant` and a tuple variant as a
            // call, and either names the enum in its first path
            // segment — which is all an *element type* needs.
            // Generic enums are left out: their type arguments come
            // from the payload, and an array literal has no
            // annotation to reconcile that against.
            Expr::QualifiedIdentifier(path)
                if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
            {
                self.resolve_enum_instance(path[0], None).ok().map(Type::Enum)
            }
            Expr::AssociatedFunctionCall(base, _, _) if self.enum_defs.contains_key(&base) => {
                self.resolve_enum_instance(base, None).ok().map(Type::Enum)
            }
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(Binding::RefScalar { pointee_ty, .. }) => Some(*pointee_ty),
                Some(Binding::Struct { struct_id, .. }) => Some(Type::Struct(*struct_id)),
                Some(Binding::Tuple { elements }) => {
                    let element_tys: Vec<Type> = elements
                        .iter()
                        .map(|e| match &e.shape {
                            TupleElementShape::Scalar { ty, .. } => *ty,
                            TupleElementShape::Struct { struct_id, .. } => {
                                Type::Struct(*struct_id)
                            }
                            TupleElementShape::Tuple { tuple_id, .. } => {
                                Type::Tuple(*tuple_id)
                            }
                        })
                        .collect();
                    let id = intern_tuple(self.module, element_tys);
                    Some(Type::Tuple(id))
                }
                Some(Binding::Enum(storage)) => Some(Type::Enum(storage.enum_id)),
                Some(Binding::Array { .. }) => None,
                Some(Binding::FunctionPtr { .. }) => Some(Type::U64),
                // A5-P2: dyn-trait identifiers don't have a single
                // scalar IR type — they're a 2-tuple of U64 leaves.
                // value_scalar() only fires for scalar carriers, so
                // returning None makes the caller fall through to a
                // path that reports the unsupported usage cleanly.
                Some(Binding::DynTraitObj { .. }) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            _ => self.value_scalar(expr_ref),
        }
    }

    pub(super) fn allocate_tuple_element_shape(
        &mut self,
        ty: Type,
    ) -> Result<TupleElementShape, String> {
        match ty {
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                Ok(TupleElementShape::Struct { struct_id, fields })
            }
            Type::Tuple(inner_id) => {
                let elements = self.allocate_tuple_elements(inner_id)?;
                Ok(TupleElementShape::Tuple {
                    tuple_id: inner_id,
                    elements,
                })
            }
            scalar => {
                let local = self.module.function_mut(self.func_id).add_local(scalar);
                Ok(TupleElementShape::Scalar { local, ty: scalar })
            }
        }
    }

    /// Read `t.N` where `t` resolves to a tuple binding. Like field
    /// access on a struct, the obj must be a bare identifier so the
    /// lookup is purely static.
    /// Walk a (possibly nested) tuple-access chain rooted at an
    /// identifier or struct field-access, returning the matched
    /// tuple element list at the deepest step. Used by
    /// `lower_tuple_access`'s `Expr::TupleAccess` arm to resolve
    /// `t.0.1` style access where the inner step also lands on a
    /// tuple shape.
    pub(super) fn resolve_tuple_chain_elements(
        &self,
        obj: &ExprRef,
    ) -> Result<Vec<TupleElementBinding>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        match obj_expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Tuple { elements }) => Ok(elements.clone()),
                _ => Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(obj)? {
                FieldChainResult::Tuple { elements } => Ok(elements),
                _ => Err("tuple chain expects a tuple-typed step".to_string()),
            },
            Expr::TupleAccess(inner, idx) => {
                let inner_elements = self.resolve_tuple_chain_elements(&inner)?;
                let elem = inner_elements
                    .iter()
                    .find(|e| e.index == idx)
                    .ok_or_else(|| format!("tuple has no element at index {idx}"))?;
                match &elem.shape {
                    TupleElementShape::Tuple { elements, .. } => Ok(elements.clone()),
                    _ => Err("inner tuple element is not a tuple".to_string()),
                }
            }
            _ => Err(
                "compiler MVP only supports tuple chains on identifiers, struct fields, or nested tuple elements".to_string(),
            ),
        }
    }

    pub(super) fn lower_tuple_access(
        &mut self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<Option<ValueId>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        // Three shapes are accepted: (1) a bare identifier bound to
        // a tuple; (2) a field-access chain whose final step lands
        // on a tuple-typed struct field (`outer.inner.0` style);
        // (3) another tuple access whose result is itself a tuple
        // (`t.0.1` for nested tuples).
        let elements = match obj_expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Tuple { elements }) => elements,
                Some(_) => {
                    return Err(format!(
                        "`{}` is not a tuple value",
                        self.interner.resolve(sym).unwrap_or("?")
                    ));
                }
                None => {
                    return Err(format!(
                        "undefined identifier `{}`",
                        self.interner.resolve(sym).unwrap_or("?")
                    ));
                }
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(obj)? {
                FieldChainResult::Tuple { elements } => elements,
                FieldChainResult::Struct { .. } => {
                    return Err(
                        "tuple access on a struct-typed field — try a field name instead of an index"
                            .to_string(),
                    );
                }
                FieldChainResult::Scalar { .. } => {
                    return Err("tuple access on a scalar field".to_string());
                }
                FieldChainResult::Enum(_) => {
                    return Err(
                        "tuple access on an enum-typed field — match on it instead"
                            .to_string(),
                    );
                }
            },
            Expr::TupleAccess(inner_obj, inner_index) => {
                // Recurse to resolve the inner tuple-access result;
                // it must itself be a tuple sub-binding for indexing
                // to make sense. We pre-walk via the same elements
                // chain as lower_tuple_access does for identifiers.
                let inner_elements = self.resolve_tuple_chain_elements(&inner_obj)?;
                match inner_elements
                    .iter()
                    .find(|e| e.index == inner_index)
                    .map(|e| e.shape.clone())
                {
                    Some(TupleElementShape::Tuple { elements: inner, .. }) => inner,
                    Some(TupleElementShape::Struct { .. }) => {
                        return Err(
                            "tuple access on a struct element — use a field name instead"
                                .to_string(),
                        );
                    }
                    Some(TupleElementShape::Scalar { .. }) => {
                        return Err("tuple access on a scalar element".to_string());
                    }
                    None => {
                        return Err(format!("tuple has no element at index {inner_index}"));
                    }
                }
            }
            _ => {
                return Err(
                    "compiler MVP only supports tuple access on a bare identifier, a struct field-access chain, or a nested tuple element".to_string(),
                );
            }
        };
        let elem = elements.iter().find(|e| e.index == index).ok_or_else(|| {
            format!("tuple has no element at index {index}")
        })?;
        match &elem.shape {
            TupleElementShape::Scalar { local, ty } => {
                Ok(self.emit(InstKind::LoadLocal(*local), Some(*ty)))
            }
            TupleElementShape::Struct { fields, .. } => {
                self.pending_struct_value = Some(fields.clone());
                self.pending_tuple_value = None;
                Ok(None)
            }
            TupleElementShape::Tuple { elements: inner, .. } => {
                self.pending_tuple_value = Some(inner.clone());
                self.pending_struct_value = None;
                Ok(None)
            }
        }
    }

    /// Lower a struct literal in expression position. The result
    /// becomes the function's pending struct value; the implicit
    /// return path picks it up. Non-return uses (e.g. `val p = ...`)
    /// hit `lower_let` first and never reach here.
    pub(super) fn lower_struct_literal_tail(
        &mut self,
        struct_name: DefaultSymbol,
        fields: Vec<(DefaultSymbol, ExprRef)>,
    ) -> Result<Option<ValueId>, String> {
        // The function's return type tells us which monomorphisation
        // to use; for non-generic structs the annotation isn't
        // needed (instantiate with no args).
        let ret_ty = self.module.function(self.func_id).return_type;
        let struct_id = if let Type::Struct(id) = ret_ty {
            // Verify the literal's name matches the return enum.
            if self.module.struct_def(id).base_name != struct_name {
                return Err(format!(
                    "tail-position struct literal `{}` does not match function return type `{}`",
                    self.interner.resolve(struct_name).unwrap_or("?"),
                    self.interner.resolve(self.module.struct_def(id).base_name).unwrap_or("?"),
                ));
            }
            id
        } else {
            // Fall back to non-generic instantiation.
            self.resolve_struct_instance(struct_name, None)?
        };
        let field_bindings = self.allocate_struct_fields(struct_id);
        self.store_struct_literal_fields(struct_id, &field_bindings, &fields)?;
        self.pending_struct_value = Some(field_bindings);
        Ok(None)
    }

    /// Tuple-literal counterpart to `lower_struct_literal_tail`.
    /// Allocates one local per element (inferring the element's
    /// scalar type from the rhs expression), stores each value, and
    /// stashes the element list as the pending tuple value.
    pub(super) fn lower_tuple_literal_tail(
        &mut self,
        elems: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        let mut element_bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
        for (i, e) in elems.iter().enumerate() {
            let ty = self
                .value_scalar(e)
                .ok_or_else(|| format!("tuple element #{i} has no inferable type"))?;
            let shape = self.allocate_tuple_element_shape(ty)?;
            element_bindings.push(TupleElementBinding { index: i, shape });
        }
        for (i, e) in elems.iter().enumerate() {
            let shape = element_bindings[i].shape.clone();
            self.store_value_into_tuple_element_shape(e, i, &shape)?;
        }
        self.pending_tuple_value = Some(element_bindings);
        Ok(None)
    }
}
