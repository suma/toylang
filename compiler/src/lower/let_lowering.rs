//! `val` / `var` declaration lowering.
//!
//! `lower_let` is the centralised binding-shape picker. It
//! peeks at the rhs expression to decide what kind of binding
//! to allocate, then evaluates the rhs into that binding:
//!
//! - struct literal rhs -> `Binding::Struct` (one local per
//!   field, field tree allocated up front).
//! - tuple literal rhs -> `Binding::Tuple` (per-element shape
//!   allocated up front).
//! - enum literal rhs -> `Binding::Enum` (tag + payload tree
//!   allocated via `allocate_enum_storage`).
//! - array literal rhs -> `Binding::Array` (stack slot of the
//!   correct stride sized from `infer_array_element_type`).
//! - range slice rhs (`arr[start..end]`) -> sliced array shape
//!   with leaf-index addressing (constant-bound only for now).
//! - scalar / call / method-call / field-access rhs ->
//!   `Binding::Scalar`. Compound-returning calls allocate the
//!   appropriate `Struct` / `Tuple` / `Enum` shape and use the
//!   pre-allocated storage path.
//!
//! Re-binding (`var p = q` where `q` is itself a struct /
//! tuple / enum binding) deep-copies the source storage into a
//! freshly-allocated target via the corresponding `copy_*`
//! helper.

