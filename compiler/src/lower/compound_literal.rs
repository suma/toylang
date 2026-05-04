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
                    // Field type is itself a struct; the rhs must be
                    // a struct literal of the matching shape.
                    let inner_expr = self
                        .program
                        .expression
                        .get(value_ref)
                        .ok_or_else(|| "struct field rhs missing".to_string())?;
                    let inner_literal = match inner_expr {
                        Expr::StructLiteral(_, inner_fs) => inner_fs,
                        other => {
                            return Err(format!(
                                "compiler MVP requires struct field `{}.{}` to be initialised by a struct literal (got {:?})",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str,
                                other
                            ));
                        }
                    };
                    self.store_struct_literal_fields(
                        inner_id,
                        &inner_fields,
                        &inner_literal,
                    )?;
                }
                FieldShape::Tuple { elements: inner_elements, .. } => {
                    // Field type is a tuple; the rhs must be a tuple
                    // literal of the matching length. Element values
                    // store directly into the per-element locals.
                    let inner_expr = self
                        .program
                        .expression
                        .get(value_ref)
                        .ok_or_else(|| "struct field rhs missing".to_string())?;
                    let inner_elems = match inner_expr {
                        Expr::TupleLiteral(es) => es,
                        _ => {
                            return Err(format!(
                                "compiler MVP requires tuple-typed struct field `{}.{}` to be initialised by a tuple literal",
                                self.interner.resolve(outer_base).unwrap_or("?"),
                                field_str
                            ));
                        }
                    };
                    if inner_elems.len() != inner_elements.len() {
                        return Err(format!(
                            "tuple-typed struct field `{}.{}` expects {} elements, got {}",
                            self.interner.resolve(outer_base).unwrap_or("?"),
                            field_str,
                            inner_elements.len(),
                            inner_elems.len(),
                        ));
                    }
                    for (i, e) in inner_elems.iter().enumerate() {
                        let shape = inner_elements[i].shape.clone();
                        self.store_value_into_tuple_element_shape(e, i, &shape)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Allocate a `FieldBinding` tree for a struct, recursively
    /// expanding nested struct fields into their own per-field
    /// locals. Used everywhere a struct binding shape is created
    /// (val rhs of a struct literal, struct param expansion at
    /// function entry, struct-returning call destinations, the
    /// pending-struct-value channel for tail-position struct
    /// literals).
    pub(super) fn allocate_struct_fields(&mut self, struct_id: StructId) -> Vec<FieldBinding> {
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
                Some(Binding::Enum(_)) => None,
                Some(Binding::Array { .. }) => None,
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
