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
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::array_layout::leaf_scalar_count;
use super::bindings::{Binding, EnumStorage, FieldChainResult, FieldShape, TupleElementShape};
use super::FunctionLower;
use crate::ir::{BinOp, Const, InstKind, LocalId, Type, ValueId};

/// One step of a field / tuple access chain below an array-element
/// read — see [`FunctionLower::resolve_array_element_leaf`].
#[derive(Clone, Copy)]
enum AccessStep {
    Field(DefaultSymbol),
    TupleIdx(usize),
}

/// A field / tuple chain rooted at an array element, resolved to the
/// single leaf scalar it names: `ps[i].x`, `ps[i].pos.y`,
/// `ts[i].0.x`. `leaf` is the leaf's index within one element in
/// declaration order — the same walk `flatten_struct_locals`
/// produces — and `leaf_ty` its IR type.
pub(super) struct ArrayElementLeaf {
    pub(super) arr_sym: DefaultSymbol,
    pub(super) index_ref: ExprRef,
    pub(super) leaf: usize,
    pub(super) leaf_ty: Type,
}

/// Walk `steps` (innermost first) through `ty` and return the leaf
/// index + type they land on. `Ok(None)` for a shape the walk cannot
/// follow; `Err` for one it can follow to something that is not a
/// scalar leaf.
fn resolve_leaf_path(
    module: &crate::ir::Module,
    mut ty: Type,
    steps: impl Iterator<Item = AccessStep>,
    interner: &DefaultStringInterner,
) -> Result<Option<(usize, Type)>, String> {
    let mut acc = 0usize;
    for step in steps {
        match (ty, step) {
            (Type::Struct(id), AccessStep::Field(field)) => {
                // IR struct fields are keyed by *name string*
                // (`StructDef::fields: Vec<(String, Type)>`), so the
                // symbol resolves through the interner — the same
                // convention `lower_field_access` uses.
                let field_name = interner
                    .resolve(field)
                    .ok_or_else(|| "field name missing in interner".to_string())?;
                let fields = module.struct_def(id).fields.clone();
                let mut found = None;
                for (name, ft) in &fields {
                    if name == field_name {
                        found = Some(*ft);
                        break;
                    }
                    acc += leaf_scalar_count(module, *ft);
                }
                match found {
                    Some(ft) => ty = ft,
                    None => {
                        return Err(format!(
                            "struct has no field `{field_name}`"
                        ))
                    }
                }
            }
            (Type::Tuple(id), AccessStep::TupleIdx(idx)) => {
                let elems = module.tuple_defs[id.0 as usize].clone();
                match idx < elems.len() {
                    true => {
                        acc += elems[..idx]
                            .iter()
                            .map(|t| leaf_scalar_count(module, *t))
                            .sum::<usize>();
                        ty = elems[idx];
                    }
                    false => return Err(format!("tuple has no element at index {idx}")),
                }
            }
            _ => {
                // e.g. `ps[i].0` on a struct element, or a field name
                // on a tuple — report it rather than silently
                // dropping the access.
                return Ok(None);
            }
        }
    }
    match ty {
        // The chain lands on a scalar leaf — the shortcut's target.
        Type::I64 | Type::U64 | Type::F64 | Type::F32 | Type::Bool
        | Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::I32 | Type::U32
        | Type::Str => Ok(Some((acc, ty))),
        // A compound value the chain stops short of: reading it
        // whole needs the pending-compound channel, so point the
        // user at the element-binding form instead.
        Type::Struct(_) | Type::Tuple(_) | Type::Enum(_) => Err(
            "field chain names a whole compound value; bind the element first (`val p = ps[i]`) \
             and access fields on the binding"
                .to_string(),
        ),
        _ => Ok(None),
    }
}

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
            FieldChainResult::Struct { fields, .. } => fields,
            FieldChainResult::Scalar { .. }
            | FieldChainResult::Tuple { .. }
            | FieldChainResult::Enum(_) => {
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
            // JIT-enum-1: an enum-typed field is a compound value
            // too, so it leaves the value graph the same way and is
            // stashed for whatever consumes it (tail return, a `val`
            // binding, an argument).
            FieldShape::Enum(storage) => {
                self.pending_struct_value = None;
                self.pending_tuple_value = None;
                self.pending_enum_value = Some((**storage).clone());
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
                Some(Binding::RefScalar { .. }) => Err(format!(
                    "compiler MVP cannot use reference scalar `{}` as a field-access chain root",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::Struct { struct_id, fields }) => Ok(FieldChainResult::Struct {
                    struct_id: *struct_id,
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
                Some(Binding::FunctionPtr { .. }) => Err(format!(
                    "compiler MVP cannot use function value `{}` in a field-access chain",
                    self.interner.resolve(sym).unwrap_or("?")
                )),
                Some(Binding::DynTraitObj { .. }) => Err(format!(
                    "compiler MVP cannot use dyn-trait `{}` in a field-access chain",
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
                    TupleElementShape::Struct { struct_id, fields } => Ok(FieldChainResult::Struct {
                        struct_id: *struct_id,
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
                    FieldChainResult::Struct { fields, .. } => fields,
                    FieldChainResult::Scalar { .. }
                    | FieldChainResult::Tuple { .. }
                    | FieldChainResult::Enum(_) => {
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
                    FieldShape::Struct { struct_id, fields } => Ok(FieldChainResult::Struct {
                        struct_id: *struct_id,
                        fields: fields.clone(),
                    }),
                    FieldShape::Tuple { elements, .. } => Ok(FieldChainResult::Tuple {
                        elements: elements.clone(),
                    }),
                    FieldShape::Enum(storage) => {
                        Ok(FieldChainResult::Enum((**storage).clone()))
                    }
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
            FieldChainResult::Struct { fields, .. } => fields,
            FieldChainResult::Scalar { .. }
            | FieldChainResult::Tuple { .. }
            | FieldChainResult::Enum(_) => {
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
            // JIT-enum-1: an enum field has no single local to store
            // into. `resolve_field_enum_storage` is the route the
            // assignment path takes instead; reaching here means the
            // caller did not try it first.
            FieldShape::Enum(_) => Err(format!(
                "internal: enum-typed field `{field_str}` must be assigned through its storage"
            )),
        }
    }

    /// JIT-enum-1: resolve `obj.field` to the field's `EnumStorage`
    /// when that field is enum-typed, and `None` for every other
    /// shape so the caller can fall back to the scalar-local path.
    /// Assignment needs this because an enum occupies a tag local
    /// plus a payload slot per variant element, not one local.
    pub(super) fn resolve_field_enum_storage(
        &self,
        obj: &ExprRef,
        field: DefaultSymbol,
    ) -> Option<EnumStorage> {
        let FieldChainResult::Struct { fields, .. } = self.resolve_field_chain(obj).ok()? else {
            return None;
        };
        let field_str = self.interner.resolve(field)?;
        match &fields.iter().find(|f| f.name == field_str)?.shape {
            FieldShape::Enum(storage) => Some((**storage).clone()),
            _ => None,
        }
    }

    /// DATA-ORIENTED Phase 0: resolve a field / tuple access chain
    /// rooted at an array element (`ps[i].x`, `ps[i].pos.y`,
    /// `ts[i].0.x`) down to `(array binding, index expression, leaf
    /// index, leaf type)`. `Ok(None)` when the expression is not
    /// that shape — the regular struct-binding paths take over — so
    /// callers can use this as a try-first probe. The chain must
    /// land on a *scalar* leaf: a compound stopover keeps the
    /// pending-compound channel shape (`val p = ps[i]` then `p.x`).
    pub(super) fn resolve_array_element_leaf(
        &self,
        expr: &ExprRef,
    ) -> Result<Option<ArrayElementLeaf>, String> {
        // Collect the steps outside-in, then walk them innermost
        // first against the element type.
        let mut steps: Vec<AccessStep> = Vec::new();
        let mut cursor = *expr;
        let index_ref;
        let arr_sym;
        loop {
            let e = self
                .program
                .expression
                .get(&cursor)
                .ok_or_else(|| "field-chain expression missing".to_string())?;
            match e {
                Expr::FieldAccess(inner, field) => {
                    steps.push(AccessStep::Field(field));
                    cursor = inner;
                }
                Expr::TupleAccess(inner, idx) => {
                    steps.push(AccessStep::TupleIdx(idx));
                    cursor = inner;
                }
                Expr::SliceAccess(obj, info) => {
                    if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                        return Ok(None);
                    }
                    let Some(index) = info.start else {
                        return Ok(None);
                    };
                    let Some(Expr::Identifier(sym)) =
                        self.program.expression.get(&obj)
                    else {
                        return Ok(None);
                    };
                    index_ref = index;
                    arr_sym = sym;
                    break;
                }
                _ => return Ok(None),
            }
        }
        let Some(Binding::Array { element_ty, .. }) = self.bindings.get(&arr_sym).cloned() else {
            return Ok(None);
        };
        // A scalar element has no fields to name; leave it to the
        // slice-access path (which also produces the right error for
        // `flags[i].x`-style mistakes).
        if !matches!(element_ty, Type::Struct(_) | Type::Tuple(_)) {
            return Ok(None);
        }
        let (leaf, leaf_ty) = match resolve_leaf_path(
            self.module,
            element_ty,
            steps.iter().rev().copied(),
            self.interner,
        )? {
            Some(pair) => pair,
            None => return Ok(None),
        };
        Ok(Some(ArrayElementLeaf {
            arr_sym,
            index_ref,
            leaf,
            leaf_ty,
        }))
    }

    /// The guarded, lowered element index for an array-element leaf
    /// access — shared by the read and write shortcuts. Returns
    /// `(runtime index value, const-folded index if it folded)`.
    fn lower_array_element_index(
        &mut self,
        info: &ArrayElementLeaf,
        length: usize,
    ) -> Result<(ValueId, Option<usize>), String> {
        match self.resolve_const_index(&info.index_ref, length) {
            super::array_access::ConstIndex::Valid(i) => {
                let idx_v = self
                    .emit(
                        InstKind::Const(Const::U64(i as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                Ok((idx_v, Some(i)))
            }
            super::array_access::ConstIndex::OutOfBounds => {
                Err(format!("array index out of bounds (length {length})"))
            }
            super::array_access::ConstIndex::NotConstant => {
                let raw_idx = self
                    .lower_expr(&info.index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
                let idx_ty = self.value_scalar(&info.index_ref).unwrap_or(Type::U64);
                let idx_v = self.emit_index_guard(&info.index_ref, raw_idx, idx_ty, length)?;
                Ok((idx_v, None))
            }
        }
    }

    /// `ps[i].f` read — one `ArrayLoad` of the named leaf, honouring
    /// the binding's layout. This is DATA-ORIENTED's single-column
    /// shortcut and the reason SoA pays: a loop over one field
    /// touches one column instead of materialising every leaf of
    /// every element. `Ok(None)` when the expression is not an
    /// array-element leaf chain.
    pub(super) fn try_lower_array_element_leaf(
        &mut self,
        expr: &ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(info) = self.resolve_array_element_leaf(expr)? else {
            return Ok(None);
        };
        self.emit_array_element_leaf_load(&info)
    }

    /// The load half of the shortcut, split out so the write path
    /// lowers the *same* leaf index (identical guard, identical
    /// fold) instead of a second implementation drifting.
    fn emit_array_element_leaf_load(
        &mut self,
        info: &ArrayElementLeaf,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(Binding::Array { element_ty, length, storage, .. }) =
            self.bindings.get(&info.arr_sym).cloned()
        else {
            return Ok(None);
        };
        let leaf_count = leaf_scalar_count(self.module, element_ty);
        let (idx_v, const_i) = self.lower_array_element_index(info, length)?;
        // AoS: flat leaf index `i * leaf_count + leaf` (folded when
        // the element index folded). SoA: the leaf's own column slot
        // at the element index — one load, no arithmetic.
        let (slot, leaf_idx_v) = match &storage {
            super::bindings::ArrayStorage::Columns(cols) => (cols[info.leaf], idx_v),
            super::bindings::ArrayStorage::Interleaved(slot) => {
                let leaf_idx_v = match const_i {
                    Some(i) => self
                        .emit(
                            InstKind::Const(Const::U64((i * leaf_count + info.leaf) as u64)),
                            Some(Type::U64),
                        )
                        .expect("Const returns a value"),
                    None => {
                        let base_v = {
                            let leaf_count_v = self
                                .emit(
                                    InstKind::Const(Const::U64(leaf_count as u64)),
                                    Some(Type::U64),
                                )
                                .expect("Const returns a value");
                            self.emit(
                                InstKind::BinOp {
                                    op: BinOp::Mul,
                                    lhs: idx_v,
                                    rhs: leaf_count_v,
                                },
                                Some(Type::U64),
                            )
                            .expect("imul returns")
                        };
                        let off_v = self
                            .emit(
                                InstKind::Const(Const::U64(info.leaf as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns a value");
                        self.emit(
                            InstKind::BinOp {
                                op: BinOp::Add,
                                lhs: base_v,
                                rhs: off_v,
                            },
                            Some(Type::U64),
                        )
                        .expect("iadd returns")
                    }
                };
                (*slot, leaf_idx_v)
            }
        };
        let v = self
            .emit(
                InstKind::ArrayLoad {
                    slot,
                    index: leaf_idx_v,
                    elem_ty: info.leaf_ty,
                },
                Some(info.leaf_ty),
            )
            .expect("ArrayLoad returns");
        // A scalar value leaves the pending-compound channels, same
        // as the scalar paths in `lower_field_access`.
        self.pending_struct_value = None;
        self.pending_tuple_value = None;
        Ok(Some(Some(v)))
    }

    /// `ps[i].f = v` write — the store half of the shortcut: one
    /// `ArrayStore` of the named leaf. `Ok(None)` when the lhs is
    /// not an array-element leaf chain. Yields the stored value so
    /// assignment-as-expression keeps its meaning.
    pub(super) fn try_lower_array_element_leaf_store(
        &mut self,
        lhs: &ExprRef,
        rhs: &ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(info) = self.resolve_array_element_leaf(lhs)? else {
            return Ok(None);
        };
        let Some(Binding::Array { element_ty, length, storage, .. }) =
            self.bindings.get(&info.arr_sym).cloned()
        else {
            return Ok(None);
        };
        let leaf_count = leaf_scalar_count(self.module, element_ty);
        let (idx_v, const_i) = self.lower_array_element_index(&info, length)?;
        let v = self
            .lower_expr(rhs)?
            .ok_or_else(|| "field assignment rhs produced no value".to_string())?;
        let (slot, leaf_idx_v) = match &storage {
            super::bindings::ArrayStorage::Columns(cols) => (cols[info.leaf], idx_v),
            super::bindings::ArrayStorage::Interleaved(slot) => {
                let leaf_idx_v = match const_i {
                    Some(i) => self
                        .emit(
                            InstKind::Const(Const::U64((i * leaf_count + info.leaf) as u64)),
                            Some(Type::U64),
                        )
                        .expect("Const returns a value"),
                    None => {
                        let base_v = {
                            let leaf_count_v = self
                                .emit(
                                    InstKind::Const(Const::U64(leaf_count as u64)),
                                    Some(Type::U64),
                                )
                                .expect("Const returns a value");
                            self.emit(
                                InstKind::BinOp {
                                    op: BinOp::Mul,
                                    lhs: idx_v,
                                    rhs: leaf_count_v,
                                },
                                Some(Type::U64),
                            )
                            .expect("imul returns")
                        };
                        let off_v = self
                            .emit(
                                InstKind::Const(Const::U64(info.leaf as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns a value");
                        self.emit(
                            InstKind::BinOp {
                                op: BinOp::Add,
                                lhs: base_v,
                                rhs: off_v,
                            },
                            Some(Type::U64),
                        )
                        .expect("iadd returns")
                    }
                };
                (*slot, leaf_idx_v)
            }
        };
        self.emit(
            InstKind::ArrayStore {
                slot,
                index: leaf_idx_v,
                value: v,
                elem_ty: info.leaf_ty,
            },
            None,
        );
        Ok(Some(Some(v)))
    }
}