use frontend::ast::{Expr, ExprRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::array_layout::{elem_stride_bytes, leaf_scalar_count, leaf_type_at};
use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{Const, InstKind, LocalId, Type, ValueId};

impl<'a> FunctionLower<'a> {
    /// Centralised val/var-with-rhs handling. Picks the binding shape
    /// from the rhs expression: a struct literal allocates a struct
    /// binding (one local per field); anything else allocates a single
    /// scalar local. Anything more exotic (e.g. assigning a struct
    /// value returned from a function) is rejected for the MVP.
    pub(super) fn lower_let(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let rhs = self
            .program
            .expression
            .get(rhs_ref)
            .ok_or_else(|| "let rhs missing".to_string())?;
        // Tuple-literal RHS: allocate one local per element. Like
        // structs, tuples never flow through the IR's value graph;
        // the only way to consume one is via `t.N` element access on a
        // bound name. The parser desugars `val (a, b) = e` into
        // `val tmp = e; val a = tmp.0; val b = tmp.1`, so this branch
        // also handles destructuring.
        if let Expr::TupleLiteral(elems) = rhs.clone() {
            let mut bindings: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
            // Pre-allocate locals so element-rhs evaluation order
            // doesn't matter. For nested tuple / struct elements
            // (`((a, b), c)`, `(Point, i64)`) we recurse into the
            // literal to determine the type and intern any new
            // tuple shapes along the way.
            for (i, elem_ref) in elems.iter().enumerate() {
                let elem_ty = self
                    .infer_tuple_element_type(elem_ref)
                    .ok_or_else(|| {
                        format!(
                            "compiler MVP could not infer type for tuple element #{i}"
                        )
                    })?;
                let shape = self.allocate_tuple_element_shape(elem_ty)?;
                bindings.push(TupleElementBinding { index: i, shape });
            }
            self.bindings.insert(
                name,
                Binding::Tuple {
                    elements: bindings.clone(),
                },
            );
            // Evaluate and store each element's value. Scalar
            // elements take the fast path; compound elements (struct
            // / nested tuple) route through the same helpers used
            // for enum-payload slots.
            for (i, elem_ref) in elems.iter().enumerate() {
                let shape = bindings[i].shape.clone();
                self.store_value_into_tuple_element_shape(elem_ref, i, &shape)?;
            }
            return Ok(None);
        }
        // Array-literal RHS. Phase S supports a fixed-size array of
        // scalars: `val arr = [a, b, c]`. Each element gets its own
        // local; access happens via `arr[const_idx]` (constant
        // indices only — runtime indexing would require a
        // stack-allocated buffer).
        // Range-slice array read: `val sub = arr[start..end]`.
        // Phase Y2 supports constant bounds only — both endpoints
        // must fold via `try_constant_index`. The result is a fresh
        // fixed-length array binding whose stack slot mirrors the
        // source slot's leaf layout. Each leaf scalar is copied with
        // an `ArrayLoad` + `ArrayStore` pair.
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone() {
            if matches!(info.slice_type, frontend::ast::SliceType::RangeSlice) {
                let arr_expr = self
                    .program
                    .expression
                    .get(&arr_obj)
                    .ok_or_else(|| "array-access object missing".to_string())?;
                let arr_sym = match arr_expr {
                    Expr::Identifier(s) => s,
                    _ => {
                        return Err(
                            "compiler MVP only supports range slicing on a bare identifier"
                                .to_string(),
                        );
                    }
                };
                let (element_ty, length, src_slot) = match self.bindings.get(&arr_sym).cloned() {
                    Some(Binding::Array { element_ty, length, slot }) => {
                        (element_ty, length, slot)
                    }
                    _ => {
                        return Err(format!(
                            "`{}` is not an array binding",
                            self.interner.resolve(arr_sym).unwrap_or("?")
                        ));
                    }
                };
                // Defaults for omitted endpoints follow the
                // interpreter: `..end` starts at 0, `start..` ends
                // at `length`, `..` is the whole array.
                let start = match info.start {
                    Some(s) => self.try_constant_index(&s).ok_or_else(|| {
                        "compiler MVP only supports constant range-slice bounds".to_string()
                    })?,
                    None => 0,
                };
                let end = match info.end {
                    Some(e) => self.try_constant_index(&e).ok_or_else(|| {
                        "compiler MVP only supports constant range-slice bounds".to_string()
                    })?,
                    None => length,
                };
                if start > end || end > length {
                    return Err(format!(
                        "range slice {start}..{end} out of bounds (array length {length})"
                    ));
                }
                let new_len = end - start;
                let leaf_count = leaf_scalar_count(self.module, element_ty);
                let stride = elem_stride_bytes(element_ty, self.module);
                let dst_slot = self
                    .module
                    .function_mut(self.func_id)
                    .add_array_slot(element_ty, new_len * leaf_count, stride);
                for i in 0..new_len {
                    for j in 0..leaf_count {
                        let src_idx = (start + i) * leaf_count + j;
                        let dst_idx = i * leaf_count + j;
                        let leaf_ty = leaf_type_at(self.module, element_ty, j);
                        let src_idx_v = self
                            .emit(
                                InstKind::Const(Const::U64(src_idx as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns");
                        let v = self
                            .emit(
                                InstKind::ArrayLoad {
                                    slot: src_slot,
                                    index: src_idx_v,
                                    elem_ty: leaf_ty,
                                },
                                Some(leaf_ty),
                            )
                            .expect("ArrayLoad returns");
                        let dst_idx_v = self
                            .emit(
                                InstKind::Const(Const::U64(dst_idx as u64)),
                                Some(Type::U64),
                            )
                            .expect("Const returns");
                        self.emit(
                            InstKind::ArrayStore {
                                slot: dst_slot,
                                index: dst_idx_v,
                                value: v,
                                elem_ty: leaf_ty,
                            },
                            None,
                        );
                    }
                }
                self.bindings.insert(
                    name,
                    Binding::Array {
                        element_ty,
                        length: new_len,
                        slot: dst_slot,
                    },
                );
                return Ok(None);
            }
        }
        // Compound-element array read: `val p: Point = arr[i]`.
        // Allocate the right binding shape and load each leaf
        // directly into its locals via the same per-leaf
        // ArrayLoad sequence `lower_slice_access` would emit, so
        // chain access (`p.x`) and field-by-field reads work
        // through the existing struct-binding path.
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone() {
            if matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                let arr_expr = self
                    .program
                    .expression
                    .get(&arr_obj)
                    .ok_or_else(|| "array-access object missing".to_string())?;
                if let Expr::Identifier(arr_sym) = arr_expr {
                    if let Some(Binding::Array { element_ty, .. }) =
                        self.bindings.get(&arr_sym).cloned()
                    {
                        match element_ty {
                            Type::Struct(struct_id) => {
                                // Lower the element read, which stashes
                                // a pending_struct_value with freshly
                                // allocated leaves filled in.
                                self.pending_struct_value = None;
                                let _ = self.lower_slice_access(&arr_obj, &info)?;
                                if let Some(fields) =
                                    self.pending_struct_value.take()
                                {
                                    self.bindings.insert(
                                        name,
                                        Binding::Struct { struct_id, fields },
                                    );
                                    return Ok(None);
                                }
                            }
                            Type::Tuple(_) => {
                                self.pending_tuple_value = None;
                                let _ = self.lower_slice_access(&arr_obj, &info)?;
                                if let Some(elements) =
                                    self.pending_tuple_value.take()
                                {
                                    self.bindings.insert(
                                        name,
                                        Binding::Tuple { elements },
                                    );
                                    return Ok(None);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Expr::ArrayLiteral(elems) = rhs.clone() {
            if elems.is_empty() {
                return Err(
                    "compiler MVP cannot infer element type for empty array literal".to_string(),
                );
            }
            // Element type comes from the first element. Scalars
            // resolve via `value_scalar`; struct literals resolve
            // through the struct table.
            let elem_ty = self.infer_array_element_type(&elems[0])?;
            if !matches!(
                elem_ty,
                Type::I64
                    | Type::U64
                    | Type::F64
                    | Type::Bool
                    | Type::I8
                    | Type::U8
                    | Type::I16
                    | Type::U16
                    | Type::I32
                    | Type::U32
                    | Type::Struct(_)
                    | Type::Tuple(_)
            ) {
                return Err(format!(
                    "compiler MVP only supports scalar / struct / tuple array elements; got {elem_ty:?}"
                ));
            }
            // For homogeneous scalar element arrays the stride is
            // the scalar's actual byte size (1/2/4/8 — see
            // `array_layout::elem_stride_bytes`); for compound
            // elements each leaf gets a uniform 8-byte slot in the
            // same buffer. The slot's `length` therefore counts
            // leaves, not elements.
            let leaf_count = leaf_scalar_count(self.module, elem_ty);
            let stride = elem_stride_bytes(elem_ty, self.module);
            let slot_len = elems.len() * leaf_count;
            let slot = self
                .module
                .function_mut(self.func_id)
                .add_array_slot(elem_ty, slot_len, stride);
            for (i, e) in elems.iter().enumerate() {
                self.store_array_element(slot, elem_ty, i, leaf_count, e)?;
            }
            self.bindings.insert(
                name,
                Binding::Array {
                    element_ty: elem_ty,
                    length: elems.len(),
                    slot,
                },
            );
            return Ok(None);
        }
        // Enum-construction RHS. `Enum::Variant` (unit) parses as a
        // `QualifiedIdentifier(vec![enum, variant])`; `Enum::Variant(args)`
        // parses as `AssociatedFunctionCall(enum, variant, args)`.
        // Either way the lowering allocates an `Enum` binding (tag local
        // + per-variant payload locals) and stores the chosen tag plus
        // the supplied arguments in this variant's payload slots.
        if let Expr::QualifiedIdentifier(path) = rhs.clone() {
            if path.len() == 2 {
                if self.enum_defs.contains_key(&path[0]) {
                    let enum_id = self.resolve_enum_instance(path[0], annotation)?;
                    let enum_def = self.module.enum_def(enum_id).clone();
                    let variant_idx = enum_def
                        .variants
                        .iter()
                        .position(|v| v.name == path[1])
                        .ok_or_else(|| {
                            format!(
                                "unknown enum variant `{}::{}`",
                                self.interner.resolve(path[0]).unwrap_or("?"),
                                self.interner.resolve(path[1]).unwrap_or("?"),
                            )
                        })?;
                    if !enum_def.variants[variant_idx].payload_types.is_empty() {
                        return Err(format!(
                            "enum variant `{}::{}` is a tuple variant; supply its arguments \
                             via `{}::{}(...)`",
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                            self.interner.resolve(path[0]).unwrap_or("?"),
                            self.interner.resolve(path[1]).unwrap_or("?"),
                        ));
                    }
                    self.bind_enum(name, enum_id, variant_idx, &[])?;
                    return Ok(None);
                }
            }
        }
        if let Expr::AssociatedFunctionCall(enum_name, variant_name, args) = rhs.clone() {
            if self.enum_defs.contains_key(&enum_name) {
                let enum_id = self.resolve_enum_instance_with_args(
                    enum_name,
                    variant_name,
                    &args,
                    annotation,
                )?;
                let enum_def = self.module.enum_def(enum_id).clone();
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == variant_name)
                    .ok_or_else(|| {
                        format!(
                            "unknown enum variant `{}::{}`",
                            self.interner.resolve(enum_name).unwrap_or("?"),
                            self.interner.resolve(variant_name).unwrap_or("?"),
                        )
                    })?;
                let expected = enum_def.variants[variant_idx].payload_types.len();
                if args.len() != expected {
                    return Err(format!(
                        "enum variant `{}::{}` expects {} payload value(s), got {}",
                        self.interner.resolve(enum_name).unwrap_or("?"),
                        self.interner.resolve(variant_name).unwrap_or("?"),
                        expected,
                        args.len(),
                    ));
                }
                self.bind_enum(name, enum_id, variant_idx, &args)?;
                return Ok(None);
            }
        }
        // Composite enum-producing RHS: `if`-chain / `match` / block
        // whose every branch ends in an enum construction or an enum
        // binding identifier of the same enum. Pre-allocate the
        // shared target locals once and have each branch write into
        // them; cranelift's `def_var` walk turns the per-branch
        // writes into proper SSA at the merge.
        if let Some(base_name) = self.detect_enum_result(rhs_ref) {
            let enum_id = self.resolve_enum_instance(base_name, annotation)?;
            let storage = self.allocate_enum_storage(enum_id);
            self.bindings
                .insert(name, Binding::Enum(storage.clone()));
            self.lower_into_enum_storage(rhs_ref, &storage)?;
            return Ok(None);
        }
        // Struct-literal RHS: allocate one local per field (recursing
        // into nested struct fields), evaluate each field expression,
        // store into the matching local. The IR layer never sees a
        // struct value — we decompose at the lowering boundary.
        if let Expr::StructLiteral(struct_name, fields) = rhs {
            // Resolve to the right monomorphised instance. Generic
            // structs need an annotation to pick T; non-generic
            // ones short-circuit to a single instance.
            let struct_id =
                self.resolve_struct_instance(struct_name, annotation)?;
            let field_bindings = self.allocate_struct_fields(struct_id);
            // Insert the binding before evaluating field rhs
            // expressions so an inner literal that walks back to the
            // same name (currently unsupported but defensive) doesn't
            // see a missing binding.
            self.bindings.insert(
                name,
                Binding::Struct {
                    struct_id,
                    fields: field_bindings.clone(),
                },
            );
            self.store_struct_literal_fields(struct_id, &field_bindings, &fields)?;
            return Ok(None);
        }
        // Compound-returning method call RHS: `val q = p.swap()`.
        // Resolves the receiver / method target the same way
        // `lower_method_call` does, then routes the multi-result
        // through `CallStruct` / `CallTuple` / `CallEnum` into a
        // freshly-allocated binding. Mirrors the per-target
        // branches below for plain function calls.
        if let Expr::MethodCall(recv, method_sym, method_args) = rhs.clone() {
            if let Some((target_id, recv_binding)) =
                self.resolve_method_target(&recv, method_sym, &method_args)?
            {
                let target_ret = self.module.function(target_id).return_type;
                if matches!(
                    target_ret,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)
                ) {
                    // Build the call args: receiver leaf scalars
                    // first, then method arguments (each lowered
                    // individually so identifier-arg expansion for
                    // struct / tuple / enum stays intact).
                    let mut all_args: Vec<ValueId> = Vec::new();
                    match &recv_binding {
                        Binding::Struct { fields, .. } => {
                            for (local, ty) in flatten_struct_locals(fields) {
                                let v = self
                                    .emit(InstKind::LoadLocal(local), Some(ty))
                                    .expect("LoadLocal returns");
                                all_args.push(v);
                            }
                        }
                        Binding::Enum(storage) => {
                            let storage = storage.clone();
                            let vs = self.load_enum_locals(&storage);
                            all_args.extend(vs);
                        }
                        _ => unreachable!(
                            "resolve_method_target only returns struct/enum receivers"
                        ),
                    }
                    for a in &method_args {
                        let v = self
                            .lower_expr(a)?
                            .ok_or_else(|| "method argument produced no value".to_string())?;
                        all_args.push(v);
                    }
                    match target_ret {
                        Type::Struct(struct_id) => {
                            let fields = self.allocate_struct_fields(struct_id);
                            let dests: Vec<LocalId> =
                                flatten_struct_locals(&fields)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.bindings.insert(
                                name,
                                Binding::Struct { struct_id, fields },
                            );
                            self.emit(
                                InstKind::CallStruct {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        Type::Tuple(tuple_id) => {
                            let elements = self.allocate_tuple_elements(tuple_id)?;
                            let dests: Vec<LocalId> =
                                flatten_tuple_element_locals(&elements)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.bindings.insert(
                                name,
                                Binding::Tuple { elements },
                            );
                            self.emit(
                                InstKind::CallTuple {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        Type::Enum(enum_id) => {
                            let storage = self.allocate_enum_storage(enum_id);
                            let dests = Self::flatten_enum_dests(&storage);
                            self.bindings.insert(name, Binding::Enum(storage));
                            self.emit(
                                InstKind::CallEnum {
                                    target: target_id,
                                    args: all_args,
                                    dests,
                                },
                                None,
                            );
                        }
                        _ => unreachable!("guard ensured compound return"),
                    }
                    return Ok(None);
                }
            }
        }
        // Tuple-returning call RHS: `val pair = make_pair()`. Same
        // shape as struct-returning calls, just routed through
        // CallTuple. Detect early so the parser-desugared
        // `val (a, b) = make_pair()` (which becomes
        // `val tmp = make_pair(); val a = tmp.0; val b = tmp.1`) is
        // also handled here without special-casing destructuring.
        if let Expr::Call(fn_name, args_ref) = rhs.clone() {
            if let Some(target_id) = self.module.lookup_function(None, fn_name) {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Tuple(tuple_id) = target_ret {
                    let element_bindings = self.allocate_tuple_elements(tuple_id)?;
                    let dests: Vec<LocalId> = flatten_tuple_element_locals(&element_bindings)
                        .into_iter()
                        .map(|(local, _)| local)
                        .collect();
                    self.bindings.insert(
                        name,
                        Binding::Tuple { elements: element_bindings },
                    );
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallTuple {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
                if let Type::Enum(enum_id) = target_ret {
                    // Enum-returning call: pre-allocate the binding's
                    // storage tree, flatten it into the CallEnum dest
                    // list (tag first, then each variant's payloads
                    // in declaration order, recursing through nested
                    // enum slots). Codegen then routes the multi-
                    // return slots straight into our locals.
                    let storage = self.allocate_enum_storage(enum_id);
                    let dests = Self::flatten_enum_dests(&storage);
                    self.bindings
                        .insert(name, Binding::Enum(storage));
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallEnum {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Struct-returning call RHS: `val p = make_point()`. Allocate
        // a struct binding and use `CallStruct` so codegen can route
        // the multi-return values into the per-field locals.
        if let Expr::Call(fn_name, args_ref) = rhs {
            if let Some(target_id) = self.module.lookup_function(None, fn_name) {
                let target_ret = self.module.function(target_id).return_type;
                if let Type::Struct(struct_id) = target_ret {
                    let field_bindings = self.allocate_struct_fields(struct_id);
                    // CallStruct dests are the leaf scalar locals in
                    // declaration order — exactly what the cranelift
                    // multi-result call gives us back.
                    let dests: Vec<LocalId> = flatten_struct_locals(&field_bindings)
                        .into_iter()
                        .map(|(l, _)| l)
                        .collect();
                    self.bindings.insert(
                        name,
                        Binding::Struct {
                            struct_id,
                            fields: field_bindings,
                        },
                    );
                    // Lower the args separately so we can hand them to
                    // `CallStruct` directly. The argument expressions
                    // themselves are scalar (struct args resolve via
                    // identifiers; cross-struct call args are handled by
                    // the regular `lower_call` path below if they show up
                    // in this position).
                    let arg_values = self.lower_call_args(&args_ref)?;
                    self.emit(
                        InstKind::CallStruct {
                            target: target_id,
                            args: arg_values,
                            dests,
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // #121 Phase A: `val name: T = __builtin_ptr_read(p, off)` —
        // the read width is taken from the annotation. Without this
        // intercept, lower_builtin_call's PtrRead arm rejects the
        // call with a clear error pointing at the missing type
        // hint, but a let-binding always supplies one.
        if let Expr::BuiltinCall(frontend::ast::BuiltinFunction::PtrRead, args) = rhs.clone() {
            if args.len() == 2 {
                let elem_ty = annotation.and_then(|a| {
                    super::types::lower_scalar(a)
                });
                if let Some(elem_ty) = elem_ty {
                    let ptr = self
                        .lower_expr(&args[0])?
                        .ok_or_else(|| "ptr_read ptr produced no value".to_string())?;
                    let offset = self
                        .lower_expr(&args[1])?
                        .ok_or_else(|| "ptr_read offset produced no value".to_string())?;
                    let v = self.emit(
                        InstKind::PtrRead { ptr, offset, elem_ty },
                        Some(elem_ty),
                    );
                    let local = self
                        .module
                        .function_mut(self.func_id)
                        .add_local(elem_ty);
                    self.bindings.insert(
                        name,
                        Binding::Scalar { local, ty: elem_ty },
                    );
                    self.emit(
                        InstKind::StoreLocal {
                            dst: local,
                            src: v.expect("PtrRead produces a value"),
                        },
                        None,
                    );
                    return Ok(None);
                }
            }
        }
        // Scalar fallback (existing behaviour).
        let v = self
            .lower_expr(rhs_ref)?
            .ok_or_else(|| "val/var rhs produced no value".to_string())?;
        let scalar = self
            .value_scalar(rhs_ref)
            .ok_or_else(|| "could not infer scalar type for val/var rhs".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(name, Binding::Scalar { local, ty: scalar });
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        Ok(None)
    }
}
