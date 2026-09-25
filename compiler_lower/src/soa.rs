//! DATA-ORIENTED Phase 2 — the column arithmetic behind `SoaVec<T>`.
//!
//! `soa Vec<T>` desugars to the stdlib `SoaVec<T>`
//! (`core/std/collections/soa_vec.t`), whose buffer is one allocation
//! divided into one column per leaf scalar of `T`:
//!
//! ```text
//! buffer: [col_0 x cap][col_1 x cap] ... [col_{k-1} x cap]
//! leaf j of element i  ->  byte_off = prefix_j * cap + i * stride_j
//! ```
//!
//! `stride_j` is leaf `j`'s own width and `prefix_j` the sum of the
//! widths before it — both constants once `T` is monomorphised, with
//! only `cap` arriving at runtime. That is the whole reason these can
//! be builtins at all: leaf *selection* stays a compile-time decision,
//! unlike the `__builtin_field_offset(T, i)` reflection the design doc
//! rejects, whose index is a runtime value the
//! `__builtin_ptr_read`-style annotation convention cannot describe.
//!
//! Columns are derived from the same `compute_leaf_layout` the AoS
//! `__builtin_ptr_read` / `__builtin_ptr_write` expansion walks, so the
//! two layouts agree leaf-for-leaf: `prefix_j` *is* the AoS byte offset
//! of leaf `j`, and the columns' total is `__builtin_sizeof::<T>()`
//! times the capacity. A scalar `T` has one column with `prefix = 0`,
//! which collapses the formula to `i * sizeof(T)` — the AoS address,
//! as it must be (one leaf is one column; there is nothing to split).
//!
//! Both accessors expand into the ordinary `PtrRead` / `PtrWrite` the
//! IR already has, so codegen and the IR VM learn nothing about SoA —
//! the same property Phase 0 has at the stack-slot level.

use string_interner::DefaultSymbol;

use crate::ir::{InstKind, Type, ValueId};

use super::FunctionLower;

/// One column: the byte offset its elements start at (the prefix sum
/// of the widths before it), its per-element width, and its leaf type.
pub(super) type Column = (u64, u64, Type);

/// How a buffer addresses the leaves of element `i`.
///
/// `Interleaved` is the historical `__builtin_ptr_read` /
/// `__builtin_ptr_write` shape — the caller has already computed the
/// element's base byte offset (`i * elem_size`, usually), and each
/// leaf sits at a fixed distance after it. `Columns` is Phase 2's: the
/// element index and the capacity arrive separately because the stride
/// between two elements of one column is the *leaf's* width, not the
/// element's.
#[derive(Clone, Copy)]
pub(super) enum BufferAddress {
    Interleaved { base: ValueId },
    Columns { index: ValueId, cap: ValueId },
}

