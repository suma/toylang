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
    flatten_struct_locals, flatten_tuple_element_locals, Binding, FieldBinding,
    TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{ArraySlotId, BinOp, Const, InstKind, LocalId, Terminator, Type, ValueId};

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

    /// Determine the IR `Type` of an array element from its first
    /// literal. Scalars use `value_scalar`; struct / tuple literals
    /// resolve via `infer_tuple_element_type` (which already handles
    /// both, including interning new tuple shapes).
    pub(super) fn infer_array_element_type(&mut self, expr_ref: &ExprRef) -> Result<Type, String> {
        if let Some(t) = self.infer_tuple_element_type(expr_ref) {
            return Ok(t);
        }
        Err("compiler MVP could not infer type for array element".to_string())
    }

    /// Lower one element value into the array's stack slot at the
    /// right leaf-index range. Scalar elements take a single
    /// `ArrayStore` at index `i * leaf_count + 0`; struct elements
    /// decompose into per-leaf `ArrayStore`s starting at
    /// `i * leaf_count`.
    pub(super) fn store_array_element(
        &mut self,
        slot: ArraySlotId,
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
                    let leaf_idx = index * leaf_count + j;
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
                            value: v,
                            elem_ty: *ty,
                        },
                        None,
                    );
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
                    let leaf_idx = index * leaf_count + j;
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
                            value: v,
                            elem_ty: *ty,
                        },
                        None,
                    );
                }
                Ok(())
            }
            _ => {
                let v = self.lower_expr(expr_ref)?.ok_or_else(|| {
                    format!("array element #{index} produced no value")
                })?;
                let leaf_idx = index * leaf_count;
                let idx_v = self
                    .emit(
                        InstKind::Const(Const::U64(leaf_idx as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                self.emit(
                    InstKind::ArrayStore { slot, index: idx_v, value: v, elem_ty },
                    None,
                );
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
    fn emit_index_guard(
        &mut self,
        idx: ValueId,
        idx_ty: Type,
        length: usize,
    ) -> Result<ValueId, String> {
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
            self.emit_trap_unless(non_negative, self.contract_msgs.index_out_of_bounds);
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
        self.emit_trap_unless(in_bounds, self.contract_msgs.index_out_of_bounds);
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
        let (element_ty, length, slot) = match self.bindings.get(&arr_sym).cloned() {
            Some(Binding::Array { element_ty, length, slot }) => (element_ty, length, slot),
            Some(_) => {
                return Err(format!(
                    "`{}` is not an array binding",
                    self.interner.resolve(arr_sym).unwrap_or("?")
                ));
            }
            None => {
                return Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(arr_sym).unwrap_or("?")
                ));
            }
        };
        // For compound array elements (struct), allocate a fresh
        // struct binding and load each leaf scalar into the
        // matching local. The result flows through the
        // `pending_struct_value` channel so chain access /
        // tail-position reads pick it up. For scalar elements,
        // emit a single `ArrayLoad` and return the resulting
        // value as before.
        let leaf_count = leaf_scalar_count(self.module, element_ty);
        if matches!(element_ty, Type::Struct(_) | Type::Tuple(_)) {
            // Allocate the right binding shape, then load each leaf
            // scalar into its local via per-leaf `ArrayLoad`. The
            // result flows through `pending_struct_value` /
            // `pending_tuple_value` so the val rhs path / chain
            // access can bind it.
            let leaves: Vec<(LocalId, Type)>;
            let pending_struct: Option<Vec<FieldBinding>>;
            let pending_tuple: Option<Vec<TupleElementBinding>>;
            match element_ty {
                Type::Struct(struct_id) => {
                    let fields = self.allocate_struct_fields(struct_id);
                    leaves = flatten_struct_locals(&fields);
                    pending_struct = Some(fields);
                    pending_tuple = None;
                }
                Type::Tuple(tuple_id) => {
                    let elements = self.allocate_tuple_elements(tuple_id)?;
                    leaves = flatten_tuple_element_locals(&elements);
                    pending_struct = None;
                    pending_tuple = Some(elements);
                }
                _ => unreachable!(),
            }
            // Element-base leaf index: const-fold or `imul(idx, leaf_count)`.
            let base_v = match self.resolve_const_index(index_ref, length) {
                ConstIndex::Valid(i) => self
                    .emit(
                        InstKind::Const(Const::U64((i * leaf_count) as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value"),
                ConstIndex::OutOfBounds => {
                    return Err(format!("array index out of bounds (length {length})"));
                }
                ConstIndex::NotConstant => {
                let raw_idx = self
                    .lower_expr(index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
                let idx_ty = self.value_scalar(index_ref).unwrap_or(Type::U64);
                let raw_idx = self.emit_index_guard(raw_idx, idx_ty, length)?;
                let leaf_count_v = self
                    .emit(
                        InstKind::Const(Const::U64(leaf_count as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                self.emit(
                    InstKind::BinOp {
                        op: BinOp::Mul,
                        lhs: raw_idx,
                        rhs: leaf_count_v,
                    },
                    Some(Type::U64),
                )
                .expect("imul returns")
                }
            };
            for (j, (local, ty)) in leaves.iter().enumerate() {
                let leaf_idx_v = if j == 0 {
                    base_v
                } else {
                    let off_v = self
                        .emit(
                            InstKind::Const(Const::U64(j as u64)),
                            Some(Type::U64),
                        )
                        .expect("Const returns");
                    self.emit(
                        InstKind::BinOp {
                            op: BinOp::Add,
                            lhs: base_v,
                            rhs: off_v,
                        },
                        Some(Type::U64),
                    )
                    .expect("iadd returns")
                };
                let v = self
                    .emit(
                        InstKind::ArrayLoad {
                            slot,
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
                self.emit_index_guard(raw_idx, idx_ty, length)?
            }
        };
        Ok(self.emit(
            InstKind::ArrayLoad { slot, index: idx_v, elem_ty: element_ty },
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
        let (element_ty, length, slot) = match self.bindings.get(&arr_sym).cloned() {
            Some(Binding::Array { element_ty, length, slot }) => (element_ty, length, slot),
            Some(_) => {
                return Err(format!(
                    "`{}` is not an array binding",
                    self.interner.resolve(arr_sym).unwrap_or("?")
                ));
            }
            None => {
                return Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(arr_sym).unwrap_or("?")
                ));
            }
        };
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
                self.emit_index_guard(raw_idx, idx_ty, length)?
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
