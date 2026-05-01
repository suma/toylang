//! Field-access read paths.
//!
//! Lowers `obj.field` and `obj.0` style reads from struct /
//! tuple bindings, plus the chain-resolution helpers used by
//! both reads and writes.
//!
//! - `lower_field_access`: top-level read of `obj.field`.
//!   Resolves the leftmost binding via `resolve_field_chain`,
//!   then either emits a scalar load or stashes a pending
//!   struct value for tail-position chained struct returns.
//! - `resolve_field_chain`: walks an `Expr::FieldAccess` /
//!   `Expr::TupleAccess` chain and returns a
//!   `FieldChainResult` (final shape + chain of nested
//!   field / tuple steps).
//! - `resolve_tuple_element_local`: from a tuple binding +
//!   index, descends into the matching `TupleElementShape`
//!   (handles nested struct / tuple element shapes too).
//! - `resolve_field_local`: from a struct field shape +
//!   field name, descends into the matching scalar local or
//!   nested compound shape.

use frontend::ast::{Expr, ExprRef};
use string_interner::DefaultSymbol;

use super::bindings::{Binding, FieldChainResult, FieldShape, TupleElementShape};
use super::FunctionLower;
use crate::ir::{InstKind, LocalId, ValueId};

impl<'a> FunctionLower<'a> {
    /// Read `obj.field` where `obj` resolves to either a struct
    /// binding directly (`p.x`) or another field access (`a.b.c`).
    /// Walks the chain through nested struct fields and returns
    /// either a scalar load or stashes a pending struct value (for
    /// tail-position chained struct returns).
    pub(super) fn lower_field_access(
        &mut self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<Option<ValueId>, String> {
        // Resolve the obj sub-expression to a `FieldChainResult`
        // first; it must be a struct (we're stepping into one of its
        // fields). Then look up `field` in that struct's bindings.
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } | FieldChainResult::Tuple { .. } => {
                return Err("field access on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, ty } => {
                self.pending_struct_value = None;
                Ok(self.emit(InstKind::LoadLocal(*local), Some(*ty)))
            }
            FieldShape::Struct { fields, .. } => {
                // Mid-chain struct value — stash for tail-position
                // implicit return, returning no SSA value because
                // the IR keeps struct values out of the value graph.
                self.pending_struct_value = Some(fields.clone());
                Ok(None)
            }
            FieldShape::Tuple { elements, .. } => {
                // Same idea for a tuple-typed struct field — stash
                // the element list as the pending tuple value so a
                // tail-position `outer.inner` chain reaches the
                // implicit-return path.
                self.pending_struct_value = None;
                self.pending_tuple_value = Some(elements.clone());
                Ok(None)
            }
        }
    }

