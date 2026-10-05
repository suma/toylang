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

use string_interner::DefaultSymbol;

use crate::ir::{ArraySlotId, EnumId, LocalId, StructId, TupleId, Type, ValueId};

/// Backing slots of one array binding — see `Binding::Array`.
#[derive(Debug, Clone)]
pub(super) enum ArrayStorage {
    /// AoS: a single slot of `length * leaf_count` leaves,
    /// element-major (`leaf_idx = i * leaf_count + j`).
    Interleaved(ArraySlotId),
    /// SoA: one homogeneous slot per leaf scalar, each `length`
    /// long (`leaf j of element i` lives at `columns[j][i]`).
    Columns(Vec<ArraySlotId>),
}

impl ArrayStorage {
    /// The one slot behind a *scalar*-element array — the AoS slot,
    /// or the single column a one-leaf SoA array degenerates to.
    /// Callers guard on the element type being scalar, so any other
    /// shape here is a lowering bug.
    pub(super) fn scalar_slot(&self) -> ArraySlotId {
        match self {
            ArrayStorage::Interleaved(slot) => *slot,
            ArrayStorage::Columns(cols) => cols[0],
        }
    }
}

/// Top-level binding shape attached to each user-visible name.
#[derive(Debug, Clone)]
pub(super) enum Binding {    Scalar {
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
    /// Fixed-size array binding (Phase Y). Backed by one or more
    /// per-function stack slots; both constant and runtime indices
    /// lower to `ArrayLoad` / `ArrayStore` against them.
    Array {
        element_ty: Type,
        length: usize,
        /// DATA-ORIENTED: the backing slots. `Interleaved` is the
        /// classic AoS shape — one slot holding `length * leaf_count`
        /// leaves, element-major, addressed by the flat leaf index
        /// `i * leaf_count + j`. `Columns` is SoA — one slot per leaf
        /// scalar, each a homogeneous array of `length` elements,
        /// so leaf `j` of element `i` is just `columns[j][i]`.
        /// Columns reuse the existing scalar-array slot machinery
        /// unchanged, which is why neither `ArraySlotInfo`, the
        /// codegen, nor the IR VM knows SoA exists.
        storage: ArrayStorage,
    },
    /// Closures Phase 5b: function-pointer binding. The `local`
    /// holds a `Type::U64` value that is the runtime address of a
    /// function — produced either by `InstKind::FuncAddr` (when a
    /// closure / direct fn name is passed as a value) or by the
    /// caller's argument when this binding represents a
    /// function-typed parameter (`f: (T1, T2) -> R`). A
    /// subsequent `Expr::Call(name, args)` whose name resolves
    /// here lowers to `LoadLocal` + `InstKind::CallIndirect`
    /// using the recorded signature.
    FunctionPtr {
        local: LocalId,
        param_tys: Vec<Type>,
        ret_ty: Type,
    },
    /// A5-P2: `&dyn TraitName` parameter binding. The fat pointer
    /// arrives as two flat U64 locals (the cranelift ABI flattens
    /// `Type::Tuple([U64, U64])` parameters into a pair of scalar
    /// slots). `data_ptr_local` holds the underlying struct's data
    /// pointer (null for empty structs in MVP-A) and
    /// `vtable_ptr_local` holds the vtable address. Method dispatch
    /// on this binding loads `vtable_ptr`, indexes by the method's
    /// trait-order slot, and emits `CallIndirect` against the
    /// trait's declared signature.
    DynTraitObj {
        trait_sym: DefaultSymbol,
        data_ptr_local: LocalId,
        vtable_ptr_local: LocalId,
    },
    /// CONST-ARRAY: a borrowed array, `t: &[u32; 64]` -- the address
    /// of the first element, which is a stack array's slot, a `const`
    /// table in the read-only section, or another borrow passed on.
    /// Elements are scalars packed at their own width (the layout both
    /// a scalar stack array and a `const` table have), so element `i`
    /// is one `PtrRead` / `PtrWrite` at `i * stride` after the bounds
    /// check an owned array gets.
    ArrayRef {
        ptr: LocalId,
        element_ty: Type,
        length: usize,
        is_mut: bool,
    },
    /// RANGE-FOR: a range value, `val r = a..b`. Its two bounds are
    /// two scalar locals of the element type. What a range can do in
    /// the compiled lanes is read its bounds (`r.start` / `r.end`,
    /// which is also what `for i in r` is rewritten into), be copied
    /// into another name, and be printed; anything else that meets
    /// one is refused by name rather than guessed at.
    Range {
        start: LocalId,
        end: LocalId,
        ty: Type,
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
    /// A payload of type `()` — `Result<(), E>`'s `Ok` (UNIT-TYPE-ARG).
    ///
    /// It holds **no local**, deliberately. `flatten_compound_leaf_types`
    /// gives `Type::Unit` zero leaves, so a local here would put the
    /// storage's flat value list one entry ahead of the function
    /// boundary's and every later payload would be read from the wrong
    /// slot. Nothing to store, nothing to load, nothing to copy.
    Unit,
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
    /// JIT-enum-1: enum-typed field reached by the chain
    /// (`p.color`). The storage is the scrutinee / source for
    /// whatever the caller does next — match on it, copy it into a
    /// binding, or stash it as the pending enum value.
    Enum(EnumStorage),
}

/// Resolved match scrutinee. Enum scrutinees are dispatched by
/// reading the existing tag local; scalar scrutinees evaluate the
/// scrutinee expression once and pin the result for arm comparisons.
#[derive(Debug, Clone)]
pub(super) enum MatchScrutinee {
    Enum(EnumStorage),
    Scalar { value: ValueId, ty: Type },
    /// PATTERN-COMPOUND-LOWER: a struct being matched by its fields.
    /// The bindings are the scrutinee's own locals — a field pattern
    /// reads them for a comparison and re-binds them for a name, so
    /// nothing is copied.
    Struct { struct_id: StructId, fields: Vec<FieldBinding> },
    /// The same for a tuple.
    Tuple { elements: Vec<TupleElementBinding> },
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
    /// JIT-enum-1: enum-typed struct field. Holds a full
    /// `EnumStorage` — a tag local plus a payload slot per element of
    /// every variant — which is the same thing `Binding::Enum` and
    /// `PayloadSlot::Enum` hold, so every enum helper (construct,
    /// copy, tag dispatch, payload bind) works on a field unchanged.
    /// A field cannot be one local the way a scalar is: the variant
    /// is only known at runtime, so the storage has to be wide enough
    /// for any of them.
    Enum(Box<EnumStorage>),
}

/// DIAG-SYMBOL-NAME-LOWER: what a diagnostic calls a field's backing
/// shape. `{:?}` here would print the whole storage tree — every leaf
/// local of every variant — where the reader only needs to know which
/// of the four shapes was found.
/// DIAG-SYMBOL-NAME-LOWER: what a diagnostic calls the shape a
/// field-access chain landed on. Same reasoning as
/// [`field_shape_name`].
pub(super) fn field_chain_result_name(chain: &FieldChainResult) -> &'static str {
    match chain {
        FieldChainResult::Scalar { .. } => "a scalar",
        FieldChainResult::Struct { .. } => "a struct",
        FieldChainResult::Tuple { .. } => "a tuple",
        FieldChainResult::Enum(_) => "an enum",
    }
}

pub(super) fn field_shape_name(shape: &FieldShape) -> &'static str {
    match shape {
        FieldShape::Scalar { .. } => "a scalar field",
        FieldShape::Struct { .. } => "a struct field",
        FieldShape::Tuple { .. } => "a tuple field",
        FieldShape::Enum { .. } => "an enum field",
    }
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
            // Same canonical order the boundary flatteners use for a
            // `Type::Enum` field: tag first, then every variant's
            // payload leaves in declaration order.
            FieldShape::Enum(storage) => {
                flatten_enum_storage_locals_into(storage, &mut out);
            }
        }
    }
    out
}

