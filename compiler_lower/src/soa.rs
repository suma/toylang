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
