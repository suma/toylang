//! Array literal lowering and indexed access.
//!
//! `lower_let`'s `Expr::ArrayLiteral` arm allocates a single
//! `ArraySlotId` per array binding and stores each element via
//! `store_array_element`. `lower_slice_access` and
//! `lower_slice_assign` handle `arr[i]` reads / writes for both
//! constant and runtime indices, plus const-bound range slicing
//! (`arr[start..end]`). Compound array elements (struct / tuple)
//! occupy `leaf_count` consecutive 8-byte leaf slots inside the
//! same backing buffer; per-leaf ArrayLoad / ArrayStore sequences
//! materialise / decompose them at the access site.
//!
//! `try_constant_index` lives here too — it folds a literal
//! integer index (or a top-level const reference) to a `usize` so
//! constant-index access can hit the same `ArrayLoad` instruction
//! as runtime index without a redundant runtime `Const` round-trip.

use frontend::ast::{Expr, ExprRef, UnaryOp};

use super::array_layout::leaf_scalar_count;
use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, ArrayStorage, Binding, FieldBinding,
    TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{BinOp, Const, InstKind, LocalId, Terminator, Type, ValueId};

/// Where the leaves of one array element live, prepared once for the
/// whole element (see `begin_leaf_addressing`).
pub(super) struct LeafAddressing {
    storage: ArrayStorage,
    leaf_count: usize,
    /// The element index as a value — SoA addresses columns with it
    /// directly.
    elem_idx: ValueId,
    /// Set when the index folded, which lets AoS fold the whole leaf
    /// index into one constant.
    const_elem_idx: Option<usize>,
    /// AoS with a runtime index: `i * leaf_count`, shared by the
    /// element's leaves.
    aos_base: Option<ValueId>,
}

/// Result of folding a constant array index against the array length.
pub(super) enum ConstIndex {
    /// Folded, negative-adjusted, in-bounds index.
    Valid(usize),
    /// Constant but out of bounds (compile-time error).
    OutOfBounds,
    /// Not a compile-time constant — lower as a runtime value.
    NotConstant,
}

impl<'a> FunctionLower<'a> {