    /// Helper that walks a (possibly nested) field-access chain and
    /// returns either the leaf scalar (LocalId + Type) or the inner
    /// `FieldBinding` list of a struct sub-binding. Pure / immutable
    /// — used by both reads and writes.
    pub(super) fn resolve_field_chain(&self, expr_ref: &ExprRef) -> Result<FieldChainResult, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "field-chain expression missing".to_string())?;
        match expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { local, ty }) => Ok(FieldChainResult::Scalar {
                    local: *local,
                    ty: *ty,
                }),
                Some(Binding::Struct { fields, .. }) => Ok(FieldChainResult::Struct {
                    fields: fields.clone(),
                }),
                Some(Binding::Tuple { .. }) => Err(format!(
                    "compiler MVP cannot use tuple `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::Array { .. }) => Err(format!(
                    "compiler MVP cannot use array `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::Enum { .. }) => Err(format!(
                    "compiler MVP cannot use enum `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                None => Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
            },
            Expr::TupleAccess(inner, idx) => {
                // Phase Q2: chain may pass through a tuple element
                // before stepping back into a struct sub-binding
                // (e.g. `t.0.x` where `t.0` is a Point).
                let inner_elements = self.resolve_tuple_chain_elements(&inner)?;
                let elem = inner_elements
                    .iter()
                    .find(|e| e.index == idx)
                    .ok_or_else(|| format!("tuple has no element at index {idx}"))?;
                match &elem.shape {
                    TupleElementShape::Scalar { local, ty } => Ok(FieldChainResult::Scalar {
                        local: *local,
                        ty: *ty,
                    }),
                    TupleElementShape::Struct { fields, .. } => Ok(FieldChainResult::Struct {
                        fields: fields.clone(),
                    }),
                    TupleElementShape::Tuple { elements, .. } => Ok(FieldChainResult::Tuple {
                        elements: elements.clone(),
                    }),
                }
            }
            Expr::FieldAccess(inner, field_sym) => {
                let inner_ref = self.resolve_field_chain(&inner)?;
                let fields = match inner_ref {
                    FieldChainResult::Struct { fields } => fields,
                    FieldChainResult::Scalar { .. } | FieldChainResult::Tuple { .. } => {
                        return Err("field access on a non-struct value".to_string());
                    }
                };
                let field_str = self
                    .interner
                    .resolve(field_sym)
                    .ok_or_else(|| "field name missing in interner".to_string())?
                    .to_string();
                let fb = fields
                    .iter()
                    .find(|f| f.name == field_str)
                    .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
                match &fb.shape {
                    FieldShape::Scalar { local, ty } => Ok(FieldChainResult::Scalar {
                        local: *local,
                        ty: *ty,
                    }),
                    FieldShape::Struct { fields, .. } => Ok(FieldChainResult::Struct {
                        fields: fields.clone(),
                    }),
                    FieldShape::Tuple { elements, .. } => Ok(FieldChainResult::Tuple {
                        elements: elements.clone(),
                    }),
                }
            }
            _ => Err(
                "compiler MVP only supports field-access chains rooted at a bare identifier"
                    .to_string(),
            ),
        }
    }


    /// Resolve the LocalId backing `obj.N` where `obj` is required to
    /// be a bare identifier referring to a tuple binding. Used by
    /// element-write lowering. The read side has its own helper because
    /// it returns the type alongside the local for the LoadLocal
    /// instruction's result type.
    pub(super) fn resolve_tuple_element_local(
        &self,
        obj: &ExprRef,
        index: usize,
    ) -> Result<LocalId, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "tuple-access object missing".to_string())?;
        let obj_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports tuple-element assignment on a bare identifier"
                        .to_string(),
                );
            }
        };
        let elements = match self.bindings.get(&obj_sym) {
            Some(Binding::Tuple { elements }) => elements,
            _ => {
                return Err(format!(
                    "`{}` is not a tuple value",
                    self.interner.resolve(obj_sym).unwrap_or("?")
                ));
            }
        };
        elements
            .iter()
            .find(|e| e.index == index)
            .and_then(|e| match &e.shape {
                TupleElementShape::Scalar { local, .. } => Some(*local),
                _ => None,
            })
            .ok_or_else(|| {
                format!(
                    "tuple `{}` has no scalar element at index {} (compound elements cannot be reassigned as a whole — write to inner leaves instead)",
                    self.interner.resolve(obj_sym).unwrap_or("?"),
                    index
                )
            })
    }

    /// Resolve the LocalId backing `obj.field...field = value` for
    /// any depth of chained field access. Walks through nested
    /// struct fields and returns the leaf scalar local. The leaf
    /// must be a scalar; assigning to a struct sub-binding whole
    /// is rejected (consistent with the top-level reassignment ban).
    pub(super) fn resolve_field_local(
        &self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Result<LocalId, String> {
        let inner = self.resolve_field_chain(obj)?;
        let fields = match inner {
            FieldChainResult::Struct { fields } => fields,
            FieldChainResult::Scalar { .. } | FieldChainResult::Tuple { .. } => {
                return Err("field assignment on a non-struct value".to_string());
            }
        };
        let field_str = self
            .interner
            .resolve(field)
            .ok_or_else(|| "field name missing in interner".to_string())?
            .to_string();
        let fb = fields
            .iter()
            .find(|f| f.name == field_str)
            .ok_or_else(|| format!("struct has no field `{field_str}`"))?;
        match &fb.shape {
            FieldShape::Scalar { local, .. } => Ok(*local),
            FieldShape::Struct { .. } => Err(format!(
                "compiler MVP cannot assign whole struct to nested field `{field_str}` (assign individual leaf scalars instead)"
            )),
            FieldShape::Tuple { .. } => Err(format!(
                "compiler MVP cannot assign whole tuple to struct field `{field_str}` (assign individual elements via `obj.{field_str}.N` instead)"
            )),
        }
    }

}