/// Flatten an `EnumStorage` into `(LocalId, Type)` pairs in the
/// canonical order used by `flatten_enum_dests_into` and
/// `flatten_compound_leaf_types(Type::Enum)`: `(tag_local, U64)`
/// first, then each variant's payload slots in declaration order
/// (recursing through nested enum / struct / tuple payloads).
/// Used by `&mut Enum` writeback so the body-time leaf list aligns
/// with the declaration-time `self_writeback_types` shape.
pub(super) fn flatten_enum_storage_locals(storage: &EnumStorage) -> Vec<(LocalId, Type)> {
    let mut out = Vec::new();
    flatten_enum_storage_locals_into(storage, &mut out);
    out
}

fn flatten_enum_storage_locals_into(storage: &EnumStorage, out: &mut Vec<(LocalId, Type)>) {
    out.push((storage.tag_local, Type::U64));
    for variant in &storage.payloads {
        for slot in variant {
            match slot {
                PayloadSlot::Scalar { local, ty } => out.push((*local, *ty)),
                PayloadSlot::Enum(inner) => flatten_enum_storage_locals_into(inner, out),
                PayloadSlot::Struct { fields, .. } => {
                    out.extend(flatten_struct_locals(fields));
                }
                PayloadSlot::Tuple { elements, .. } => {
                    out.extend(flatten_tuple_element_locals(elements));
                }
                // A `()` payload has no local (UNIT-TYPE-ARG); it must
                // not appear here either, or the flat list runs ahead
                // of the boundary's leaf order.
                PayloadSlot::Unit => {}
            }
        }
    }
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

/// The names in scope while one function body is lowered, with an undo
/// log so a scope can be left without having copied the table on the
/// way in.
///
/// Every block used to clone the whole table on entry and write every
/// entry back on exit, and every `match` arm cloned it too: ~5,000
/// copies of a ~10-entry table on `poc/logsearch`, ~4 ms of a 22 ms
/// lowering, and a struct binding's leaves cloned with each copy
/// (LOWER-BINDING-CLONE). Now `insert` records what it displaced, a
/// scope remembers where the log stood ([`Self::mark`]), and leaving
/// it walks back only what was written since.
///
/// Reads go through `Deref` to the map. There is no `DerefMut`: a
/// write that skipped the log would survive the rollback.
#[derive(Debug, Default)]
pub(super) struct BindingMap {
    map: rustc_hash::FxHashMap<DefaultSymbol, Binding>,
    /// `(name, what it held before)` for each `insert`, oldest first.
    log: Vec<(DefaultSymbol, Option<Binding>)>,
}

/// A point in a [`BindingMap`]'s history to return to.
#[derive(Debug, Clone, Copy)]
pub(super) struct BindingMark(usize);

impl std::ops::Deref for BindingMap {
    type Target = rustc_hash::FxHashMap<DefaultSymbol, Binding>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl BindingMap {
    pub(super) fn insert(&mut self, name: DefaultSymbol, binding: Binding) {
        let old = self.map.insert(name, binding);
        self.log.push((name, old));
    }

    pub(super) fn mark(&self) -> BindingMark {
        BindingMark(self.log.len())
    }

    /// Back to exactly what the table held at `mark`: names introduced
    /// since are gone, names rebound since hold their old binding. A
    /// `match` arm's pattern names must not leak into the next arm.
    pub(super) fn rollback(&mut self, mark: BindingMark) {
        while self.log.len() > mark.0 {
            let (name, old) = self.log.pop().expect("log is longer than the mark");
            match old {
                Some(binding) => {
                    self.map.insert(name, binding);
                }
                None => {
                    self.map.remove(&name);
                }
            }
        }
    }

    /// Leave a block: names it *rebound* get their outer binding back,
    /// names it *introduced* stay. Removing those too is what scoping
    /// would mean, but `let_lowering`'s type inference still looks them
    /// up after the block (`val t: u64 = with allocator = a { .. x }`),
    /// so this is the behaviour the table-copying version had.
    ///
    /// The log keeps one `(name, None)` per name the block introduced,
    /// so an enclosing [`Self::rollback`] still removes them.
    pub(super) fn restore_shadowed(&mut self, mark: BindingMark) {
        let written = self.log.split_off(mark.0);
        let mut first: rustc_hash::FxHashMap<DefaultSymbol, Option<Binding>> =
            rustc_hash::FxHashMap::default();
        let mut order: Vec<DefaultSymbol> = Vec::new();
        for (name, old) in written {
            if let std::collections::hash_map::Entry::Vacant(slot) = first.entry(name) {
                slot.insert(old);
                order.push(name);
            }
        }
        for name in order {
            match first.remove(&name).expect("recorded above") {
                Some(outer) => {
                    self.map.insert(name, outer);
                }
                None => self.log.push((name, None)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Binding, BindingMap};
    use crate::ir::{LocalId, Type};
    use string_interner::{DefaultStringInterner, DefaultSymbol};

    fn scalar(n: u32) -> Binding {
        Binding::Scalar { local: LocalId(n), ty: Type::U64 }
    }

    fn local_of(map: &BindingMap, name: DefaultSymbol) -> Option<u32> {
        match map.get(&name) {
            Some(Binding::Scalar { local, .. }) => Some(local.0),
            Some(_) => panic!("only scalars here"),
            None => None,
        }
    }

    /// What the table-copying code did on leaving a block: every name
    /// bound before the block gets its old binding back, names the
    /// block introduced keep whatever they ended up as.
    #[test]
    fn leaving_a_block_restores_rebound_names_and_keeps_new_ones() {
        let mut interner = DefaultStringInterner::default();
        let (x, y, z) = (interner.get_or_intern("x"), interner.get_or_intern("y"), interner.get_or_intern("z"));
        let mut map = BindingMap::default();
        map.insert(x, scalar(1));
        let block = map.mark();
        map.insert(x, scalar(2)); // rebinds an outer name
        map.insert(y, scalar(3)); // introduces a name ...
        map.insert(y, scalar(4)); // ... and rebinds it within the block
        map.insert(x, scalar(5));
        map.restore_shadowed(block);
        assert_eq!(local_of(&map, x), Some(1));
        assert_eq!(local_of(&map, y), Some(4));
        assert_eq!(local_of(&map, z), None);
    }

    /// A `match` arm returns to exactly the table it started from --
    /// including names a block inside the arm introduced.
    #[test]
    fn an_arm_rollback_also_removes_what_an_inner_block_introduced() {
        let mut interner = DefaultStringInterner::default();
        let (x, y) = (interner.get_or_intern("x"), interner.get_or_intern("y"));
        let mut map = BindingMap::default();
        map.insert(x, scalar(1));
        let arm = map.mark();
        map.insert(x, scalar(2));
        let block = map.mark();
        map.insert(y, scalar(3));
        map.insert(x, scalar(4));
        map.restore_shadowed(block);
        assert_eq!(local_of(&map, x), Some(2));
        assert_eq!(local_of(&map, y), Some(3));
        map.rollback(arm);
        assert_eq!(local_of(&map, x), Some(1));
        assert_eq!(local_of(&map, y), None);
    }
}
