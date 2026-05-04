//! Per-binding shape descriptors for the lowering pass.
//!
//! Every `val` / `var` introduces a `Binding` whose variant captures
//! the value's storage shape. Scalar bindings live in a single IR
//! `LocalId`; struct / tuple / enum bindings expand into multiple
//! locals (one per leaf scalar) so the IR's value graph never sees a
//! compound value flow through SSA. Array bindings share a per-array
//! `ArraySlotId` for stack-backed memory access.
//!
//! Two flatten helpers (`flatten_struct_locals`,
//! `flatten_tuple_element_locals`) walk the recursive shapes and
//! produce a `Vec<(LocalId, Type)>` of leaf scalars in declaration
//! order — used at function-boundary sites and array-slot lowering
//! to round-trip compound values through scalar IR slots.

use crate::ir::{ArraySlotId, EnumId, LocalId, StructId, TupleId, Type, ValueId};

/// Top-level binding shape attached to each user-visible name.
#[derive(Debug, Clone)]
pub(super) enum Binding {
    Scalar {
        local: LocalId,
        ty: Type,
    },
    /// REF-Stage-2 (b)+(c)+(g): `&mut T` or `&T` parameter binding.
    /// The IR `local` holds a U64-sized pointer (incoming
    /// `stack_addr` value from the caller's `AddressOf`); reads of
    /// the binding emit `LoadLocal` + `LoadRef`, and assignments to
    /// the binding (only allowed when `is_mut`) emit `LoadLocal` +
    /// `StoreRef` so the mutation propagates back to the caller's
    /// storage. Today only created for scalar `pointee_ty` —
    /// struct / tuple / enum `&mut T` is a future phase.
    RefScalar {
        local: LocalId,
        pointee_ty: Type,
        is_mut: bool,
    },
    Struct {
        /// Identifies the monomorphised struct instance this binding
        /// belongs to. Codegen uses it to look up the field type list
        /// when flattening at function boundaries; lowering uses it to
        /// validate explicit-return / re-binding compatibility.
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    /// Tuple bindings expand into one local per element, indexed
    /// positionally rather than by name.
    Tuple {
        elements: Vec<TupleElementBinding>,
    },
    /// Enum bindings carry an `EnumStorage` tree: a tag local plus
    /// per-variant payload slots.
    Enum(EnumStorage),
    /// Fixed-size array binding. Backed by a per-function stack
    /// slot (Phase Y); both constant and runtime indices lower to
    /// `ArrayLoad` / `ArrayStore` against this slot.
    Array {
        element_ty: Type,
        length: usize,
        slot: ArraySlotId,
    },
}

/// Storage tree for one enum value in IR. `tag_local` holds the
/// 0-based variant index; `payloads[variant_idx]` is one slot per
/// declared payload of that variant. Slots are recursive — a scalar
/// payload uses a single `LocalId`, an enum payload nests another
/// `EnumStorage`. The same shape drives function-boundary flattening
/// (codegen recurses through `Type::Enum` in
/// `flatten_struct_to_cranelift_tys`), so the order is canonical.
#[derive(Debug, Clone)]
pub(super) struct EnumStorage {
    pub(super) enum_id: EnumId,
    pub(super) tag_local: LocalId,
    pub(super) payloads: Vec<Vec<PayloadSlot>>,
}

#[derive(Debug, Clone)]
pub(super) enum PayloadSlot {
    Scalar {
        local: LocalId,
        ty: Type,
    },
    Enum(Box<EnumStorage>),
    /// Struct-typed payload. Stores the same `FieldBinding` tree
    /// that `Binding::Struct` uses, so all the existing struct
    /// helpers work unchanged.
    Struct {
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    /// Tuple-typed payload. Stores the same `TupleElementBinding`
    /// list that `Binding::Tuple` uses.
    Tuple {
        tuple_id: TupleId,
        elements: Vec<TupleElementBinding>,
    },
}

/// One element of a `Binding::Tuple`. `index` is the element's
/// positional index used by `t.0` / `t.1` access. The `shape`
/// recursion mirrors `FieldShape` — a tuple element may itself
/// be a struct (`(Point, i64)`) or another tuple (`((a, b), c)`).
#[derive(Debug, Clone)]
pub(super) struct TupleElementBinding {
    pub(super) index: usize,
    pub(super) shape: TupleElementShape,
}

#[derive(Debug, Clone)]
pub(super) enum TupleElementShape {
    Scalar {
        local: LocalId,
        ty: Type,
    },
    Struct {
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    Tuple {
        tuple_id: TupleId,
        elements: Vec<TupleElementBinding>,
    },
}

impl TupleElementBinding {
    /// Convenience accessor for sites that have already verified the
    /// element is scalar (mostly the boundary / print fast paths).
    /// Returns `None` for compound shapes so the caller can detour.
    #[allow(dead_code)]
    pub(super) fn scalar(&self) -> Option<(LocalId, Type)> {
        match &self.shape {
            TupleElementShape::Scalar { local, ty } => Some((*local, *ty)),
            _ => None,
        }
    }
}

/// Result of walking a field-access chain (`a`, `a.b`, `a.b.c`, ...).
/// Either we land on a scalar leaf (ready for LoadLocal) or on an
/// inner struct / tuple sub-binding.
#[derive(Debug, Clone)]
pub(super) enum FieldChainResult {
    #[allow(dead_code)]
    Scalar { local: LocalId, ty: Type },
    /// Inner struct sub-binding — `struct_id` carries the
    /// monomorphised struct shape so callers can dispatch
    /// methods on this nested struct without a separate lookup.
    Struct {
        struct_id: crate::ir::StructId,
        fields: Vec<FieldBinding>,
    },
    /// Inner tuple sub-binding — e.g. `outer.inner` where
    /// `inner: (i64, i64)`. Callers either step further with a
    /// `TupleAccess` or stash the elements as a pending tuple.
    Tuple { elements: Vec<TupleElementBinding> },
}

/// Resolved match scrutinee. Enum scrutinees are dispatched by
/// reading the existing tag local; scalar scrutinees evaluate the
/// scrutinee expression once and pin the result for arm comparisons.
#[derive(Debug, Clone)]
pub(super) enum MatchScrutinee {
    Enum(EnumStorage),
    Scalar { value: ValueId, ty: Type },
}

/// One field of a `Binding::Struct`. `name` matches `StructField.name`
/// exactly so we can compare against the interner-resolved field name
/// at access sites without re-interning. The `shape` is recursive
/// because struct fields can themselves be structs / tuples.
#[derive(Debug, Clone)]
pub(super) struct FieldBinding {
    pub(super) name: String,
    pub(super) shape: FieldShape,
}

#[derive(Debug, Clone)]
pub(super) enum FieldShape {
    Scalar {
        local: LocalId,
        ty: Type,
    },
    Struct {
        struct_id: StructId,
        fields: Vec<FieldBinding>,
    },
    /// Tuple-typed struct field. Stores the same `TupleElementBinding`
    /// list `Binding::Tuple` uses, so a chain access like
    /// `outer.inner.0` walks struct → tuple element via the existing
    /// field-chain helpers.
    #[allow(dead_code)]
    Tuple {
        tuple_id: TupleId,
        elements: Vec<TupleElementBinding>,
    },
}

/// Flatten a `FieldBinding` tree into a sequential `(LocalId, Type)`
/// list, in declaration order. Mirrors the flat scalar walk codegen
/// does over `Module.struct_defs` so the lowering and backend agree
/// on parameter / return order.
pub(super) fn flatten_struct_locals(fields: &[FieldBinding]) -> Vec<(LocalId, Type)> {
    let mut out = Vec::new();
    for fb in fields {
        match &fb.shape {
            FieldShape::Scalar { local, ty } => out.push((*local, *ty)),
            FieldShape::Struct { fields: nested, .. } => {
                out.extend(flatten_struct_locals(nested));
            }
            FieldShape::Tuple { elements, .. } => {
                out.extend(flatten_tuple_element_locals(elements));
            }
        }
    }
    out
}

/// Flatten a tuple-element list into a sequential `(LocalId, Type)`
/// list, recursing through struct / tuple sub-shapes so compound
/// elements still expose their leaf scalars in declaration order.
pub(super) fn flatten_tuple_element_locals(
    elements: &[TupleElementBinding],
) -> Vec<(LocalId, Type)> {
    let mut out = Vec::new();
    for el in elements {
        match &el.shape {
            TupleElementShape::Scalar { local, ty } => {
                out.push((*local, *ty));
            }
            TupleElementShape::Struct { fields, .. } => {
                out.extend(flatten_struct_locals(fields));
            }
            TupleElementShape::Tuple { elements: inner, .. } => {
                out.extend(flatten_tuple_element_locals(inner));
            }
        }
    }
    out
}