impl FunctionLower<'_> {
    /// The columns `ty` splits into, in the leaf order every other
    /// per-leaf walk in the lowering uses (struct fields / tuple
    /// elements in declaration order; an enum's tag then its
    /// variants' payloads).
    ///
    /// `None` when a leaf's width is unknown, which is the same
    /// condition that makes `compute_leaf_layout` fail.
    pub(super) fn soa_columns(&self, ty: Type) -> Option<Vec<Column>> {
        let leaves = self.compute_leaf_layout(ty)?;
        leaves
            .iter()
            .map(|(prefix, leaf_ty)| {
                self.compute_byte_size(*leaf_ty)
                    .map(|stride| (*prefix, stride, *leaf_ty))
            })
            .collect()
    }

    /// Lower a read/write call's address arguments.
    ///
    /// `args[1]` is the element's base byte offset for the
    /// interleaved builtins and the element *index* for the column
    /// ones, where `args[2]` carries the capacity the columns are
    /// spaced by.
    pub(super) fn lower_buffer_address(
        &mut self,
        args: &[crate::ExprRef],
        soa: bool,
    ) -> Result<BufferAddress, String> {
        if soa {
            let index = self
                .lower_expr(&args[1])?
                .ok_or_else(|| "soa index produced no value".to_string())?;
            let cap = self
                .lower_expr(&args[2])?
                .ok_or_else(|| "soa cap produced no value".to_string())?;
            Ok(BufferAddress::Columns { index, cap })
        } else {
            let base = self
                .lower_expr(&args[1])?
                .ok_or_else(|| "ptr offset produced no value".to_string())?;
            Ok(BufferAddress::Interleaved { base })
        }
    }

    /// The byte offset leaf `column` of one element sits at, under
    /// whichever addressing the buffer uses.
    ///
    /// The two layouts agree leaf-for-leaf — a column's `prefix` *is*
    /// the AoS byte offset of that leaf inside an element — so both
    /// the reads and the writes of both layouts come from this one
    /// place. That is what keeps a `soa_write` and the `soa_read`
    /// after it addressing the same byte; when the formula lived in
    /// four copies, a vec reading a neighbouring column was one typo
    /// away.
    pub(super) fn emit_leaf_offset(
        &mut self,
        address: &BufferAddress,
        column: &Column,
    ) -> ValueId {
        match *address {
            BufferAddress::Interleaved { base } => {
                let (prefix, _, _) = *column;
                // The first leaf sits at the element's own base.
                if prefix == 0 {
                    return base;
                }
                let prefix_v = self
                    .emit(InstKind::Const(crate::ir::Const::U64(prefix)), Some(Type::U64))
                    .expect("Const returns a value");
                self.emit(
                    InstKind::BinOp {
                        op: crate::ir::BinOp::Add,
                        lhs: base,
                        rhs: prefix_v,
                    },
                    Some(Type::U64),
                )
                .expect("BinOp returns a value")
            }
            BufferAddress::Columns { index, cap } => self.emit_soa_offset(column, index, cap),
        }
    }

    /// `prefix * cap + index * stride`, as IR values.
    ///
    /// The two algebraic identities are taken here rather than left to
    /// the folder: column 0 starts at the buffer's own base, so its
    /// `0 * cap` term is dropped, and a 1-byte leaf needs no scaling.
    /// Everything else is emitted as written — one formula, used by
    /// both the reads and the writes, because the two must address the
    /// same byte or a vec silently reads a neighbouring column.
    fn emit_soa_offset(&mut self, column: &Column, index: ValueId, cap: ValueId) -> ValueId {
        let (prefix, stride, _) = *column;
        let within_column = if stride == 1 {
            index
        } else {
            let stride_v = self
                .emit(InstKind::Const(crate::ir::Const::U64(stride)), Some(Type::U64))
                .expect("Const returns a value");
            self.emit(
                InstKind::BinOp { op: crate::ir::BinOp::Mul, lhs: index, rhs: stride_v },
                Some(Type::U64),
            )
            .expect("BinOp returns a value")
        };
        if prefix == 0 {
            return within_column;
        }
        let prefix_v = self
            .emit(InstKind::Const(crate::ir::Const::U64(prefix)), Some(Type::U64))
            .expect("Const returns a value");
        let column_base = self
            .emit(
                InstKind::BinOp { op: crate::ir::BinOp::Mul, lhs: prefix_v, rhs: cap },
                Some(Type::U64),
            )
            .expect("BinOp returns a value");
        self.emit(
            InstKind::BinOp {
                op: crate::ir::BinOp::Add,
                lhs: column_base,
                rhs: within_column,
            },
            Some(Type::U64),
        )
        .expect("BinOp returns a value")
    }
}

