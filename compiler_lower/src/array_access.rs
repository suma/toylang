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

use frontend::ast::{Expr, ExprRef};

use super::array_layout::leaf_scalar_count;
use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, FieldBinding,
    TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{ArraySlotId, BinOp, Const, InstKind, LocalId, Type, ValueId};

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
            let base_v = if let Some(idx_const) = self.try_constant_index(index_ref) {
                if idx_const >= length {
                    return Err(format!(
                        "array index {idx_const} out of bounds (length {length})"
                    ));
                }
                self.emit(
                    InstKind::Const(Const::U64((idx_const * leaf_count) as u64)),
                    Some(Type::U64),
                )
                .expect("Const returns a value")
            } else {
                let raw_idx = self
                    .lower_expr(index_ref)?
                    .ok_or_else(|| "array index produced no value".to_string())?;
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
        let idx_v = if let Some(idx_const) = self.try_constant_index(index_ref) {
            if idx_const >= length {
                return Err(format!(
                    "array index {idx_const} out of bounds (length {length})"
                ));
            }
            self.emit(InstKind::Const(Const::U64(idx_const as u64)), Some(Type::U64))
                .expect("Const returns a value")
        } else {
            self.lower_expr(index_ref)?
                .ok_or_else(|| "array index produced no value".to_string())?
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
        let idx_v = if let Some(idx_const) = self.try_constant_index(index_ref) {
            if idx_const >= length {
                return Err(format!(
                    "array index {idx_const} out of bounds (length {length})"
                ));
            }
            self.emit(InstKind::Const(Const::U64(idx_const as u64)), Some(Type::U64))
                .expect("Const returns a value")
        } else {
            self.lower_expr(index_ref)?
                .ok_or_else(|| "array index produced no value".to_string())?
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
}