    /// Emit one leaf store at (element `i`, leaf `j`) honouring the
    /// binding's layout — DATA-ORIENTED's two indexings:
    ///
    /// ```text
    /// AoS: leaf_idx = i * leaf_count + j   (one interleaved slot)
    /// SoA: slot = columns[j], index = i    (one slot per column)
    /// ```
    ///
    /// Both shapes hand codegen an ordinary `ArrayStore` against a
    /// scalar `elem_ty`, which is why nothing downstream changes.
    pub(super) fn emit_array_leaf_store(
        &mut self,
        storage: &ArrayStorage,
        leaf_count: usize,
        i: usize,
        j: usize,
        value: ValueId,
        leaf_ty: Type,
    ) {
        let (slot, leaf_idx) = match storage {
            ArrayStorage::Interleaved(slot) => (*slot, i * leaf_count + j),
            ArrayStorage::Columns(cols) => (cols[j], i),
        };
        let idx_v = self
            .emit(
                InstKind::Const(Const::U64(leaf_idx as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        self.emit(
            InstKind::ArrayStore {
                slot,
                index: idx_v,
                value,
                elem_ty: leaf_ty,
            },
            None,
        );
    }
    /// The guarded element index for an array access, constant-folded
    /// when it can be.
    pub(super) fn lower_element_index(
        &mut self,
        index_ref: &ExprRef,
        length: usize,
    ) -> Result<ValueId, String> {
        match self.resolve_const_index(index_ref, length) {
            ConstIndex::Valid(i) => Ok(self
                .emit(InstKind::Const(Const::U64(i as u64)), Some(Type::U64))
                .expect("Const returns a value")),
            ConstIndex::OutOfBounds => {
                Err(format!("array index out of bounds (length {length})"))
            }
            ConstIndex::NotConstant => {
                let raw_idx = self
                    .lower_expr(index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
                let idx_ty = self.value_scalar(index_ref).unwrap_or(Type::U64);
                self.emit_index_guard(index_ref, raw_idx, idx_ty, length)
            }
        }
    }

    /// Prepare to address every leaf of one element, hoisting the
    /// part they share.
    ///
    /// Under SoA a leaf is its own column at the element index, so
    /// there is nothing to share. Under AoS the leaf index is
    /// `i * leaf_count + j`: a constant when `i` folded, otherwise
    /// one multiply that every leaf then offsets from — computed
    /// here, once, rather than per leaf.
    pub(super) fn begin_leaf_addressing(
        &mut self,
        storage: &ArrayStorage,
        leaf_count: usize,
        elem_idx: ValueId,
        const_elem_idx: Option<usize>,
    ) -> LeafAddressing {
        let aos_base = match (storage, const_elem_idx) {
            (ArrayStorage::Interleaved(_), None) => {
                let leaf_count_v = self
                    .emit(InstKind::Const(Const::U64(leaf_count as u64)), Some(Type::U64))
                    .expect("Const returns a value");
                Some(
                    self.emit(
                        InstKind::BinOp { op: BinOp::Mul, lhs: elem_idx, rhs: leaf_count_v },
                        Some(Type::U64),
                    )
                    .expect("imul returns"),
                )
            }
            _ => None,
        };
        LeafAddressing {
            storage: storage.clone(),
            leaf_count,
            elem_idx,
            const_elem_idx,
            aos_base,
        }
    }

    /// The `(slot, index)` an `ArrayLoad` / `ArrayStore` of leaf `j`
    /// addresses. Both directions go through here, so a read and the
    /// write it mirrors cannot drift apart.
    pub(super) fn leaf_target(
        &mut self,
        addressing: &LeafAddressing,
        j: usize,
    ) -> (crate::ir::ArraySlotId, ValueId) {
        match &addressing.storage {
            ArrayStorage::Columns(cols) => (cols[j], addressing.elem_idx),
            ArrayStorage::Interleaved(slot) => {
                let index = match (addressing.const_elem_idx, addressing.aos_base, j) {
                    (Some(i), _, _) => self
                        .emit(
                            InstKind::Const(
                                Const::U64((i * addressing.leaf_count + j) as u64),
                            ),
                            Some(Type::U64),
                        )
                        .expect("Const returns a value"),
                    (None, Some(base), 0) => base,
                    (None, Some(base), _) => {
                        let off_v = self
                            .emit(InstKind::Const(Const::U64(j as u64)), Some(Type::U64))
                            .expect("Const returns");
                        self.emit(
                            InstKind::BinOp { op: BinOp::Add, lhs: base, rhs: off_v },
                            Some(Type::U64),
                        )
                        .expect("iadd returns")
                    }
                    (None, None, _) => {
                        unreachable!("AoS runtime path always computes the element base")
                    }
                };
                (*slot, index)
            }
        }
    }

    /// Determine the IR `Type` of an array element from its first
    /// literal. Scalars use `value_scalar`; struct / tuple literals
    /// resolve via `infer_tuple_element_type` (which already handles
    /// both, including interning new tuple shapes).
    pub(super) fn infer_array_element_type(
        &mut self,
        expr_ref: &ExprRef,
        annotation: Option<&frontend::type_decl::TypeDecl>,
    ) -> Result<Type, String> {
        if let Some(t) = self.infer_tuple_element_type(expr_ref) {
            return Ok(t);
        }
        // DATA-ORIENTED Phase 3: a generic enum element names its
        // enum but not its instantiation (`Option::Some(1i64)` could
        // be an `Option<i64>` or, one day, an `Option<T>` under a
        // substitution), so the annotation decides — the same rule
        // `val x: Option<i64> = Option::Some(1i64)` follows.
        if let Some(annotation) = annotation
            && let Some(id) = self.lower_type_arg(annotation)
        {
            return Ok(id);
        }
        Err("compiler MVP could not infer type for array element".to_string())
    }

    /// Lower one element value into the array's backing storage at
    /// the right leaf position. Scalar elements take a single
    /// `ArrayStore`; struct / tuple elements decompose into per-leaf
    /// `ArrayStore`s — the slot + index pair per leaf comes from the
    /// binding's layout (`emit_array_leaf_store`).
    pub(super) fn store_array_element(
        &mut self,
        storage: &ArrayStorage,
        elem_ty: Type,
        index: usize,
        leaf_count: usize,
        expr_ref: &ExprRef,
    ) -> Result<(), String> {
        match elem_ty {
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                let expr = self
                    .program
                    .expression
                    .get(expr_ref)
                    .ok_or_else(|| "array element missing".to_string())?;
                match expr {
                    Expr::StructLiteral(name, literal_fields) => {
                        let expected = self.module.struct_def(struct_id).base_name;
                        if name != expected {
                            return Err(format!(
                                "array element struct name mismatch: expected `{}`, got `{}`",
                                self.interner.resolve(expected).unwrap_or("?"),
                                self.interner.resolve(name).unwrap_or("?"),
                            ));
                        }
                        self.store_struct_literal_fields(
                            struct_id,
                            &fields,
                            &literal_fields,
                        )?;
                    }
                    _ => {
                        return Err(
                            "compiler MVP only supports struct-literal array elements (bind to val first)"
                                .to_string(),
                        );
                    }
                }
                let leaves = flatten_struct_locals(&fields);
                for (j, (local, ty)) in leaves.iter().enumerate() {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit_array_leaf_store(storage, leaf_count, index, j, v, *ty);
                }
                Ok(())
            }
            // DATA-ORIENTED Phase 3: an enum element goes through the
            // same three steps as a struct — allocate the destination
            // shape, lower the value into it, then push every leaf
            // into the array. `lower_into_enum_storage` is the same
            // helper a `val s: Shape = Shape::Circle(2i64)` binding
            // uses, so a variant written into an array and one
            // written into a local are built identically.
            Type::Enum(enum_id) => {
                let value = self.allocate_enum_storage(enum_id);
                self.lower_into_enum_storage(expr_ref, &value)?;
                let leaves = super::bindings::flatten_enum_storage_locals(&value);
                for (j, (local, ty)) in leaves.iter().enumerate() {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit_array_leaf_store(storage, leaf_count, index, j, v, *ty);
                }
                Ok(())
            }
            Type::Tuple(tuple_id) => {
                // Tuple element: same shape as struct, just routed
                // through `allocate_tuple_elements` /
                // `flatten_tuple_element_locals`.
                let elements = self.allocate_tuple_elements(tuple_id)?;
                let expr = self
                    .program
                    .expression
                    .get(expr_ref)
                    .ok_or_else(|| "array element missing".to_string())?;
                match expr {
                    Expr::TupleLiteral(literal_elems) => {
                        if literal_elems.len() != elements.len() {
                            return Err(format!(
                                "array element tuple length mismatch: expected {}, got {}",
                                elements.len(),
                                literal_elems.len(),
                            ));
                        }
                        for (j, e) in literal_elems.iter().enumerate() {
                            let shape = elements[j].shape.clone();
                            self.store_value_into_tuple_element_shape(e, j, &shape)?;
                        }
                    }
                    _ => {
                        return Err(
                            "compiler MVP only supports tuple-literal array elements".to_string(),
                        );
                    }
                }
                let leaves = flatten_tuple_element_locals(&elements);
                for (j, (local, ty)) in leaves.iter().enumerate() {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit_array_leaf_store(storage, leaf_count, index, j, v, *ty);
                }
                Ok(())
            }
            _ => {
                let v = self.lower_expr(expr_ref)?.ok_or_else(|| {
                    format!("array element #{index} produced no value")
                })?;
                self.emit_array_leaf_store(storage, leaf_count, index, 0, v, elem_ty);
                Ok(())
            }
        }
    }

    /// RUNTIME-TRAP: bounds-check a **runtime** array index and return
    /// the index to use for the access.
    ///
    /// Constant indices are folded (and rejected at compile time) by
    /// `resolve_const_index`; this covers the values that only exist at
    /// run time, which previously reached `ArrayLoad` / `ArrayStore`
    /// unchecked. Unchecked, `arr[10u64]` on a 3-element array read
    /// whatever followed the backing slot: the AOT binary printed a
    /// stack address and exited 0, and the IR VM raised an internal
    /// "value not defined" error rather than a toylang panic.
    ///
    /// A signed index is first adjusted the way the tree-walker adjusts
    /// it — `arr[-1i64]` is the last element — so all four engines agree
    /// on negative runtime indices as they already do on negative
    /// constant ones. After adjustment a still-negative index is out of
    /// bounds, which is why the signed path needs the second
    /// comparison; on the unsigned path the single `idx < length` test
    /// is sufficient.
    pub(super) fn emit_index_guard(
        &mut self,
        index_ref: &ExprRef,
        idx: ValueId,
        idx_ty: Type,
        length: usize,
    ) -> Result<ValueId, String> {
        // CONTRACT-ELISION: a `requires i < 8u64` on an `[T; 8]` states
        // exactly what this guard would test, and it was already
        // checked on entry. Unsigned needs nothing more; a signed
        // index additionally needs `requires i >= 0` (or a chain of
        // `>=` facts proving it), because the negative-adjustment path
        // below — which this elision removes along with the guard — is
        // otherwise still live.
        if let Some(sym) = self.parameter_name(index_ref)
            && self.facts.is_below(sym, length as u128)
            && (!idx_ty.is_signed() || self.facts.is_nonneg(sym))
        {
            return Ok(idx);
        }
        let Some(len_const) = Const::from_usize_in(idx_ty, length) else {
            // Either a non-integer index (the type checker rejects
            // those) or a length too large for the index type, in
            // which case every value of that type is in bounds.
            return Ok(idx);
        };
        let zero_const = Const::zero(idx_ty).expect("integer type has a zero");
        let mut idx = idx;
        if idx_ty.is_signed() {
            let local = self.module.function_mut(self.func_id).add_local(idx_ty);
            self.emit(InstKind::StoreLocal { dst: local, src: idx }, None);
            let zero = self
                .emit(InstKind::Const(zero_const), Some(idx_ty))
                .expect("Const returns a value");
            let is_neg = self
                .emit(
                    InstKind::BinOp { op: BinOp::Lt, lhs: idx, rhs: zero },
                    Some(Type::Bool),
                )
                .expect("BinOp returns a value");
            let adjust = self.fresh_block();
            let merge = self.fresh_block();
            self.terminate(Terminator::Branch {
                cond: is_neg,
                then_blk: adjust,
                else_blk: merge,
            });
            self.switch_to(adjust);
            let len_v = self
                .emit(InstKind::Const(len_const), Some(idx_ty))
                .expect("Const returns a value");
            let adjusted = self
                .emit(
                    InstKind::BinOp { op: BinOp::Add, lhs: idx, rhs: len_v },
                    Some(idx_ty),
                )
                .expect("BinOp returns a value");
            self.emit(InstKind::StoreLocal { dst: local, src: adjusted }, None);
            self.terminate(Terminator::Jump(merge));
            self.switch_to(merge);
            idx = self
                .emit(InstKind::LoadLocal(local), Some(idx_ty))
                .expect("LoadLocal returns a value");
            let zero = self
                .emit(InstKind::Const(zero_const), Some(idx_ty))
                .expect("Const returns a value");
            let non_negative = self
                .emit(
                    InstKind::BinOp { op: BinOp::Ge, lhs: idx, rhs: zero },
                    Some(Type::Bool),
                )
                .expect("BinOp returns a value");
            // The length is materialised before the guard so both
            // out-of-range directions report the same pair of numbers.
            let len_v = self
                .emit(InstKind::Const(len_const), Some(idx_ty))
                .expect("Const returns a value");
            self.emit_trap_values_unless(
                non_negative,
                crate::ir::panic_kind::INDEX_OUT_OF_BOUNDS,
                idx,
                len_v,
            );
        }
        let len_v = self
            .emit(InstKind::Const(len_const), Some(idx_ty))
            .expect("Const returns a value");
        let in_bounds = self
            .emit(
                InstKind::BinOp { op: BinOp::Lt, lhs: idx, rhs: len_v },
                Some(Type::Bool),
            )
            .expect("BinOp returns a value");
        self.emit_trap_values_unless(
            in_bounds,
            crate::ir::panic_kind::INDEX_OUT_OF_BOUNDS,
            idx,
            len_v,
        );
        Ok(idx)
    }

    /// Lower `arr[index]`. Phase S only handles single-element
    /// access on a bare identifier bound to an array, with a
    /// constant index folding to a direct LoadLocal on the matching
    /// per-element local. Range slicing and runtime indices are
    /// rejected for now.
    pub(super) fn lower_slice_access(
        &mut self,
        obj: &ExprRef,
        info: &frontend::ast::SliceInfo,
    ) -> Result<Option<ValueId>, String> {
        if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
            return Err(
                "compiler MVP only supports single-element array access (`arr[i]`); range slicing is not implemented".to_string(),
            );
        }
        let index_ref = info
            .start
            .as_ref()
            .ok_or_else(|| "single-element slice missing index".to_string())?;
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "array-access object missing".to_string())?;
        let arr_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports array access on a bare identifier".to_string(),
                );
            }
        };
        // POINTER P2: a struct / enum binding is not an array — `p[i]`
        // is a `__getitem__` call in disguise. Route through the
        // regular method-call machinery so monomorphisation,
        // contracts and `&mut self` writeback behave exactly like the
        // same call written by hand. The tree-walker has always
        // dispatched this way (slice.rs), so this is the compiled
        // lanes catching up, not a new semantics.
        // CONST-ARRAY: a `const K: [T; N] = [..]` is not a binding —
        // it is bytes in the read-only section, and an index is one
        // load from them. Ahead of the `__getitem__` detour because a
        // const array has no methods to dispatch to.
        if !matches!(self.bindings.get(&arr_sym), Some(Binding::Array { .. }))
            && let Some(array) = self.const_arrays.get(&arr_sym)
        {
            let (elem_ty, stride, length, bytes) = (
                array.elem_ty,
                array.stride,
                array.length,
                array.bytes.clone(),
            );
            // The same index path a stack array takes: constants are
            // folded and rejected at compile time, a runtime index is
            // adjusted for a negative value and bounds-checked. A
            // `const` table reads out of bounds exactly as loudly as
            // any other array.
            let idx = self.lower_element_index(index_ref, length as usize)?;
            let base = self
                .emit(InstKind::ConstBytesAddr { bytes }, Some(Type::U64))
                .ok_or_else(|| "const array address returned no value".to_string())?;
            let stride_v = self
                .emit(InstKind::Const(Const::U64(stride)), Some(Type::U64))
                .ok_or_else(|| "const array stride returned no value".to_string())?;
            let offset = self
                .emit(
                    InstKind::BinOp {
                        op: crate::ir::BinOp::Mul,
                        lhs: idx,
                        rhs: stride_v,
                    },
                    Some(Type::U64),
                )
                .ok_or_else(|| "const array offset returned no value".to_string())?;
            return Ok(self.emit(
                InstKind::PtrRead {
                    ptr: base,
                    offset,
                    elem_ty,
                },
                Some(elem_ty),
            ));
        }
        if !matches!(
            self.bindings.get(&arr_sym),
            Some(Binding::Array { .. })
        ) {
            let getitem_sym = self
                .interner
                .get("__getitem__")
                .ok_or_else(|| format!("`{}` is not an array binding and no `__getitem__` method exists", self.interner.resolve(arr_sym).unwrap_or("?")))?;
            let args = vec![*index_ref];
            return self.lower_method_call(obj, getitem_sym, &args);
        }
        let (element_ty, length, storage) = match self.bindings.get(&arr_sym).cloned() {
            Some(Binding::Array { element_ty, length, storage }) => {
                (element_ty, length, storage)
            }
            Some(_) | None => unreachable!("non-array binding was routed to __getitem__ above"),
        };
        // For compound array elements (struct), allocate a fresh
        // struct binding and load each leaf scalar into the
        // matching local. The result flows through the
        // `pending_struct_value` channel so chain access /
        // tail-position reads pick it up. For scalar elements,
        // emit a single `ArrayLoad` and return the resulting
        // value as before.
        let leaf_count = leaf_scalar_count(self.module, element_ty);
        if matches!(element_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
            // Allocate the right binding shape, then load each leaf
            // scalar into its local via per-leaf `ArrayLoad`. The
            // result flows through `pending_struct_value` /
            // `pending_tuple_value` so the val rhs path / chain
            // access can bind it.
            let leaves: Vec<(LocalId, Type)>;
            let pending_struct: Option<Vec<FieldBinding>>;
            let pending_tuple: Option<Vec<TupleElementBinding>>;
            let pending_enum: Option<super::bindings::EnumStorage>;
            match element_ty {
                Type::Struct(struct_id) => {
                    let fields = self.allocate_struct_fields(struct_id);
                    leaves = flatten_struct_locals(&fields);
                    pending_struct = Some(fields);
                    pending_tuple = None;
                    pending_enum = None;
                }
                Type::Tuple(tuple_id) => {
                    let elements = self.allocate_tuple_elements(tuple_id)?;
                    leaves = flatten_tuple_element_locals(&elements);
                    pending_struct = None;
                    pending_tuple = Some(elements);
                    pending_enum = None;
                }
                // DATA-ORIENTED Phase 3: the tag and every variant's
                // payload, in the one order the whole lowering agrees
                // on. Reading fills the inactive variants' slots with
                // whatever the array holds there — harmless, and the
                // same rule `__builtin_ptr_read` follows for an enum:
                // the tag decides which slots a `match` looks at.
                Type::Enum(enum_id) => {
                    let storage = self.allocate_enum_storage(enum_id);
                    leaves = super::bindings::flatten_enum_storage_locals(&storage);
                    pending_struct = None;
                    pending_tuple = None;
                    pending_enum = Some(storage);
                }
                _ => unreachable!(),
            }
            // The guarded element index, lowered once and shared by
            // every leaf. DATA-ORIENTED: under SoA each leaf's load
            // uses this value directly against its own column slot;
            // under AoS it is the element base the per-leaf offsets
            // add onto.
            let const_elem_idx = match self.resolve_const_index(index_ref, length) {
                ConstIndex::Valid(i) => Some(i),
                ConstIndex::OutOfBounds => {
                    return Err(format!("array index out of bounds (length {length})"));
                }
                ConstIndex::NotConstant => None,
            };
            let elem_idx_v = match const_elem_idx {
                Some(i) => self
                    .emit(InstKind::Const(Const::U64(i as u64)), Some(Type::U64))
                    .expect("Const returns a value"),
                None => {
                    let raw_idx = self
                        .lower_expr(index_ref)?
                        .ok_or_else(|| "array index produced no value".to_string())?;
                    let idx_ty = self.value_scalar(index_ref).unwrap_or(Type::U64);
                    self.emit_index_guard(index_ref, raw_idx, idx_ty, length)?
                }
            };
            let addressing =
                self.begin_leaf_addressing(&storage, leaf_count, elem_idx_v, const_elem_idx);
            for (j, (local, ty)) in leaves.iter().enumerate() {
                let (load_slot, leaf_idx_v) = self.leaf_target(&addressing, j);
                let v = self
                    .emit(
                        InstKind::ArrayLoad {
                            slot: load_slot,
                            index: leaf_idx_v,
                            elem_ty: *ty,
                        },
                        Some(*ty),
                    )
                    .expect("ArrayLoad returns");
                self.emit(InstKind::StoreLocal { dst: *local, src: v }, None);
            }
            self.pending_struct_value = pending_struct;
            self.pending_tuple_value = pending_tuple;
            self.pending_enum_value = pending_enum;
            return Ok(None);
        }
        // Scalar element path. Constant index folds into a Const at
        // compile time; anything else lowers as a runtime value.
        // Both forms hit the same `ArrayLoad` instruction so codegen
        // treats them uniformly. Constant-index out-of-bounds is
        // caught here.
        let idx_v = match self.resolve_const_index(index_ref, length) {
            ConstIndex::Valid(i) => self
                .emit(InstKind::Const(Const::U64(i as u64)), Some(Type::U64))
                .expect("Const returns a value"),
            ConstIndex::OutOfBounds => {
                return Err(format!("array index out of bounds (length {length})"));
            }
            ConstIndex::NotConstant => {
                let raw_idx = self
                    .lower_expr(index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
                let idx_ty = self.value_scalar(index_ref).unwrap_or(Type::U64);
                self.emit_index_guard(index_ref, raw_idx, idx_ty, length)?
            }
        };
        Ok(self.emit(
            InstKind::ArrayLoad {
                slot: storage.scalar_slot(),
                index: idx_v,
                elem_ty: element_ty,
            },
            Some(element_ty),
        ))
    }

    /// Lower `arr[i] = v`. Phase S supports single-element write on
    /// a bare-identifier array binding with a constant index. Range
    /// assignment is rejected.
    pub(super) fn lower_slice_assign(
        &mut self,
        obj: &ExprRef,
        start: Option<&ExprRef>,
        end: Option<&ExprRef>,
        value: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        if end.is_some() {
            return Err(
                "compiler MVP only supports single-element array write (`arr[i] = v`); range assignment is not implemented".to_string(),
            );
        }
        let index_ref = start
            .ok_or_else(|| "single-element slice write missing index".to_string())?;
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "array-write object missing".to_string())?;
        let arr_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(
                    "compiler MVP only supports array write on a bare identifier".to_string(),
                );
            }
        };
        // POINTER P2: `p[i] = v` on a struct / enum binding is a
        // `__setitem__` call — route through the regular method-call
        // machinery (`&mut self` writeback included), exactly like
        // the tree-walker's dispatch.
        if !matches!(
            self.bindings.get(&arr_sym),
            Some(Binding::Array { .. })
        ) {
            let setitem_sym = self
                .interner
                .get("__setitem__")
                .ok_or_else(|| format!("`{}` is not an array binding and no `__setitem__` method exists", self.interner.resolve(arr_sym).unwrap_or("?")))?;
            let args = vec![*index_ref, *value];
            return self.lower_method_call(obj, setitem_sym, &args);
        }
        let (element_ty, length, storage) = match self.bindings.get(&arr_sym).cloned() {
            Some(Binding::Array { element_ty, length, storage }) => {
                (element_ty, length, storage)
            }
            Some(_) | None => unreachable!("non-array binding was routed to __setitem__ above"),
        };
        // DATA-ORIENTED Phase 3: an enum element *must* be written
        // whole. A struct's leaves each have a name to assign through
        // (`ps[i].x = v`), but a variant is not a set of fields —
        // without this, an array of enums would be write-once.
        //
        // The cost is the one DATA-ORIENTED's undecided point 3
        // names: under SoA these stores scatter across the columns.
        // That is inherent to writing a whole element by column, and
        // it is what the layout trades for the reads.
        if let Type::Enum(enum_id) = element_ty {
            let leaf_count = leaf_scalar_count(self.module, element_ty);
            let value_storage = self.allocate_enum_storage(enum_id);
            self.lower_into_enum_storage(value, &value_storage)?;
            let leaves = super::bindings::flatten_enum_storage_locals(&value_storage);
            let const_elem_idx = match self.resolve_const_index(index_ref, length) {
                ConstIndex::Valid(i) => Some(i),
                ConstIndex::OutOfBounds => {
                    return Err(format!("array index out of bounds (length {length})"));
                }
                ConstIndex::NotConstant => None,
            };
            let elem_idx_v = self.lower_element_index(index_ref, length)?;
            let addressing =
                self.begin_leaf_addressing(&storage, leaf_count, elem_idx_v, const_elem_idx);
            for (j, (local, ty)) in leaves.iter().enumerate() {
                let v = self
                    .emit(InstKind::LoadLocal(*local), Some(*ty))
                    .expect("LoadLocal returns a value");
                let (slot, index) = self.leaf_target(&addressing, j);
                self.emit(
                    InstKind::ArrayStore { slot, index, value: v, elem_ty: *ty },
                    None,
                );
            }
            return Ok(None);
        }
        // Compound-element whole writes (`ps[i] = p`) are not
        // supported for structs and tuples — the value graph never
        // carries a compound, and their leaves can be written one at
        // a time by name instead.
        if leaf_scalar_count(self.module, element_ty) != 1 {
            return Err(
                "compiler MVP cannot write a whole compound array element (`ps[i] = p`); write individual leaves via `ps[i].field = v`".to_string(),
            );
        }
        let slot = storage.scalar_slot();
        let idx_v = match self.resolve_const_index(index_ref, length) {
            ConstIndex::Valid(i) => self
                .emit(InstKind::Const(Const::U64(i as u64)), Some(Type::U64))
                .expect("Const returns a value"),
            ConstIndex::OutOfBounds => {
                return Err(format!("array index out of bounds (length {length})"));
            }
            ConstIndex::NotConstant => {
                let raw_idx = self
                    .lower_expr(index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
                let idx_ty = self.value_scalar(index_ref).unwrap_or(Type::U64);
                self.emit_index_guard(index_ref, raw_idx, idx_ty, length)?
            }
        };
        let v = self
            .lower_expr(value)?
            .ok_or_else(|| "array write rhs produced no value".to_string())?;
        self.emit(
            InstKind::ArrayStore { slot, index: idx_v, value: v, elem_ty: element_ty },
            None,
        );
        Ok(None)
    }

    /// Fold a literal-integer index into a `usize`. Currently
    /// accepts `Int64` / `UInt64` / `Number` literals only;
    /// arbitrary const-expression folding is deferred.
    pub(super) fn try_constant_index(&self, expr_ref: &ExprRef) -> Option<usize> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::UInt64(v) => Some(v as usize),
            Expr::Int64(v) if v >= 0 => Some(v as usize),
            Expr::Number(_) => {
                // `Number` is a type-unspecified literal — usually
                // emitted as u64 by the parser when no suffix is
                // present. Fall back to a u64 view.
                None
            }
            Expr::Identifier(sym) => self.const_values.get(&sym).and_then(|c| match c {
                Const::U64(v) => Some(*v as usize),
                Const::I64(v) if *v >= 0 => Some(*v as usize),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Resolve a constant array index against a known `length`, applying
    /// Python-style negative indexing (`-1` → last element, `arr[len + i]`).
    /// Mirrors the tree-walker so `a[-1i64]` / `a[-2i64]` work on all
    /// backends; a constant out-of-bounds (positive or negative) is reported
    /// so the caller emits the same compile-time error positive OOB already
    /// produces. Returns `NotConstant` for runtime indices.
    pub(super) fn resolve_const_index(&self, expr_ref: &ExprRef, length: usize) -> ConstIndex {
        let Some(raw) = self.const_signed_index(expr_ref) else {
            return ConstIndex::NotConstant;
        };
        let adjusted = if raw < 0 { raw + length as i128 } else { raw };
        if adjusted >= 0 && (adjusted as u128) < length as u128 {
            ConstIndex::Valid(adjusted as usize)
        } else {
            ConstIndex::OutOfBounds
        }
    }

    /// Fold a constant index expression to a signed `i128`, accepting
    /// `Int64` / `UInt64` / `Number` literals, `-<literal>`
    /// (`Unary::Negate`), and top-level `const` references.
    fn const_signed_index(&self, expr_ref: &ExprRef) -> Option<i128> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::UInt64(v) => Some(v as i128),
            Expr::Int64(v) => Some(v as i128),
            Expr::Number(sym) => self.interner.resolve(sym)?.parse::<i128>().ok(),
            Expr::Unary(UnaryOp::Negate, inner) => self.const_signed_index(&inner).map(|x| -x),
            Expr::Identifier(sym) => self.const_values.get(&sym).and_then(|c| match c {
                Const::U64(v) => Some(*v as i128),
                Const::I64(v) => Some(*v as i128),
                _ => None,
            }),
            _ => None,
        }
    }
}