/// DATA-ORIENTED Phase 1: `ps.mass` — the column window.
///
/// A `Column<T>` (`core/std/column.t`) is three scalars: the address
/// the field's values start at, how many there are, and how far apart
/// they sit. Which is to say it is a *strided* window, and that is
/// what lets one type describe a column of either layout:
///
/// | layout | address of element 0's leaf `j` | stride |
/// |---|---|---|
/// | `soa` | column `j`'s own slot, offset 0 | that leaf's width |
/// | interleaved | the array's slot at leaf index `j` | one element's leaves |
///
/// The address is the reason this is compiler-side rather than a
/// library call: a stack array's storage has no source-level name.
/// Once built, everything the window does is ordinary stdlib toylang
/// reading through `__builtin_ptr_read`.
impl FunctionLower<'_> {
    /// The leaf index and IR type of `field` within `element_ty`, or
    /// `None` if the element is not a struct with that field.
    ///
    /// Counts *leaves*, not fields: a compound field ahead of the one
    /// asked for occupies as many columns as it has leaves, so
    /// `struct S { pos: Pos, mass: f64 }` puts `mass` at leaf 2.
    pub(super) fn column_leaf_of_field(
        &self,
        element_ty: Type,
        field: DefaultSymbol,
    ) -> Option<(usize, Type)> {
        let Type::Struct(struct_id) = element_ty else {
            return None;
        };
        // IR struct fields carry their names as text (the interner a
        // monomorph was built from is not this one), so the lookup
        // resolves the symbol rather than comparing ids.
        let wanted = self.interner.resolve(field)?.to_string();
        let fields = self.module.struct_def(struct_id).fields.clone();
        let mut leaf = 0usize;
        for (name, field_ty) in &fields {
            if *name == wanted {
                return Some((leaf, *field_ty));
            }
            leaf += super::array_layout::leaf_scalar_count(self.module, *field_ty);
        }
        None
    }

    /// `val ms = ps.mass` — bind a `Column<T>` over one field of every
    /// element.
    ///
    /// Returns `Ok(None)` when the field is not one of the element's
    /// (the caller then falls through to the ordinary field-access
    /// paths, so an array-typed *struct field* keeps working).
    pub(super) fn lower_let_column_window(
        &mut self,
        name: DefaultSymbol,
        element_ty: Type,
        length: usize,
        storage: &super::bindings::ArrayStorage,
        field: DefaultSymbol,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some((leaf, leaf_ty)) = self.column_leaf_of_field(element_ty, field) else {
            return Ok(None);
        };
        self.reject_compound_column(leaf_ty, field)?;
        let (slot, index, stride) = match storage {
            // One column per leaf: the window is that column's whole
            // slot, and its values are adjacent (Phase 0.5 sized each
            // column by its leaf).
            super::bindings::ArrayStorage::Columns(columns) => {
                let stride =
                    super::array_layout::elem_stride_bytes(leaf_ty, self.module) as u64;
                (columns[leaf], 0u64, stride)
            }
            // Interleaved: the slot is byte-addressed (NUM-W-AOT-pack
            // Phase 3), so the leaf sits `offsets[leaf]` bytes into
            // element 0 and the next element's copy is one packed
            // element further on.
            super::bindings::ArrayStorage::Interleaved(slot) => {
                let (unit, offsets) = self.interleaved_units(*slot);
                (*slot, offsets[leaf], unit)
            }
        };
        // `ArrayElemAddr` scales by the *element type it is given*, so
        // the interleaved case asks in whole leaf slots (`U64`, 8
        // bytes) rather than in the narrow leaf's own width.
        let addr_elem_ty = match storage {
            super::bindings::ArrayStorage::Columns(_) => leaf_ty,
            super::bindings::ArrayStorage::Interleaved(_) => Type::U64,
        };
        let index_v = self
            .emit(InstKind::Const(crate::ir::Const::U64(index)), Some(Type::U64))
            .expect("Const returns a value");
        let addr = self
            .emit(
                InstKind::ArrayElemAddr { slot, index: index_v, elem_ty: addr_elem_ty },
                Some(Type::U64),
            )
            .expect("ArrayElemAddr returns a value");
        let len_v = self
            .emit(
                InstKind::Const(crate::ir::Const::U64(length as u64)),
                Some(Type::U64),
            )
            .expect("Const returns a value");
        let stride_v = self
            .emit(InstKind::Const(crate::ir::Const::U64(stride)), Some(Type::U64))
            .expect("Const returns a value");
        self.bind_column(name, leaf_ty, addr, len_v, stride_v)?;
        Ok(Some(None))
    }

    /// `val ms = vs.mass` where `vs: SoaVec<T>` — the heap column
    /// window (DATA-ORIENTED Phase 1 over Phase 2's buffer).
    ///
    /// The buffer is one allocation split into columns, so column `j`
    /// starts `prefix_j * cap` bytes in and its values are `stride_j`
    /// apart — the same arithmetic `__builtin_soa_read` uses, minus
    /// the element index. Unlike the stack form, the address is a
    /// runtime value: `cap` is only known while the program runs.
    ///
    /// Returns `Ok(None)` unless the binding really is a `SoaVec`
    /// whose element has this field, so ordinary field access on any
    /// other struct falls through untouched.
    pub(super) fn lower_let_soa_vec_column(
        &mut self,
        name: DefaultSymbol,
        struct_id: crate::ir::StructId,
        fields: &[super::bindings::FieldBinding],
        field: DefaultSymbol,
    ) -> Result<Option<Option<ValueId>>, String> {
        let def = self.module.struct_def(struct_id).clone();
        if self.interner.resolve(def.base_name) != Some("SoaVec") {
            return Ok(None);
        }
        let Some(element_ty) = def.type_args.first().copied() else {
            return Ok(None);
        };
        let Some((leaf, leaf_ty)) = self.column_leaf_of_field(element_ty, field) else {
            return Ok(None);
        };
        self.reject_compound_column(leaf_ty, field)?;
        let columns = self.soa_columns(element_ty).ok_or_else(|| {
            format!(
                "column window: unable to compute the column layout for `{}`",
                crate::spelling::spell_type(self.module, self.interner, element_ty)
            )
        })?;
        let (prefix, stride, _) = columns[leaf];

        // `SoaVec { data, len, cap }` — the leaf locals are
        // in declaration order, as everywhere else in the lowering.
        let locals = super::bindings::flatten_struct_locals(fields);
        let data_local = locals.first().ok_or("column window: SoaVec has no data field")?.0;
        let len_local = locals.get(1).ok_or("column window: SoaVec has no len field")?.0;
        let cap_local = locals.get(2).ok_or("column window: SoaVec has no cap field")?.0;
        let data = self
            .emit(InstKind::LoadLocal(data_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let len = self
            .emit(InstKind::LoadLocal(len_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let cap = self
            .emit(InstKind::LoadLocal(cap_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let prefix_v = self
            .emit(InstKind::Const(crate::ir::Const::U64(prefix)), Some(Type::U64))
            .expect("Const returns a value");
        let column_offset = self
            .emit(
                InstKind::BinOp { op: crate::ir::BinOp::Mul, lhs: prefix_v, rhs: cap },
                Some(Type::U64),
            )
            .expect("BinOp returns a value");
        let addr = self
            .emit(
                InstKind::BinOp { op: crate::ir::BinOp::Add, lhs: data, rhs: column_offset },
                Some(Type::U64),
            )
            .expect("BinOp returns a value");
        let stride_v = self
            .emit(InstKind::Const(crate::ir::Const::U64(stride)), Some(Type::U64))
            .expect("Const returns a value");
        // The window is over the *live* elements, not the capacity:
        // `len` is what a caller may read.
        self.bind_column(name, leaf_ty, addr, len, stride_v)?;
        Ok(Some(None))
    }

    /// The lowering's half of the checker's rule: a window reads one
    /// value per stride, and a compound field occupies as many
    /// columns as it has leaves. The checker refuses these first (so
    /// every engine refuses the same program); this catches a field
    /// that only the lowering can see is compound.
    fn reject_compound_column(&self, leaf_ty: Type, field: DefaultSymbol) -> Result<(), String> {
        if matches!(leaf_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
            return Err(format!(
                "column window `{}`: a compound field occupies several columns",
                self.interner.resolve(field).unwrap_or("?")
            ));
        }
        Ok(())
    }

    /// Bind `name` to a `Column<T>` made of these three scalars.
    /// Shared by the stack and heap constructions, which differ only
    /// in how they arrive at them.
    fn bind_column(
        &mut self,
        name: DefaultSymbol,
        leaf_ty: Type,
        addr: ValueId,
        len: ValueId,
        stride: ValueId,
    ) -> Result<(), String> {
        let column_sym = self
            .interner
            .get("Column")
            .ok_or_else(|| "column window: the stdlib `Column<T>` is not loaded".to_string())?;
        let column_id = super::templates::instantiate_struct(
            self.module,
            self.struct_defs,
            self.enum_defs,
            column_sym,
            vec![leaf_ty],
            self.interner,
        )?;
        let fields = self.allocate_struct_fields(column_id);
        let locals = super::bindings::flatten_struct_locals(&fields);
        let values = [addr, len, stride];
        if locals.len() != values.len() {
            return Err(format!(
                "column window: `Column` should have {} scalar fields, found {}",
                values.len(),
                locals.len()
            ));
        }
        for ((local, _), value) in locals.iter().zip(values.iter()) {
            self.emit(InstKind::StoreLocal { dst: *local, src: *value }, None);
        }
        self.bindings
            .insert(name, super::bindings::Binding::Struct { struct_id: column_id, fields });
        Ok(())
    }
}
