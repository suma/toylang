//! `print` / `println` lowering: scalar values, string literals,
//! identifiers referring to compound bindings, struct / tuple / enum
//! literals, calls returning compound values, and method calls
//! returning compound values. Compound values are emitted as an
//! interleaved sequence of `PrintRaw` (punctuation + field labels)
//! and `Print` (leaf scalars), matching the interpreter's
//! `Object::to_display_string` output byte-for-byte. Format helpers
//! live here too so the display path is self-contained.

use frontend::ast::{Expr, ExprRef};

use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, EnumStorage, FieldBinding,
    FieldShape, PayloadSlot, TupleElementBinding, TupleElementShape,
};
use super::FunctionLower;
use crate::ir::{
    ArraySlotId, BinOp, Const, EnumId, InstKind, LocalId, StructId, Terminator, Type, ValueId,
};

impl<'a> FunctionLower<'a> {
    /// `print(x)` and `println(x)` accept a primitive scalar value, a
    /// string literal, or — via decomposition through the binding table —
    /// an identifier that refers to a struct or tuple `val` / `var`.
    /// Compound values are emitted as an interleaved sequence of
    /// `PrintRaw` (punctuation + field labels) and `Print` (leaf
    /// scalars), matching the interpreter's `to_display_string` format
    /// (`Point { x: 3, y: 4 }`, `(3, 4)`, with struct fields sorted
    /// alphabetically). Anything else (struct literals in expression
    /// position, function-returning struct/tuple values, dicts,
    /// allocators, ...) is deferred.
    pub(super) fn lower_print(
        &mut self,
        args: &Vec<ExprRef>,
        newline: bool,
    ) -> Result<Option<ValueId>, String> {
        if args.len() != 1 {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} expects 1 argument, got {}", args.len()));
        }
        // Special-case string-literal arguments before evaluating the
        // expression so we route them through the dedicated `PrintStr`
        // instruction (avoiding a `Type::Str` value flow).
        if let Some(Expr::String(sym)) = self.program.expression.get(&args[0]) {
            self.emit(InstKind::PrintStr { message: sym, newline }, None);
            return Ok(None);
        }
        // Struct- and tuple-typed identifier arguments: read the
        // binding shape and emit a formatted multi-call sequence. We
        // restrict to identifier expressions because the IR does not
        // carry struct / tuple values in its SSA graph, so there is
        // no way to print an arbitrary compound expression without
        // first storing it into a binding.
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(&args[0]) {
            if let Some(binding) = self.bindings.get(&sym).cloned() {
                match binding {
                    Binding::Struct { struct_id, fields } => {
                        self.emit_print_struct(struct_id, &fields, newline);
                        return Ok(None);
                    }
                    Binding::Tuple { elements } => {
                        self.emit_print_tuple(&elements, newline);
                        return Ok(None);
                    }
                    Binding::Scalar { .. } => {}
                    Binding::Enum(storage) => {
                        self.emit_print_enum(&storage, newline)?;
                        return Ok(None);
                    }
                    Binding::Array { element_ty, length, slot } => {
                        self.emit_print_array(element_ty, length, slot, newline);
                        return Ok(None);
                    }
                }
            }
        }
        // Compound-literal shortcuts: `print(Point { ... })`,
        // `print((1, 2))`, `print(Color::Red)`,
        // `print(Shape::Circle(5))`. We allocate scratch locals for
        // the value, store / construct it, then route through the
        // same `emit_print_*` helpers as for identifier bindings.
        // Generic struct / enum literals still need an enclosing
        // `val` annotation (no annotation hint reaches this path).
        if let Some(arg_expr) = self.program.expression.get(&args[0]) {
            match arg_expr {
                Expr::StructLiteral(struct_name, literal_fields) => {
                    let struct_id =
                        self.resolve_struct_instance(struct_name, None)?;
                    let fields = self.allocate_struct_fields(struct_id);
                    self.store_struct_literal_fields(
                        struct_id,
                        &fields,
                        &literal_fields,
                    )?;
                    self.emit_print_struct(struct_id, &fields, newline);
                    return Ok(None);
                }
                Expr::TupleLiteral(elems) => {
                    let mut elements: Vec<TupleElementBinding> =
                        Vec::with_capacity(elems.len());
                    for (i, e) in elems.iter().enumerate() {
                        let ty = self.value_scalar(e).ok_or_else(|| {
                            format!("tuple element #{i} has no inferable type")
                        })?;
                        let shape = self.allocate_tuple_element_shape(ty)?;
                        elements.push(TupleElementBinding { index: i, shape });
                    }
                    for (i, e) in elems.iter().enumerate() {
                        let shape = elements[i].shape.clone();
                        self.store_value_into_tuple_element_shape(e, i, &shape)?;
                    }
                    self.emit_print_tuple(&elements, newline);
                    return Ok(None);
                }
                Expr::QualifiedIdentifier(path)
                    if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
                {
                    let enum_id = self.resolve_enum_instance(path[0], None)?;
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
                    let storage = self.allocate_enum_storage(enum_id);
                    self.write_variant_into_storage(&storage, variant_idx, &[])?;
                    self.emit_print_enum(&storage, newline)?;
                    return Ok(None);
                }
                Expr::Call(fn_name, args_ref)
                    if self
                        .module
                        .lookup_function(None, fn_name)
                        .map(|id| {
                            let ret = self.module.function(id).return_type;
                            matches!(ret, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_))
                        })
                        .unwrap_or(false) =>
                {
                    // Compound-returning function call. Allocate a
                    // scratch binding to receive the result via the
                    // matching CallStruct / CallTuple / CallEnum
                    // (same shape `lower_let` uses), then dispatch
                    // to the corresponding `emit_print_*` helper.
                    let target_id = self.module.lookup_function(None, fn_name).unwrap();
                    let target_ret = self.module.function(target_id).return_type;
                    let arg_values = self.lower_call_args(&args_ref)?;
                    match target_ret {
                        Type::Struct(struct_id) => {
                            let fields = self.allocate_struct_fields(struct_id);
                            let dests: Vec<LocalId> =
                                flatten_struct_locals(&fields)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.emit(
                                InstKind::CallStruct {
                                    target: target_id,
                                    args: arg_values,
                                    dests,
                                },
                                None,
                            );
                            self.emit_print_struct(struct_id, &fields, newline);
                        }
                        Type::Tuple(tuple_id) => {
                            let elements = self.allocate_tuple_elements(tuple_id)?;
                            let dests: Vec<LocalId> =
                                flatten_tuple_element_locals(&elements)
                                    .into_iter()
                                    .map(|(l, _)| l)
                                    .collect();
                            self.emit(
                                InstKind::CallTuple {
                                    target: target_id,
                                    args: arg_values,
                                    dests,
                                },
                                None,
                            );
                            self.emit_print_tuple(&elements, newline);
                        }
                        Type::Enum(enum_id) => {
                            let storage = self.allocate_enum_storage(enum_id);
                            let dests = Self::flatten_enum_dests(&storage);
                            self.emit(
                                InstKind::CallEnum {
                                    target: target_id,
                                    args: arg_values,
                                    dests,
                                },
                                None,
                            );
                            self.emit_print_enum(&storage, newline)?;
                        }
                        _ => unreachable!("guard ensured compound return"),
                    }
                    return Ok(None);
                }
                Expr::MethodCall(recv, method_sym, method_args) => {
                    // Try the compound-returning method path. If the
                    // receiver / method resolves and the return type
                    // is compound, route through the matching
                    // `emit_print_*` after a CallStruct/Tuple/Enum
                    // into a scratch binding. Falls through to the
                    // generic value_scalar+Print path otherwise (so
                    // scalar-returning methods still work).
                    let recv_expr =
                        self.program.expression.get(&recv).ok_or_else(|| {
                            "method-call receiver missing".to_string()
                        })?;
                    let recv_sym = match recv_expr {
                        Expr::Identifier(s) => Some(s),
                        _ => None,
                    };
                    if let Some(rs) = recv_sym {
                        if let Some(binding) = self.bindings.get(&rs).cloned() {
                            let target_sym_opt = match &binding {
                                Binding::Struct { struct_id, .. } => Some(
                                    self.module.struct_def(*struct_id).base_name,
                                ),
                                Binding::Enum(storage) => Some(
                                    self.module.enum_def(storage.enum_id).base_name,
                                ),
                                _ => None,
                            };
                            if let Some(target_sym) = target_sym_opt {
                                let target_id = self
                                    .method_func_ids
                                    .get(&(target_sym, method_sym))
                                    .copied();
                                if let Some(target_id) = target_id {
                                    let target_ret =
                                        self.module.function(target_id).return_type;
                                    if matches!(
                                        target_ret,
                                        Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)
                                    ) {
                                        // Build call args: receiver leaf scalars first.
                                        let mut all_args: Vec<ValueId> = Vec::new();
                                        match &binding {
                                            Binding::Struct { fields, .. } => {
                                                for (local, ty) in
                                                    flatten_struct_locals(fields)
                                                {
                                                    let v = self
                                                        .emit(
                                                            InstKind::LoadLocal(local),
                                                            Some(ty),
                                                        )
                                                        .expect("LoadLocal returns");
                                                    all_args.push(v);
                                                }
                                            }
                                            Binding::Enum(storage) => {
                                                let storage = storage.clone();
                                                let vs = self.load_enum_locals(&storage);
                                                all_args.extend(vs);
                                            }
                                            _ => unreachable!(),
                                        }
                                        for a in &method_args {
                                            let v = self.lower_expr(a)?.ok_or_else(
                                                || {
                                                    "method argument produced no value"
                                                        .to_string()
                                                },
                                            )?;
                                            all_args.push(v);
                                        }
                                        match target_ret {
                                            Type::Struct(struct_id) => {
                                                let fields =
                                                    self.allocate_struct_fields(struct_id);
                                                let dests: Vec<LocalId> =
                                                    flatten_struct_locals(&fields)
                                                        .into_iter()
                                                        .map(|(l, _)| l)
                                                        .collect();
                                                self.emit(
                                                    InstKind::CallStruct {
                                                        target: target_id,
                                                        args: all_args,
                                                        dests,
                                                    },
                                                    None,
                                                );
                                                self.emit_print_struct(
                                                    struct_id, &fields, newline,
                                                );
                                            }
                                            Type::Tuple(tuple_id) => {
                                                let elements =
                                                    self.allocate_tuple_elements(tuple_id)?;
                                                let dests: Vec<LocalId> =
                                                    flatten_tuple_element_locals(&elements)
                                                        .into_iter()
                                                        .map(|(l, _)| l)
                                                        .collect();
                                                self.emit(
                                                    InstKind::CallTuple {
                                                        target: target_id,
                                                        args: all_args,
                                                        dests,
                                                    },
                                                    None,
                                                );
                                                self.emit_print_tuple(
                                                    &elements, newline,
                                                );
                                            }
                                            Type::Enum(enum_id) => {
                                                let storage =
                                                    self.allocate_enum_storage(enum_id);
                                                let dests =
                                                    Self::flatten_enum_dests(&storage);
                                                self.emit(
                                                    InstKind::CallEnum {
                                                        target: target_id,
                                                        args: all_args,
                                                        dests,
                                                    },
                                                    None,
                                                );
                                                self.emit_print_enum(&storage, newline)?;
                                            }
                                            _ => unreachable!(),
                                        }
                                        return Ok(None);
                                    }
                                }
                            }
                        }
                    }
                    let _ = method_args;
                }
                Expr::AssociatedFunctionCall(enum_name, variant_name, ctor_args)
                    if self.enum_defs.contains_key(&enum_name) =>
                {
                    let enum_id = self.resolve_enum_instance_with_args(
                        enum_name,
                        variant_name,
                        &ctor_args,
                        None,
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
                    let expected =
                        enum_def.variants[variant_idx].payload_types.len();
                    if ctor_args.len() != expected {
                        return Err(format!(
                            "enum variant `{}::{}` expects {} payload value(s), got {}",
                            self.interner.resolve(enum_name).unwrap_or("?"),
                            self.interner.resolve(variant_name).unwrap_or("?"),
                            expected,
                            ctor_args.len(),
                        ));
                    }
                    let storage = self.allocate_enum_storage(enum_id);
                    self.write_variant_into_storage(&storage, variant_idx, &ctor_args)?;
                    self.emit_print_enum(&storage, newline)?;
                    return Ok(None);
                }
                _ => {}
            }
        }
        let value_ty = self.value_scalar(&args[0]).ok_or_else(|| {
            let kw = if newline { "println" } else { "print" };
            format!(
                "{kw} accepts only scalar values (i64 / u64 / bool / f64), \
                 string literals, or identifiers referring to struct / tuple bindings \
                 in this compiler MVP"
            )
        })?;
        if matches!(value_ty, Type::Unit) {
            let kw = if newline { "println" } else { "print" };
            return Err(format!("{kw} cannot print a Unit value"));
        }
        let v = self
            .lower_expr(&args[0])?
            .ok_or_else(|| "print argument produced no value".to_string())?;
        self.emit(
            InstKind::Print {
                value: v,
                value_ty,
                newline,
            },
            None,
        );
        Ok(None)
    }

    /// Emit the `Name { field: value, ... }` rendering for a struct
    /// binding. Field order matches the interpreter's
    /// `Object::to_display_string`: alphabetical by name. Nested struct
    /// fields recurse; scalar fields go through a single `Print`.
    /// Only the very last fragment carries the caller's `newline`
    /// flag, so `print` vs `println` differs by exactly one helper
    /// choice.
    pub(super) fn emit_print_struct(
        &mut self,
        struct_id: StructId,
        fields: &[FieldBinding],
        newline: bool,
    ) {
        // Format the struct's display header. Generic instantiations
        // append a `<T1, T2, ...>` suffix so the user can tell
        // `Cell<u64>` apart from `Cell<i64>` in print output;
        // non-generic structs render as before (`Point { x: 3, y: 4 }`).
        let header = self.format_struct_header(struct_id);
        self.emit_print_raw_text(format!("{header} {{ "), false);
        let mut sorted: Vec<&FieldBinding> = fields.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for (i, fb) in sorted.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            self.emit_print_raw_text(format!("{}: ", fb.name), false);
            match &fb.shape {
                FieldShape::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                FieldShape::Struct {
                    struct_id: nested_id,
                    fields: nested,
                } => {
                    self.emit_print_struct(*nested_id, nested, false);
                }
                FieldShape::Tuple { elements, .. } => {
                    self.emit_print_tuple(elements, false);
                }
            }
        }
        self.emit_print_raw_text(" }".to_string(), newline);
    }

    /// Emit the `(a, b, ...)` rendering for a tuple binding. Single-
    /// element tuples render as `(a,)` to disambiguate from a
    /// parenthesised expression, matching the interpreter.
    pub(super) fn emit_print_tuple(&mut self, elements: &[TupleElementBinding], newline: bool) {
        self.emit_print_raw_text("(".to_string(), false);
        for (i, el) in elements.iter().enumerate() {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            match &el.shape {
                TupleElementShape::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                TupleElementShape::Struct { struct_id, fields } => {
                    let fields = fields.clone();
                    self.emit_print_struct(*struct_id, &fields, false);
                }
                TupleElementShape::Tuple { elements: inner, .. } => {
                    let inner = inner.clone();
                    self.emit_print_tuple(&inner, false);
                }
            }
        }
        // `(x,)` for the 1-tuple case.
        if elements.len() == 1 {
            self.emit_print_raw_text(",".to_string(), false);
        }
        self.emit_print_raw_text(")".to_string(), newline);
    }

    pub(super) fn emit_print_raw_text(&mut self, text: String, newline: bool) {
        self.emit(InstKind::PrintRaw { text, newline }, None);
    }

    /// Render an array binding as `[a, b, c]`, matching the
    /// interpreter's `to_display_string` format for `Object::Array`.
    /// Element type is uniform across the binding (Phase S enforces
    /// this at construction time).
    pub(super) fn emit_print_array(
        &mut self,
        element_ty: Type,
        length: usize,
        slot: ArraySlotId,
        newline: bool,
    ) {
        self.emit_print_raw_text("[".to_string(), false);
        for i in 0..length {
            if i > 0 {
                self.emit_print_raw_text(", ".to_string(), false);
            }
            let idx_v = self
                .emit(InstKind::Const(Const::U64(i as u64)), Some(Type::U64))
                .expect("Const returns a value");
            let v = self
                .emit(
                    InstKind::ArrayLoad { slot, index: idx_v, elem_ty: element_ty },
                    Some(element_ty),
                )
                .expect("ArrayLoad returns a value");
            self.emit(
                InstKind::Print {
                    value: v,
                    value_ty: element_ty,
                    newline: false,
                },
                None,
            );
        }
        self.emit_print_raw_text("]".to_string(), newline);
    }

    /// Render a struct's display header (`Name` or `Name<T1, T2, ...>`)
    /// for `print` / `println`. Generic instantiations include the
    /// concrete type-argument list so callers can tell `Cell<u64>`
    /// apart from `Cell<i64>` in stdout. Type args are themselves
    /// rendered through `format_type_for_display`, recursing into
    /// nested generic struct / enum types.
    pub(super) fn format_struct_header(&self, struct_id: StructId) -> String {
        let def = self.module.struct_def(struct_id);
        let base = self.interner.resolve(def.base_name).unwrap_or("?");
        if def.type_args.is_empty() {
            base.to_string()
        } else {
            let parts: Vec<String> = def
                .type_args
                .iter()
                .map(|t| self.format_type_for_display(*t))
                .collect();
            format!("{}<{}>", base, parts.join(", "))
        }
    }

    /// Same shape as `format_struct_header` for an enum instance —
    /// `Name` for non-generic, `Name<T1, ...>` for generic. Used as
    /// the prefix in `Name<T>::Variant` enum print output.
    pub(super) fn format_enum_header(&self, enum_id: EnumId) -> String {
        let def = self.module.enum_def(enum_id);
        let base = self.interner.resolve(def.base_name).unwrap_or("?");
        if def.type_args.is_empty() {
            base.to_string()
        } else {
            let parts: Vec<String> = def
                .type_args
                .iter()
                .map(|t| self.format_type_for_display(*t))
                .collect();
            format!("{}<{}>", base, parts.join(", "))
        }
    }

    /// Human-readable rendering of an IR `Type` for display headers.
    /// Scalars resolve to their canonical names (`i64` / `u64` / ...),
    /// struct / enum types resolve through their base name + recursive
    /// type-arg list, tuples render structurally as `(t1, t2, ...)`.
    pub(super) fn format_type_for_display(&self, t: Type) -> String {
        match t {
            Type::I64 => "i64".to_string(),
            Type::U64 => "u64".to_string(),
            Type::F64 => "f64".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Unit => "()".to_string(),
            Type::Struct(id) => self.format_struct_header(id),
            Type::Enum(id) => self.format_enum_header(id),
            Type::Tuple(id) => {
                let parts: Vec<String> = self
                    .module
                    .tuple_defs
                    .get(id.0 as usize)
                    .map(|elems| {
                        elems.iter().map(|e| self.format_type_for_display(*e)).collect()
                    })
                    .unwrap_or_default();
                format!("({})", parts.join(", "))
            }
            Type::Str => "str".to_string(),
        }
    }

    /// Emit the `Enum::Variant` / `Enum::Variant(p0, p1, ...)` rendering
    /// for an enum binding, matching `Object::to_display_string` in
    /// the interpreter. Tag dispatch happens at runtime via a brif
    /// chain (the last variant is the unconditional fallback so we
    /// only emit `n - 1` comparisons). Each per-variant block writes
    /// its own fragments and then jumps to a single merge block where
    /// the print sequence ends.
    pub(super) fn emit_print_enum(
        &mut self,
        storage: &EnumStorage,
        newline: bool,
    ) -> Result<(), String> {
        let enum_def = self.module.enum_def(storage.enum_id).clone();
        // Generic enum instantiations include the type-arg list in
        // the print prefix so `Option<i64>::Some(5)` is visually
        // distinguishable from `Option<u64>::Some(5)`. Non-generic
        // enums render as before (`Color::Red`).
        let enum_str = self.format_enum_header(storage.enum_id);
        let n_variants = enum_def.variants.len();
        if n_variants == 0 {
            // No variants — this enum can never be constructed, so
            // there's nothing sensible to print. Treat as a no-op
            // rather than crashing.
            return Ok(());
        }
        let merge = self.fresh_block();
        let tag_v = self
            .emit(InstKind::LoadLocal(storage.tag_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        for (idx, variant) in enum_def.variants.iter().enumerate() {
            let variant_str = self
                .interner
                .resolve(variant.name)
                .unwrap_or("?")
                .to_string();
            let body_blk = self.fresh_block();
            let slots = storage.payloads[idx].clone();
            if idx + 1 < n_variants {
                let next = self.fresh_block();
                let want = self
                    .emit(
                        InstKind::Const(Const::U64(idx as u64)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                let cond = self
                    .emit(
                        InstKind::BinOp {
                            op: BinOp::Eq,
                            lhs: tag_v,
                            rhs: want,
                        },
                        Some(Type::Bool),
                    )
                    .expect("Eq returns a value");
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: body_blk,
                    else_blk: next,
                });
                self.switch_to(body_blk);
                self.emit_print_enum_variant_body(
                    &enum_str,
                    &variant_str,
                    &slots,
                    newline,
                )?;
                self.terminate(Terminator::Jump(merge));
                self.switch_to(next);
            } else {
                // Last variant: unconditional fallthrough. The
                // type-checker has already verified that `tag_v`
                // can only hold one of the known indices, so no
                // panic block is needed here.
                self.terminate(Terminator::Jump(body_blk));
                self.switch_to(body_blk);
                self.emit_print_enum_variant_body(
                    &enum_str,
                    &variant_str,
                    &slots,
                    newline,
                )?;
                self.terminate(Terminator::Jump(merge));
            }
        }
        self.switch_to(merge);
        Ok(())
    }

    /// Emit the body of one enum variant's print path — the literal
    /// `EnumName::VariantName` plus, for tuple variants, a parenthesised
    /// comma-separated list of payload values. The `newline` flag rides
    /// the *last* fragment so `print` and `println` differ only in one
    /// helper choice (matches the struct / tuple print pattern).
    /// Recurses into nested enum payloads so `Some(Some(5))` prints
    /// the inner value through the same dispatch.
    pub(super) fn emit_print_enum_variant_body(
        &mut self,
        enum_str: &str,
        variant_str: &str,
        payload_slots: &[PayloadSlot],
        newline: bool,
    ) -> Result<(), String> {
        let header = format!("{enum_str}::{variant_str}");
        let unit = payload_slots.is_empty();
        // For unit variants, the variant header is the only thing we
        // emit — apply the trailing newline directly to it.
        self.emit(
            InstKind::PrintRaw {
                text: header,
                newline: unit && newline,
            },
            None,
        );
        if unit {
            return Ok(());
        }
        self.emit(
            InstKind::PrintRaw {
                text: "(".to_string(),
                newline: false,
            },
            None,
        );
        let last_idx = payload_slots.len() - 1;
        for (i, slot) in payload_slots.iter().enumerate() {
            if i > 0 {
                self.emit(
                    InstKind::PrintRaw {
                        text: ", ".to_string(),
                        newline: false,
                    },
                    None,
                );
            }
            match slot {
                PayloadSlot::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::Print {
                            value: v,
                            value_ty: *ty,
                            newline: false,
                        },
                        None,
                    );
                }
                PayloadSlot::Enum(inner) => {
                    let inner = (**inner).clone();
                    self.emit_print_enum(&inner, false)?;
                }
                PayloadSlot::Struct { struct_id, fields } => {
                    let fields = fields.clone();
                    self.emit_print_struct(*struct_id, &fields, false);
                }
                PayloadSlot::Tuple { elements, .. } => {
                    let elements = elements.clone();
                    self.emit_print_tuple(&elements, false);
                }
            }
            let _ = last_idx;
        }
        self.emit(
            InstKind::PrintRaw {
                text: ")".to_string(),
                newline,
            },
            None,
        );
        Ok(())
    }
}
