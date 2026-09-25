//! Expression-side dispatchers and the call-arg expander.
//!
//! - `lower_call_args`: evaluate a call's argument list
//!   (`Expr::ExprList(items)`) into a flat `Vec<ValueId>`.
//!   Each argument is lowered through the regular expression
//!   path; struct- / tuple- / enum-typed identifier arguments
//!   are expanded into per-leaf values matching the callee
//!   signature.
//! - `lower_expr`: per-`Expr` switch. Routes literals through
//!   `Const`, identifiers through `LoadLocal`, and delegates
//!   compound shapes (binary / unary / call / method / field /
//!   tuple / struct / enum / match / if / cast / array / slice
//!   / range / block) to the matching `lower_*` helper. Stash
//!   slots (`pending_struct_value` / `pending_tuple_value` /
//!   `pending_enum_storage`) carry compound results that don't
//!   fit a single `ValueId`.
//! - `lower_builtin_call`: dispatcher for `BuiltinFunction`
//!   invocations. Routes `print` / `println` / `panic` /
//!   `assert` / `__builtin_*` to the matching helper.

use frontend::ast::{BuiltinFunction, Expr, ExprRef, StmtRef, UnaryOp};
use string_interner::DefaultSymbol;

use crate::method_call::ReceiverReload;
use super::bindings::{
    flatten_struct_locals, flatten_tuple_element_locals, Binding, EnumStorage, PayloadSlot,
    TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{BinOp, Const, EnumId, InstKind, Terminator, Type, ValueId};

/// Width of an enum's discriminant in a byte layout.
///
/// `Type::U64`, matching the tag `flatten_compound_leaf_types` emits at
/// function boundaries and the `Type::U64` local `EnumStorage` keeps it
/// in. Named because `compute_byte_size` and `collect_leaves` have to
/// agree on it or every payload offset after the tag is wrong.
const TAG_BYTE_SIZE: u64 = 8;

/// Reject a builtin lowering whose arity is wrong.
///
/// `what` is the whole expectation phrase the arm used to spell inline --
/// `"__builtin_heap_alloc takes 1 arg (size)"` -- so every message stays
/// exactly as it was while the four-line `if` at the top of twenty-one
/// arms becomes one line.
fn expect_args(args: &[ExprRef], n: usize, what: &str) -> Result<(), String> {
    if args.len() == n {
        return Ok(());
    }
    Err(format!("{what}, got {}", args.len()))
}

impl<'a> FunctionLower<'a> {
    /// Mirror of `interpreter/src/evaluation/builtin.rs::object_byte_size`
    /// for the AOT side. `__builtin_sizeof(value)` lowers to a
    /// constant via this helper. The recursion sums field /
    /// element sizes for structs / tuples / arrays, and gives an enum
    /// a `u64` tag plus every variant's payload (see the arm below).
    /// Alignment / padding are not modelled — the byte total is the
    /// natural sum, which lines up with how the user-space `Vec<T>`
    /// body uses the result (`cap * __builtin_sizeof::<T>()` for raw
    /// heap-alloc bookkeeping).
    pub(super) fn compute_byte_size(&self, ty: Type) -> Option<u64> {
        if let Some(size) = ty.scalar_byte_size() {
            return Some(size);
        }
        match ty {
            Type::Bool | Type::I8 | Type::U8 | Type::I16 | Type::U16 | Type::I32 | Type::U32
            | Type::F32 | Type::I64 | Type::U64 | Type::F64 | Type::Str => {
                unreachable!("sized by scalar_byte_size")
            }
            // SIMD: 128 bits, whatever the lane type.
            Type::Vector(_) => Some(16),
            Type::Unit => Some(0),
            Type::Struct(struct_id) => {
                let def = self.module.struct_def(struct_id);
                let mut total: u64 = 0;
                for (_name, field_ty) in &def.fields {
                    total = total.saturating_add(self.compute_byte_size(*field_ty)?);
                }
                Some(total)
            }
            Type::Tuple(tuple_id) => {
                let elements = self.module.tuple_defs[tuple_id.0 as usize].clone();
                let mut total: u64 = 0;
                for elem_ty in &elements {
                    total = total.saturating_add(self.compute_byte_size(*elem_ty)?);
                }
                Some(total)
            }
            // PTR-READ-ENUM: `u64` tag followed by *every* variant's
            // payload laid end to end — the same shape
            // `compiler_ir::layout::flatten_compound_leaf_types`
            // already uses at function boundaries, and the same shape
            // `EnumStorage` uses in locals (one slot per variant, not
            // one shared slot).
            //
            // It was `1 + max(payload)` — a packed tagged union — which
            // no part of the implementation actually laid out that way.
            // Two models meant an enum written through
            // `__builtin_ptr_write` and read back through
            // `__builtin_ptr_read` disagreed about where its payload
            // was, so the read had to be refused outright. The cost of
            // agreeing is the inactive variants' slots; for the enums
            // that carry one payload variant (`Option<T>`) there is no
            // difference at all.
            Type::Enum(enum_id) => {
                let def = self.module.enum_def(enum_id);
                let mut total: u64 = TAG_BYTE_SIZE;
                for variant in &def.variants {
                    for ty in &variant.payload_types {
                        total = total.saturating_add(self.compute_byte_size(*ty)?);
                    }
                }
                Some(total)
            }
        }
    }

    /// Walk `ty` to a list of `(byte_offset, leaf_scalar_type)`
    /// pairs, mirroring the natural-sum byte layout that
    /// `compute_byte_size` and the user-space `Vec<T>` body assume.
    /// Used by AOT-COMPOUND-PTR-RW to expand a single
    /// `__builtin_ptr_write(p, off, struct_value)` /
    /// `__builtin_ptr_read(p, off) -> Struct` into per-leaf
    /// scalar reads / writes — the AOT lower's `PtrRead` /
    /// `PtrWrite` IR nodes only model a scalar slot.
    ///
    /// Tuples flatten in declaration order; structs flatten in
    /// field declaration order (matching `flatten_struct_locals`
    /// and the IR's `StructDef.fields` iteration). Enums are
    /// rejected — variant payload layouts can't safely be
    /// encoded as a single offset list (a single buffer slot
    /// holds different leaf shapes per variant, which the
    /// per-leaf read/write model would garble). Callers that
    /// need enum support fall back to the "compound type
    /// unsupported" diagnostic and the existing single-scalar
    /// code path.
    pub(super) fn compute_leaf_layout(&self, ty: Type) -> Option<Vec<(u64, Type)>> {
        let mut leaves: Vec<(u64, Type)> = Vec::new();
        let mut offset: u64 = 0;
        self.collect_leaves(ty, &mut offset, &mut leaves)?;
        Some(leaves)
    }

    /// The leaf locals a compound value lives in, in the order
    /// `collect_leaves` walks its type.
    ///
    /// Shared by the compound `__builtin_ptr_write` expansion and its
    /// DATA-ORIENTED Phase 2 sibling `__builtin_soa_write`: both write
    /// one leaf at a time and differ only in the offset each leaf goes
    /// to, so the "where do the leaves live" half belongs in one
    /// place. `who` names the caller in the diagnostics.
    pub(super) fn compound_leaf_locals(
        &self,
        value_ref: &ExprRef,
        who: &str,
    ) -> Result<Vec<(crate::ir::LocalId, Type)>, String> {
        let value_expr = self
            .program
            .expression
            .get(value_ref)
            .ok_or_else(|| format!("{who}: value expr missing"))?;
        match value_expr {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(super::bindings::Binding::Struct { fields, .. }) => {
                    Ok(super::bindings::flatten_struct_locals(&fields))
                }
                Some(super::bindings::Binding::Tuple { elements }) => {
                    Ok(super::bindings::flatten_tuple_element_locals(&elements))
                }
                // PTR-READ-ENUM: an enum binding's locals flatten to
                // tag-then-every-variant, the same order
                // `collect_leaves` walks the type in.
                Some(super::bindings::Binding::Enum(storage)) => {
                    Ok(super::bindings::flatten_enum_storage_locals(&storage))
                }
                other => Err(format!(
                    "{who}: compound value identifier needs struct/tuple/enum binding, got {}",
                    if other.is_some() { "a binding of another shape" } else { "no binding" }
                )),
            },
            other => Err(format!(
                "{who}: compound value must be a bare identifier, got {}",
                crate::spelling::describe_expr(self.interner, &other)
            )),
        }
    }

    /// STR-INTERP-COMPOUND struct-arm body. Builds the formatted
    /// string `"TypeName { name: <to_string(value)>, ... }"`
    /// inline, matching the interpreter's
    /// `Object::to_display_string` (fields in alphabetical
    /// order). Currently restricted to structs whose fields are
    /// all scalar — nested compound fields would need recursive
    /// expansion (or an enum/tuple-aware extension) and are
    /// rejected with a precise message rather than silently
    /// falling back. Format prefixes are emitted via
    /// `ConstStrBytes` (raw `.rodata` bytes, no interner
    /// roundtrip), and per-field values go through the existing
    /// `InstKind::ToString` scalar runtime helper. Concatenation
    /// uses `InstKind::StrConcat` (the same `toy_str_concat`
    /// runtime helper string interpolation already relies on).
    fn lower_struct_to_string(
        &mut self,
        struct_id: crate::ir::StructId,
        arg_expr: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let arg_inner = self
            .program
            .expression
            .get(arg_expr)
            .ok_or_else(|| "__builtin_to_string arg expr missing".to_string())?;
        let fields = match arg_inner {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Struct { fields, .. }) => fields,
                _ => return Err(
                    "__builtin_to_string: struct identifier needs a Struct binding".to_string(),
                ),
            },
            // A struct-typed field / element (`"{o.inner}"`) resolves
            // to the same leaf tree an identifier binding carries.
            Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _) => {
                match self.resolve_field_chain(arg_expr)? {
                    super::bindings::FieldChainResult::Struct { fields, .. } => fields,
                    _ => return Err(
                        "__builtin_to_string: field is not struct-typed".to_string(),
                    ),
                }
            }
            _ => return Err(
                "__builtin_to_string: struct arg must be a bare identifier or a field access (MVP)"
                    .to_string(),
            ),
        };
        let v = self.emit_struct_format(struct_id, &fields)?;
        Ok(Some(v))
    }

    /// STR-INTERP-COMPOUND-EXTEND nested-compound helper. Builds
    /// the formatted text for a struct binding tree. Recurses into
    /// `FieldShape::Struct` / `FieldShape::Tuple` so a struct
    /// whose field is itself a struct (or tuple) prints as
    /// `Outer { inner: Inner { x: 1 } }` rather than bailing.
    fn emit_struct_format(
        &mut self,
        struct_id: crate::ir::StructId,
        fields: &[super::bindings::FieldBinding],
    ) -> Result<ValueId, String> {
        let (type_name_sym, decl_field_count): (string_interner::DefaultSymbol, usize) = {
            let def = self.module.struct_def(struct_id);
            (def.base_name, def.fields.len())
        };
        let type_name_str = self
            .interner
            .resolve(type_name_sym)
            .unwrap_or("?")
            .to_string();
        if fields.len() != decl_field_count {
            return Err(format!(
                "__builtin_to_string: struct {} field-binding count mismatch \
                 ({} bindings vs {} declared)",
                type_name_str,
                fields.len(),
                decl_field_count
            ));
        }
        let mut sorted: Vec<(usize, String)> = fields
            .iter()
            .enumerate()
            .map(|(i, fb)| (i, fb.name.clone()))
            .collect();
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        let header_text = if sorted.is_empty() {
            format!("{} {{}}", type_name_str)
        } else {
            format!("{} {{ ", type_name_str)
        };
        let mut acc = self
            .emit(
                InstKind::ConstStrBytes { bytes: header_text.into_bytes() },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        for (i, (decl_idx, name)) in sorted.iter().enumerate() {
            let prefix = format!("{}: ", name);
            let prefix_v = self
                .emit(
                    InstKind::ConstStrBytes { bytes: prefix.into_bytes() },
                    Some(Type::Str),
                )
                .expect("ConstStrBytes returns a value");
            acc = self
                .emit(InstKind::StrConcat { a: acc, b: prefix_v }, Some(Type::Str))
                .expect("StrConcat returns a value");
            let val_str = self.emit_field_to_string(&fields[*decl_idx])?;
            acc = self
                .emit(InstKind::StrConcat { a: acc, b: val_str }, Some(Type::Str))
                .expect("StrConcat returns a value");
            if i + 1 < sorted.len() {
                let sep = self
                    .emit(
                        InstKind::ConstStrBytes { bytes: b", ".to_vec() },
                        Some(Type::Str),
                    )
                    .expect("ConstStrBytes returns a value");
                acc = self
                    .emit(InstKind::StrConcat { a: acc, b: sep }, Some(Type::Str))
                    .expect("StrConcat returns a value");
            }
        }
        if !sorted.is_empty() {
            let footer = self
                .emit(
                    InstKind::ConstStrBytes { bytes: b" }".to_vec() },
                    Some(Type::Str),
                )
                .expect("ConstStrBytes returns a value");
            acc = self
                .emit(InstKind::StrConcat { a: acc, b: footer }, Some(Type::Str))
                .expect("StrConcat returns a value");
        }
        Ok(acc)
    }

    /// Dispatch a single struct field's binding tree to the right
    /// to_string emitter. Scalar leaves go through the existing
    /// `InstKind::ToString` runtime helper; nested struct / tuple
    /// fields recurse into `emit_struct_format` /
    /// `emit_tuple_format`. Enum fields aren't supported yet.
    fn emit_field_to_string(
        &mut self,
        field: &super::bindings::FieldBinding,
    ) -> Result<ValueId, String> {
        use super::bindings::FieldShape;
        match &field.shape {
            FieldShape::Scalar { local, ty } => {
                let val = self
                    .emit(InstKind::LoadLocal(*local), Some(*ty))
                    .expect("LoadLocal returns a value");
                Ok(self
                    .emit(
                        InstKind::ToString { value: val, value_ty: *ty },
                        Some(Type::Str),
                    )
                    .expect("ToString returns a value"))
            }
            FieldShape::Struct { struct_id, fields } => {
                let nested_fields = fields.clone();
                self.emit_struct_format(*struct_id, &nested_fields)
            }
            FieldShape::Tuple { tuple_id: _, elements } => {
                let nested = elements.clone();
                self.emit_tuple_format(&nested)
            }
            // JIT-enum-1: `"{p}"` where a field of `p` is an enum —
            // the same per-variant formatter the standalone
            // `"{color}"` interpolation uses.
            FieldShape::Enum(storage) => {
                let storage = (**storage).clone();
                self.emit_enum_to_string(&storage)
            }
        }
    }

    /// Tuple-element variant of `emit_field_to_string`. Same
    /// dispatch shape, just with `TupleElementShape` instead of
    /// `FieldShape`.
    fn emit_tuple_element_to_string(
        &mut self,
        element: &super::bindings::TupleElementBinding,
    ) -> Result<ValueId, String> {
        use super::bindings::TupleElementShape;
        match &element.shape {
            TupleElementShape::Scalar { local, ty } => {
                let val = self
                    .emit(InstKind::LoadLocal(*local), Some(*ty))
                    .expect("LoadLocal returns a value");
                Ok(self
                    .emit(
                        InstKind::ToString { value: val, value_ty: *ty },
                        Some(Type::Str),
                    )
                    .expect("ToString returns a value"))
            }
            TupleElementShape::Struct { struct_id, fields } => {
                let nested_fields = fields.clone();
                self.emit_struct_format(*struct_id, &nested_fields)
            }
            TupleElementShape::Tuple { tuple_id: _, elements } => {
                let nested = elements.clone();
                self.emit_tuple_format(&nested)
            }
        }
    }

    /// STR-INTERP-COMPOUND-EXTEND tuple-arm. Mirrors
    /// `lower_struct_to_string`'s shape but uses tuple-display
    /// formatting: `(a, b)` for >1 elements, `(a,)` for the
    /// single-element case (matches Rust + the interpreter's
    /// `Object::to_display_string`). Element order is the tuple's
    /// declaration order (no alphabetical sort — there's no field
    /// name to sort by). All-scalar elements only for now;
    /// nested compound elements are rejected with a precise
    /// message.
    fn lower_tuple_to_string(
        &mut self,
        arg_expr: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let arg_inner = self
            .program
            .expression
            .get(arg_expr)
            .ok_or_else(|| "__builtin_to_string arg expr missing".to_string())?;
        let elements = match arg_inner {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Tuple { elements }) => elements,
                _ => return Err(
                    "__builtin_to_string: tuple identifier needs a Tuple binding".to_string(),
                ),
            },
            // Tuple-typed field / element (`"{o.pair}"`), as above.
            Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _) => {
                match self.resolve_field_chain(arg_expr)? {
                    super::bindings::FieldChainResult::Tuple { elements } => elements,
                    _ => return Err(
                        "__builtin_to_string: field is not tuple-typed".to_string(),
                    ),
                }
            }
            _ => return Err(
                "__builtin_to_string: tuple arg must be a bare identifier or a field access (MVP)"
                    .to_string(),
            ),
        };
        let v = self.emit_tuple_format(&elements)?;
        Ok(Some(v))
    }

    /// STR-INTERP-COMPOUND-EXTEND nested-tuple helper. Builds the
    /// formatted text for a tuple element-binding tree. Recurses
    /// into nested struct / tuple elements just like
    /// `emit_struct_format`. Single-element tuples render as
    /// `(elem,)` (Rust convention), multi-element as
    /// `(elem0, elem1, ...)`.
    fn emit_tuple_format(
        &mut self,
        elements: &[super::bindings::TupleElementBinding],
    ) -> Result<ValueId, String> {
        let mut acc = self
            .emit(
                InstKind::ConstStrBytes { bytes: b"(".to_vec() },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        for (i, element) in elements.iter().enumerate() {
            let val_str = self.emit_tuple_element_to_string(element)?;
            acc = self
                .emit(InstKind::StrConcat { a: acc, b: val_str }, Some(Type::Str))
                .expect("StrConcat returns a value");
            if i + 1 < elements.len() {
                let sep = self
                    .emit(
                        InstKind::ConstStrBytes { bytes: b", ".to_vec() },
                        Some(Type::Str),
                    )
                    .expect("ConstStrBytes returns a value");
                acc = self
                    .emit(InstKind::StrConcat { a: acc, b: sep }, Some(Type::Str))
                    .expect("StrConcat returns a value");
            }
        }
        let footer_bytes: Vec<u8> = if elements.len() == 1 {
            b",)".to_vec()
        } else {
            b")".to_vec()
        };
        let footer = self
            .emit(
                InstKind::ConstStrBytes { bytes: footer_bytes },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        acc = self
            .emit(InstKind::StrConcat { a: acc, b: footer }, Some(Type::Str))
            .expect("StrConcat returns a value");
        Ok(acc)
    }

    /// STR-INTERP-COMPOUND-EXTEND-ENUM enum-arm. Builds the formatted
    /// string `EnumName::VariantName(p0, p1, ...)` inline, matching
    /// the interpreter's `Object::to_display_string` (generic enums
    /// include the concrete type-arg list: `Option<i64>::Some(5)`).
    /// The variant is chosen at runtime by dispatching on the tag
    /// local — the same brif chain `emit_print_enum` uses — and each
    /// per-variant block concatenates its text into a result local
    /// that the merge block loads. Payloads route through the same
    /// emitters as struct / tuple fields: scalars via the
    /// `InstKind::ToString` runtime helper, nested struct / tuple
    /// payloads via `emit_struct_format` / `emit_tuple_format`, enum
    /// payloads by recursion.
    fn lower_enum_to_string(
        &mut self,
        enum_id: EnumId,
        arg_expr: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let arg_inner = self
            .program
            .expression
            .get(arg_expr)
            .ok_or_else(|| "__builtin_to_string arg expr missing".to_string())?;
        let storage = match arg_inner {
            Expr::Identifier(sym) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Enum(storage)) => storage,
                _ => return Err(
                    "__builtin_to_string: enum identifier needs an Enum binding".to_string(),
                ),
            },
            // Enum-typed fields / elements are out of reach: the
            // field chain has no Enum variant yet, so there is no way
            // to name the tag / payload locals from the chain.
            _ => return Err(
                "__builtin_to_string: enum arg must be a bare identifier (MVP) — \
                 enum-typed fields are not supported at AOT"
                    .to_string(),
            ),
        };
        if storage.enum_id != enum_id {
            return Err(format!(
                "__builtin_to_string: enum id mismatch (inference said `{}` but the binding \
                 holds `{}`)",
                crate::spelling::spell_type(self.module, self.interner, Type::Enum(enum_id)),
                crate::spelling::spell_type(self.module, self.interner, Type::Enum(storage.enum_id))
            ));
        }
        let v = self.emit_enum_to_string(&storage)?;
        Ok(Some(v))
    }

    /// STR-INTERP-COMPOUND-EXTEND-ENUM: emit the tag-dispatch chain
    /// that produces the formatted text for an enum storage. Each
    /// per-variant block builds `EnumName::VariantName(...)` via
    /// `ConstStrBytes` + `StrConcat` and stores it into the result
    /// local; the merge block loads it as the final value.
    fn emit_enum_to_string(&mut self, storage: &EnumStorage) -> Result<ValueId, String> {
        let enum_def = self.module.enum_def(storage.enum_id).clone();
        // `Name` for non-generic enums, `Name<T1, ...>` for generic
        // instantiations — must match the interpreter's header
        // byte-for-byte for the consistency suite.
        let enum_str = self.format_enum_header(storage.enum_id);
        let n_variants = enum_def.variants.len();
        if n_variants == 0 {
            // An enum with no variants can never be constructed; emit
            // an empty string rather than crashing on an empty chain.
            return Ok(self
                .emit(
                    InstKind::ConstStrBytes { bytes: Vec::new() },
                    Some(Type::Str),
                )
                .expect("ConstStrBytes returns a value"));
        }
        let result_local = self.module.function_mut(self.func_id).add_local(Type::Str);
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
                    .emit(InstKind::Const(Const::U64(idx as u64)), Some(Type::U64))
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
                let text = self.emit_enum_variant_to_string(&enum_str, &variant_str, &slots)?;
                self.emit(InstKind::StoreLocal { dst: result_local, src: text }, None);
                self.terminate(Terminator::Jump(merge));
                self.switch_to(next);
            } else {
                // Last variant: unconditional fallback — the
                // type-checker guarantees the tag only holds a known
                // variant index, so no panic block is needed (same as
                // `emit_print_enum`).
                self.terminate(Terminator::Jump(body_blk));
                self.switch_to(body_blk);
                let text = self.emit_enum_variant_to_string(&enum_str, &variant_str, &slots)?;
                self.emit(InstKind::StoreLocal { dst: result_local, src: text }, None);
                self.terminate(Terminator::Jump(merge));
            }
        }
        self.switch_to(merge);
        Ok(self
            .emit(InstKind::LoadLocal(result_local), Some(Type::Str))
            .expect("LoadLocal returns a value"))
    }

    /// The text of one enum variant: `EnumName::VariantName` plus,
    /// for tuple variants, a parenthesised comma-separated list of
    /// payload values. Concatenation mirrors `emit_struct_format`'s
    /// `ConstStrBytes` + `StrConcat` chain; nested enum payloads
    /// recurse so `Some(Some(5))` renders through the same dispatch.
    fn emit_enum_variant_to_string(
        &mut self,
        enum_str: &str,
        variant_str: &str,
        slots: &[PayloadSlot],
    ) -> Result<ValueId, String> {
        let concat = |f: &mut Self, acc: ValueId, b: ValueId| {
            f.emit(InstKind::StrConcat { a: acc, b }, Some(Type::Str))
                .expect("StrConcat returns a value")
        };
        let mut acc = self
            .emit(
                InstKind::ConstStrBytes {
                    bytes: format!("{enum_str}::{variant_str}").into_bytes(),
                },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        if slots.is_empty() {
            return Ok(acc);
        }
        let open = self
            .emit(
                InstKind::ConstStrBytes { bytes: b"(".to_vec() },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        acc = concat(self, acc, open);
        for (i, slot) in slots.iter().enumerate() {
            if i > 0 {
                let sep = self
                    .emit(
                        InstKind::ConstStrBytes { bytes: b", ".to_vec() },
                        Some(Type::Str),
                    )
                    .expect("ConstStrBytes returns a value");
                acc = concat(self, acc, sep);
            }
            let val = match slot {
                PayloadSlot::Scalar { local, ty } => {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    self.emit(
                        InstKind::ToString { value: v, value_ty: *ty },
                        Some(Type::Str),
                    )
                    .expect("ToString returns a value")
                }
                PayloadSlot::Enum(inner) => {
                    let inner = (**inner).clone();
                    self.emit_enum_to_string(&inner)?
                }
                PayloadSlot::Struct { struct_id, fields } => {
                    let fields = fields.clone();
                    self.emit_struct_format(*struct_id, &fields)?
                }
                PayloadSlot::Tuple { elements, .. } => {
                    let elements = elements.clone();
                    self.emit_tuple_format(&elements)?
                }
                // UNIT-TYPE-ARG: the payload *is* `()`, and that is
                // what it renders as — `Ok(())`, matching what the
                // tree-walker prints and what a unit value spells
                // anywhere else. Rendering it as nothing would make
                // `Ok(())` and a payload-less `Ok` indistinguishable.
                PayloadSlot::Unit => self
                    .emit(
                        InstKind::ConstStrBytes { bytes: b"()".to_vec() },
                        Some(Type::Str),
                    )
                    .expect("ConstStrBytes returns a value"),
            };
            acc = concat(self, acc, val);
        }
        let close = self
            .emit(
                InstKind::ConstStrBytes { bytes: b")".to_vec() },
                Some(Type::Str),
            )
            .expect("ConstStrBytes returns a value");
        Ok(concat(self, acc, close))
    }

    fn collect_leaves(
        &self,
        ty: Type,
        offset: &mut u64,
        out: &mut Vec<(u64, Type)>,
    ) -> Option<()> {
        match ty {
            Type::Bool | Type::I8 | Type::U8
            | Type::I16 | Type::U16
            | Type::I32 | Type::U32
            | Type::I64 | Type::U64 | Type::F64 | Type::F32 | Type::Str
            // SIMD: one leaf, 16 bytes wide.
            | Type::Vector(_) => {
                out.push((*offset, ty));
                *offset = offset.saturating_add(self.compute_byte_size(ty)?);
                Some(())
            }
            Type::Unit => Some(()),
            Type::Struct(struct_id) => {
                let def = self.module.struct_def(struct_id);
                let field_tys: Vec<Type> = def.fields.iter().map(|(_, t)| *t).collect();
                for ft in field_tys {
                    self.collect_leaves(ft, offset, out)?;
                }
                Some(())
            }
            Type::Tuple(tuple_id) => {
                let elems = self.module.tuple_defs[tuple_id.0 as usize].clone();
                for et in elems {
                    self.collect_leaves(et, offset, out)?;
                }
                Some(())
            }
            // PTR-READ-ENUM: tag, then every variant's payload in
            // declaration order. Identical to `compute_byte_size`'s
            // enum arm and to `flatten_compound_leaf_types`, which is
            // the point — the buffer an enum is written to and the
            // locals it lives in now have one layout between them.
            //
            // Writing a value stores the inactive variants' slots too
            // (they hold whatever their locals hold). That is harmless:
            // the tag decides which slots a reader looks at, and the
            // same "load every slot" rule already governs enums crossing
            // a function boundary.
            Type::Enum(enum_id) => {
                out.push((*offset, Type::U64));
                *offset = offset.saturating_add(TAG_BYTE_SIZE);
                let def = self.module.enum_def(enum_id);
                let payload_tys: Vec<Type> = def
                    .variants
                    .iter()
                    .flat_map(|v| v.payload_types.iter().copied())
                    .collect();
                for pt in payload_tys {
                    self.collect_leaves(pt, offset, out)?;
                }
                Some(())
            }
        }
    }

    /// Values for one argument expression, expanding a compound
    /// binding to its leaves.
    ///
    /// The regular call path does this inline; associated-function
    /// calls used to lower each argument with a bare `lower_expr`,
    /// which produces nothing for a struct / tuple / enum binding and
    /// failed with "associated-function arg produced no value". That
    /// ruled out `Box::new(v)` for exactly the compound `v` the type
    /// exists to hold.
    /// CALL-ARG-COMPOUND-LITERAL: a struct / tuple literal written
    /// straight into an argument (`f(Point { x: 1i64, y: 2i64 })`,
    /// `f((1i64, 2i64))`).
    ///
    /// A compound never flows through SSA as one value — it lives in
    /// one local per leaf, and a call takes those leaves flattened.
    /// A compound *binding* argument was already expanded that way;
    /// a literal was not, so it reached `lower_expr`, which builds a
    /// pending compound and returns no value — hence "call argument
    /// produced no value", and the standing advice to bind the
    /// literal to a `val` first. Here it materialises into fresh
    /// leaf locals and the call takes their values, which is what
    /// the `val` was doing by hand.
    ///
    /// `param_ty` is the callee's declared type for this slot when
    /// the target is known; it picks the monomorphisation, which a
    /// struct literal's own name cannot do for a generic struct. It
    /// is only believed when it agrees with the literal in front of
    /// us — same struct name, same element count — so a caller whose
    /// slot indexing is off by an implicit receiver or a closure's
    /// env pointer degrades to the by-name path rather than building
    /// the wrong shape.
    ///
    /// Returns `Ok(None)` when the argument is not a compound
    /// literal, so the caller falls through to its normal path.
    pub(super) fn lower_compound_literal_arg(
        &mut self,
        param_ty: Option<Type>,
        arg: &ExprRef,
    ) -> Result<Option<Vec<ValueId>>, String> {
        let expr = self
            .program
            .expression
            .get(arg)
            .ok_or_else(|| "call argument missing".to_string())?;
        match expr {
            Expr::StructLiteral(struct_name, _) => {
                let struct_id = match param_ty {
                    Some(Type::Struct(id))
                        if self.module.struct_def(id).base_name == struct_name =>
                    {
                        id
                    }
                    _ => self.resolve_struct_instance(struct_name, None)?,
                };
                let fields = self.allocate_struct_fields(struct_id);
                self.store_struct_value_into_fields(struct_id, &fields, arg)?;
                Ok(Some(self.load_leaves(flatten_struct_locals(&fields))))
            }
            Expr::TupleLiteral(elems) => {
                let declared = match param_ty {
                    Some(Type::Tuple(id))
                        if self
                            .module
                            .tuple_defs
                            .get(id.0 as usize)
                            .is_some_and(|d| d.len() == elems.len()) =>
                    {
                        Some(id)
                    }
                    _ => None,
                };
                let elements = match declared {
                    Some(id) => self.allocate_tuple_elements(id)?,
                    // No declared type to follow — infer each
                    // element's own scalar type, as a tail-position
                    // tuple literal does.
                    _ => {
                        let mut out: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
                        for (i, e) in elems.iter().enumerate() {
                            let ty = self.value_scalar(e).ok_or_else(|| {
                                format!("tuple argument element #{i} has no inferable type")
                            })?;
                            let shape = self.allocate_tuple_element_shape(ty)?;
                            out.push(TupleElementBinding { index: i, shape });
                        }
                        out
                    }
                };
                self.store_tuple_value_into_elements(&elements, arg)?;
                Ok(Some(
                    self.load_leaves(flatten_tuple_element_locals(&elements)),
                ))
            }
            // ENUM-VARIANT-ARG: an enum construction written straight
            // into an argument (`take(Color::Red)`, `area(Shape::Circle(3i64))`).
            // Same story as the struct / tuple literals above — the
            // construction only had a home on a `val`, so every
            // `Option`-taking API forced a binding at each call site.
            // The unit form parses as a `QualifiedIdentifier`, the
            // tuple form as an `AssociatedFunctionCall`; both
            // materialise into a fresh `EnumStorage` here and the
            // call takes its leaves, tag first.
            Expr::QualifiedIdentifier(ref path)
                if path.len() == 2 && self.enum_defs.contains_key(&path[0]) =>
            {
                // No parameter type to follow (an associated-function
                // argument, say)? The unit form carries nothing to
                // infer from, so fall back to the by-name path — which
                // is enough for a non-generic enum and reports the
                // missing annotation for a generic one.
                let enum_id = match self.enum_instance_for_arg(path[0], param_ty) {
                    Some(id) => id,
                    None => self.resolve_enum_instance(path[0], None)?,
                };
                self.lower_enum_variant_arg(enum_id, path[0], path[1], &[])
            }
            Expr::AssociatedFunctionCall(enum_name, variant_name, ref args)
                if self.enum_defs.contains_key(&enum_name)
                    && self
                        .enum_variant_index(&enum_name, &variant_name)
                        .is_some() =>
            {
                let enum_id = match self.enum_instance_for_arg(enum_name, param_ty) {
                    Some(id) => id,
                    // Fall back to inferring the instantiation from
                    // the payload values, the way a `val` RHS does.
                    None => self.resolve_enum_instance_with_args(
                        enum_name,
                        variant_name,
                        args,
                        None,
                    )?,
                };
                self.lower_enum_variant_arg(enum_id, enum_name, variant_name, args)
            }
            // COMPOUND-ARG-CALL: an associated function returning a
            // compound (`take(P::origin())`, `f(Vec::new())`). The
            // parameter slot picks the instance when it names one —
            // that is the only thing that can, for a generic struct
            // whose constructor takes no arguments.
            Expr::AssociatedFunctionCall(struct_name, fn_name, ref args)
                if self.struct_defs.contains_key(&struct_name) =>
            {
                let struct_id = match param_ty {
                    Some(Type::Struct(id))
                        if self.module.struct_def(id).base_name == struct_name =>
                    {
                        id
                    }
                    _ => self.resolve_struct_instance(struct_name, None)?,
                };
                let Some(target_id) =
                    self.resolve_struct_method_func_id(struct_name, fn_name, struct_id, args)?
                else {
                    return Ok(None);
                };
                let ret = self.module.function(target_id).return_type;
                if !matches!(ret, Type::Enum(_) | Type::Struct(_) | Type::Tuple(_)) {
                    return Ok(None);
                }
                self.lower_compound_call_arg(target_id, args)
            }
            // ENUM-ARG-NEST / COMPOUND-ARG-CALL: a compound-returning
            // call in argument position (`sum(node(leaf(), 1i64,
            // leaf()))`, `take(mk(3i64))`). Same reason the
            // constructions above needed a home — the call's leaves
            // have to land somewhere before the outer call is emitted,
            // and on a `val` RHS that somewhere was the new binding.
            // A scalar-returning call falls through to the caller's
            // normal path.
            Expr::Call(fn_name, args_ref) => {
                let Some(target_id) = self.lookup_fn_here(None, fn_name) else {
                    return Ok(None);
                };
                let ret = self.module.function(target_id).return_type;
                if !matches!(ret, Type::Enum(_) | Type::Struct(_) | Type::Tuple(_)) {
                    return Ok(None);
                }
                let items: Vec<ExprRef> = match self.program.expression.get(&args_ref) {
                    Some(Expr::ExprList(items)) => items,
                    _ => return Ok(None),
                };
                self.lower_compound_call_arg(target_id, &items)
            }
            // OP-OVERLOAD-CHAIN: an overloaded operator whose result
            // is a struct (`take(a + b)`, and the inner `a + b` of
            // `a + b + c`). The result has to live in leaf locals like
            // any other compound, and this is the one place that hands
            // an argument slot a set of its own.
            Expr::Binary(op, lhs, rhs) => self.lower_binary_overload_arg(op, lhs, rhs),
            Expr::Unary(op, operand) => self.lower_unary_overload_arg(op, operand),
            // COMPOUND-ARG-CALL: a compound-returning method
            // (`take(o.twin())`). `prepare_compound_method_call` has
            // already lowered the receiver and the arguments, so this
            // only has to name somewhere for the results to land.
            Expr::MethodCall(recv, method_sym, ref method_args) => {
                let Some(call) =
                    self.prepare_compound_method_call(&recv, method_sym, method_args)?
                else {
                    return Ok(None);
                };
                self.lower_compound_method_arg(call)
            }
            _ => Ok(None),
        }
    }

    /// COMPOUND-ARG-CALL, method form: land a prepared compound method
    /// call's results in fresh leaf locals and yield their values.
    fn lower_compound_method_arg(
        &mut self,
        call: crate::method_call::CompoundMethodCall,
    ) -> Result<Option<Vec<ValueId>>, String> {
        // CODE-SIZE-SELF-ABI: this is where the call is emitted, so
        // this is where a materialised receiver gets read back.
        let reload = call.reload;
        let out = self.lower_compound_method_arg_inner(
            call.target,
            call.ret,
            call.args,
            call.writeback_dests,
        );
        reload.apply(self);
        out
    }

    fn lower_compound_method_arg_inner(
        &mut self,
        target: crate::ir::FuncId,
        ret: Type,
        args: Vec<ValueId>,
        writeback_dests: Vec<crate::ir::LocalId>,
    ) -> Result<Option<Vec<ValueId>>, String> {
        let call = crate::method_call::CompoundMethodCall {
            target,
            ret,
            args,
            writeback_dests,
            reload: crate::method_call::ReceiverReload::none(),
        };
        match call.ret {
            Type::Enum(enum_id) => {
                let storage = self.allocate_enum_storage(enum_id);
                let mut dests = Self::flatten_enum_dests(&storage);
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallEnum {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
                Ok(Some(self.load_enum_locals(&storage)))
            }
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                let leaves = flatten_struct_locals(&fields);
                let mut dests: Vec<crate::ir::LocalId> =
                    leaves.iter().map(|(l, _)| *l).collect();
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallStruct {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
                Ok(Some(self.load_leaves(leaves)))
            }
            Type::Tuple(tuple_id) => {
                let elements = self.allocate_tuple_elements(tuple_id)?;
                let leaves = flatten_tuple_element_locals(&elements);
                let mut dests: Vec<crate::ir::LocalId> =
                    leaves.iter().map(|(l, _)| *l).collect();
                dests.extend(call.writeback_dests);
                self.emit(
                    InstKind::CallTuple {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
                Ok(Some(self.load_leaves(leaves)))
            }
            // `prepare_compound_method_call` returns `None` for a
            // scalar return, so this is unreachable in practice.
            _ => Ok(None),
        }
    }

    /// OP-OVERLOAD-CHAIN: an overloaded binary operator standing in
    /// an argument slot. The operator's result is a struct, so it
    /// needs leaf locals of its own; `emit_binary_overload` allocates
    /// them and this loads their values for the call.
    ///
    /// `Ok(None)` when the operator is not overloaded for this
    /// operand type, so the caller's ordinary scalar path runs.
    fn lower_binary_overload_arg(
        &mut self,
        op: frontend::ast::Operator,
        lhs: ExprRef,
        rhs: ExprRef,
    ) -> Result<Option<Vec<ValueId>>, String> {
        let Some((_, fields)) = self.emit_binary_overload(op, lhs, rhs)? else {
            return Ok(None);
        };
        Ok(Some(self.load_leaves(flatten_struct_locals(&fields))))
    }

    /// The unary twin of [`Self::lower_binary_overload_arg`].
    fn lower_unary_overload_arg(
        &mut self,
        op: UnaryOp,
        operand: ExprRef,
    ) -> Result<Option<Vec<ValueId>>, String> {
        let Some((_, fields)) = self.emit_unary_overload(op, operand)? else {
            return Ok(None);
        };
        Ok(Some(self.load_leaves(flatten_struct_locals(&fields))))
    }

    /// COMPOUND-ARG-CALL: materialise a compound-returning call into
    /// fresh leaf locals and hand the argument list their values.
    ///
    /// The three `Call*` instructions already write a call's results
    /// straight into caller-side locals — that is how a `val` RHS
    /// works. All an argument slot needed was locals of its own to
    /// name, which is what the `val` was providing by hand.
    ///
    /// No drop is registered for the fresh storage: the value exists
    /// to be passed by value, so ownership moves into the callee, the
    /// same as passing a `val`-bound compound.
    fn lower_compound_call_arg(
        &mut self,
        target_id: crate::ir::FuncId,
        args_items: &[ExprRef],
    ) -> Result<Option<Vec<ValueId>>, String> {
        // REF-Stage-2 (ii): a callee with compound `&mut T` parameters
        // returns their leaves behind its own result, so the caller's
        // dest list has to carry those bindings' locals too.
        let writeback_dests = if self
            .module
            .function(target_id)
            .self_writeback_types
            .is_empty()
        {
            Vec::new()
        } else {
            self.collect_compound_writeback_dests_for(args_items, Some(target_id), 0)?
        };
        match self.module.function(target_id).return_type {
            Type::Enum(enum_id) => {
                let storage = self.allocate_enum_storage(enum_id);
                self.emit_enum_call_into_storage(&storage, target_id, args_items)?;
                Ok(Some(self.load_enum_locals(&storage)))
            }
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                let leaves = flatten_struct_locals(&fields);
                let mut dests: Vec<crate::ir::LocalId> =
                    leaves.iter().map(|(l, _)| *l).collect();
                dests.extend(writeback_dests);
                let (args, ptr_arg_reloads) =
                    self.lower_call_arg_items(args_items, Some(target_id))?;
                self.emit(
                    InstKind::CallStruct {
                        target: target_id,
                        args,
                        dests,
                    },
                    None,
                );
                for r in ptr_arg_reloads {
                    r.apply(self);
                }
                Ok(Some(self.load_leaves(leaves)))
            }
            Type::Tuple(tuple_id) => {
                let elements = self.allocate_tuple_elements(tuple_id)?;
                let leaves = flatten_tuple_element_locals(&elements);
                let mut dests: Vec<crate::ir::LocalId> =
                    leaves.iter().map(|(l, _)| *l).collect();
                dests.extend(writeback_dests);
                let (args, ptr_arg_reloads) =
                    self.lower_call_arg_items(args_items, Some(target_id))?;
                self.emit(
                    InstKind::CallTuple {
                        target: target_id,
                        args,
                        dests,
                    },
                    None,
                );
                for r in ptr_arg_reloads {
                    r.apply(self);
                }
                Ok(Some(self.load_leaves(leaves)))
            }
            _ => Ok(None),
        }
    }

    /// The `EnumId` an argument slot names, when the callee's declared
    /// type for the slot is that very enum. This is the only source
    /// that can instantiate a *generic* enum at a call site —
    /// `take(Option::None)` has nothing else to say what `T` is.
    fn enum_instance_for_arg(
        &mut self,
        base_name: DefaultSymbol,
        param_ty: Option<Type>,
    ) -> Option<crate::ir::EnumId> {
        match param_ty {
            Some(Type::Enum(id)) if self.module.enum_def(id).base_name == base_name => Some(id),
            _ => None,
        }
    }

    /// Build one enum construction into fresh storage and load its
    /// leaves in call-argument order.
    fn lower_enum_variant_arg(
        &mut self,
        enum_id: crate::ir::EnumId,
        enum_name: DefaultSymbol,
        variant_name: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<Option<Vec<ValueId>>, String> {
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
        let storage = self.allocate_enum_storage(enum_id);
        self.write_variant_into_storage(&storage, variant_idx, args)?;
        Ok(Some(self.load_enum_locals(&storage)))
    }

    /// `LoadLocal` for each leaf, in the order a call expects them.
    fn load_leaves(&mut self, leaves: Vec<(crate::ir::LocalId, Type)>) -> Vec<ValueId> {
        leaves
            .into_iter()
            .map(|(local, ty)| {
                self.emit(InstKind::LoadLocal(local), Some(ty))
                    .expect("LoadLocal returns a value")
            })
            .collect()
    }

    pub(super) fn lower_arg_values(&mut self, a: &ExprRef) -> Result<Vec<ValueId>, String> {
        let (values, reload) = self.lower_arg_values_for(a, None, 0)?;
        reload.skip();
        Ok(values)
    }

    /// CODE-SIZE-SELF-ABI: [`Self::lower_arg_values`] with the callee's
    /// slot known, so a wide compound operand can be handed over as an
    /// address instead of leaf by leaf.
    ///
    /// This is the shape operator overloads have -- `a + b` becomes
    /// `add(&a, &b)`, but the operands are lowered one at a time and so
    /// used to have no callee to ask. Passing it in is what lets an
    /// overload on a wide struct cost the same as any other method.
    pub(super) fn lower_arg_values_for(
        &mut self,
        a: &ExprRef,
        target: Option<crate::ir::FuncId>,
        param_index: usize,
    ) -> Result<(Vec<ValueId>, ReceiverReload), String> {
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(a) {
            match self.bindings.get(&sym).cloned() {
                Some(Binding::Struct { fields, .. }) => {
                    let leaves = flatten_struct_locals(&fields);
                    if target
                        .map(|t| self.module.function(t).ptr_param(param_index).is_some())
                        .unwrap_or(false)
                    {
                        let (addr, reload) = self.receiver_address(&leaves)?;
                        return Ok((vec![addr], reload));
                    }
                    let mut out = Vec::new();
                    for (local, ty) in leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        out.push(v);
                    }
                    return Ok((out, ReceiverReload::none()));
                }
                Some(Binding::Tuple { elements }) => {
                    let mut out = Vec::new();
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        out.push(v);
                    }
                    return Ok((out, ReceiverReload::none()));
                }
                Some(Binding::Enum(storage)) => {
                    return Ok((self.load_enum_locals(&storage), ReceiverReload::none()));
                }
                _ => {}
            }
        }
        if let Some(values) = self.lower_compound_literal_arg(None, a)? {
            let values = match target {
                Some(t) => self.temporary_arg(t, param_index, values)?,
                None => values,
            };
            return Ok((values, ReceiverReload::none()));
        }
        let v = self
            .lower_expr(a)?
            .ok_or_else(|| "call argument produced no value".to_string())?;
        Ok((vec![v], ReceiverReload::none()))
    }

    /// REF-Stage-2 (iv): produce the pointer a scalar `&T` parameter
    /// expects, for one argument.
    ///
    /// `pointee` is the callee's `param_ref_pointee` entry for this
    /// slot: `None` means the slot is not a scalar reference, and the
    /// caller's existing paths handle it. Otherwise the argument has
    /// to arrive as an address, and there are three ways to get one:
    ///
    /// - a `RefScalar` binding already holds one — forward it
    /// - a `Scalar` binding has a home — take its address
    /// - anything else (`f(22i64)`, `f(a + b)`, `f(p.x)`) has no home,
    ///   so give it one: spill the value into a fresh local and take
    ///   that local's address. Without this last case the *value* was
    ///   passed where a pointer was expected and the callee's first
    ///   `LoadRef` dereferenced whatever address that number named —
    ///   a wrong answer on the IR VM and a segfault once compiled.
    pub(super) fn lower_scalar_ref_arg(
        &mut self,
        arg: &ExprRef,
        pointee: Option<Type>,
    ) -> Result<Option<ValueId>, String> {
        let Some(pointee) = pointee else {
            return Ok(None);
        };
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(arg) {
            if let Some(Binding::RefScalar { local, .. }) = self.bindings.get(&sym).cloned() {
                return Ok(Some(
                    self.emit(InstKind::LoadLocal(local), Some(Type::U64))
                        .expect("LoadLocal returns a value"),
                ));
            }
            if let Some(Binding::Scalar { local, .. }) = self.bindings.get(&sym).cloned() {
                self.module
                    .function_mut(self.func_id)
                    .address_taken_locals
                    .insert(local);
                return Ok(Some(
                    self.emit(InstKind::AddressOf { local }, Some(Type::U64))
                        .expect("AddressOf returns a value"),
                ));
            }
        }
        let v = self
            .lower_expr(arg)?
            .ok_or_else(|| "reference argument produced no value".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(pointee);
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        self.module
            .function_mut(self.func_id)
            .address_taken_locals
            .insert(local);
        Ok(Some(
            self.emit(InstKind::AddressOf { local }, Some(Type::U64))
                .expect("AddressOf returns a value"),
        ))
    }

    pub(super) fn lower_call_args(&mut self, args_ref: &ExprRef) -> Result<Vec<ValueId>, String> {
        // No target, so no parameter can be pointer-passed and no
        // reload can arise.
        let (values, reloads) = self.lower_call_args_with_target(args_ref, None)?;
        debug_assert!(reloads.is_empty());
        Ok(values)
    }

    /// Variant that knows the callee's `param_ref_pointee` types so
    /// it can hand a `&T` parameter an address rather than a value:
    /// forwarding the pointer a `RefScalar` identifier already holds
    /// (dereferencing instead would corrupt
    /// `outer(x: &u64) -> u64 { inner(x) }`-style chains), taking a
    /// `Scalar` binding's address, or spilling a value that has no
    /// address of its own.
    pub(super) fn lower_call_args_with_target(
        &mut self,
        args_ref: &ExprRef,
        target: Option<crate::ir::FuncId>,
    ) -> Result<(Vec<ValueId>, Vec<ReceiverReload>), String> {
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let items: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        self.lower_call_arg_items(&items, target)
    }

    /// The address an explicit `&x` / `&mut x` argument passes, when
    /// the borrow is one of the scalar shapes that travel as a
    /// pointer (REF-Stage-2 (b)+(c)+(g)).
    ///
    /// `Ok(None)` means this is not one of them — a compound borrow,
    /// or a shape with no leaf to take the address of — and the
    /// caller falls through to the leaf-flatten erasure that peels
    /// the borrow and expands the identifier.
    ///
    /// The four shapes: a scalar local (`AddressOf`, and the local is
    /// marked address-taken so codegen gives it a stack slot), a
    /// `&mut` parameter already held as a pointer (pass the pointer
    /// on), a field or tuple chain ending in a scalar leaf, and one
    /// element of a scalar array.
    ///
    /// **One copy.** This was written twice — once for a function
    /// call's arguments and once for a method's — and the two were
    /// identical to the line. A borrow shape fixed in one of them
    /// would have left the other quietly lowering the old way.
    pub(super) fn borrow_arg_address(
        &mut self,
        arg: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        if let Some(Expr::Unary(op, inner)) = self.program.expression.get(arg)
            && matches!(op, UnaryOp::Borrow | UnaryOp::BorrowMut) {
                if let Some(Expr::Identifier(sym)) = self.program.expression.get(&inner) {
                    if let Some(Binding::Scalar { local, ty }) =
                        self.bindings.get(&sym).cloned()
                        && super::templates::is_scalar_pointee(ty) {
                            self.module
                                .function_mut(self.func_id)
                                .address_taken_locals
                                .insert(local);
                            let v = self
                                .emit(InstKind::AddressOf { local }, Some(Type::U64))
                                .expect("AddressOf returns a value");
                            return Ok(Some(v));
                        }
                    // RefScalar binding: just forward the
                    // pointer (the binding's local already
                    // holds the U64 ptr).
                    if let Some(Binding::RefScalar { local, .. }) =
                        self.bindings.get(&sym).cloned()
                    {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(Type::U64))
                            .expect("LoadLocal returns a value");
                        return Ok(Some(v));
                    }
                }
                // REF-Stage-2 (iii): `&mut <chain>` where the
                // operand is any field- / tuple-access chain
                // ending in a scalar leaf (e.g. `&mut s.field`,
                // `&mut t.0`, `&mut p.a.0`, `&mut o.inner.value`).
                // `resolve_field_chain` walks both kinds of
                // accesses and returns the leaf scalar's local;
                // we then mark it address-taken and emit
                // `AddressOf`. Compound leaves (struct / tuple
                // mid-chain) fall through to the regular
                // erasure path below.
                if matches!(
                    self.program.expression.get(&inner),
                    Some(Expr::FieldAccess(_, _)) | Some(Expr::TupleAccess(_, _))
                )
                    && let Ok(super::bindings::FieldChainResult::Scalar { local, ty }) =
                        self.resolve_field_chain(&inner)
                        && super::templates::is_scalar_pointee(ty) {
                            self.module
                                .function_mut(self.func_id)
                                .address_taken_locals
                                .insert(local);
                            let v = self
                                .emit(InstKind::AddressOf { local }, Some(Type::U64))
                                .expect("AddressOf returns a value");
                            return Ok(Some(v));
                        }
                // REF-Stage-2 (iii-index): `&mut <name>[i]` —
                // resolve the array binding's slot and emit
                // `ArrayElemAddr`, which is the canonical
                // pointer to the element in the per-array
                // stack slot. Index expression is lowered
                // first so any side effect inside it stays
                // visible.
                if let Some(Expr::SliceAccess(arr_expr, info)) =
                    self.program.expression.get(&inner)
                        && matches!(info.slice_type, frontend::ast::SliceType::SingleElement)
                            && let Some(Expr::Identifier(arr_sym)) =
                                self.program.expression.get(&arr_expr)
                                    && let Some(Binding::Array { element_ty, storage, .. }) =
                                        self.bindings.get(&arr_sym).cloned()
                                        && super::templates::is_scalar_pointee(element_ty)
                                            && let Some(idx_ref) = info.start {
                                                let idx_v = self
                                                    .lower_expr(&idx_ref)?
                                                    .ok_or_else(|| {
                                                        "array index produced no value".to_string()
                                                    })?;
                                                let v = self
                                                    .emit(
                                                        InstKind::ArrayElemAddr {
                                                            slot: storage.scalar_slot(),
                                                            index: idx_v,
                                                            elem_ty: element_ty,
                                                        },
                                                        Some(Type::U64),
                                                    )
                                                    .expect("ArrayElemAddr returns a value");
                                                return Ok(Some(v));
                                            }
            }
        Ok(None)
    }

    /// Per-item call-argument lowering, shared by the pool-backed
    /// call sites (`Expr::ExprList` args) and the let-lowering
    /// compound intercepts whose `Vec<ExprRef>` args never live in
    /// the expression pool (module-qualified calls, RUNTIME-IO).
    /// CONST-ARRAY: the address a `&[T; N]` parameter receives, when
    /// `arg` names an array: a `const` table (its bytes in the
    /// read-only section), a stack array of scalars (element 0 of its
    /// slot), or a borrowed array being passed on. An explicit `&` /
    /// `&mut` is peeled; the borrow is what the parameter says anyway.
    pub(super) fn array_ref_arg(&mut self, arg: &ExprRef) -> Result<Option<ValueId>, String> {
        let inner = match self.program.expression.get(arg) {
            Some(Expr::Unary(UnaryOp::Borrow | UnaryOp::BorrowMut, inner)) => inner,
            _ => *arg,
        };
        // An array literal argument (`f([1u8, 2u8])`) is written into a
        // slot of its own first, and that slot's address is passed.
        if let Some(Expr::ArrayLiteral(elems)) = self.program.expression.get(&inner) {
            let Some(first) = elems.first() else {
                return Ok(None);
            };
            let Some(element_ty) = self.value_scalar(first) else {
                return Ok(None);
            };
            if !element_ty.is_scalar() {
                return Ok(None);
            }
            let storage = self.allocate_array_storage(element_ty, elems.len(), false);
            for (i, e) in elems.iter().enumerate() {
                let v = self
                    .lower_expr(e)?
                    .ok_or_else(|| "array literal element produced no value".to_string())?;
                self.emit_array_leaf_store(&storage, 1, i, 0, v, element_ty);
            }
            let zero = self
                .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                .expect("Const returns a value");
            return Ok(self.emit(
                InstKind::ArrayElemAddr { slot: storage.scalar_slot(), index: zero, elem_ty: element_ty },
                Some(Type::U64),
            ));
        }
        let Some(Expr::Identifier(sym)) = self.program.expression.get(&inner) else {
            return Ok(None);
        };
        match self.bindings.get(&sym).cloned() {
            Some(Binding::ArrayRef { ptr, .. }) => {
                Ok(self.emit(InstKind::LoadLocal(ptr), Some(Type::U64)))
            }
            Some(Binding::Array { element_ty, storage, .. }) if element_ty.is_scalar() => {
                let zero = self
                    .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                    .expect("Const returns a value");
                Ok(self.emit(
                    InstKind::ArrayElemAddr { slot: storage.scalar_slot(), index: zero, elem_ty: element_ty },
                    Some(Type::U64),
                ))
            }
            Some(_) => Ok(None),
            None => match self.const_arrays.get(&sym).filter(|a| a.table.is_none()) {
                Some(array) => {
                    let bytes = array.bytes.clone();
                    Ok(self.emit(InstKind::ConstBytesAddr { bytes }, Some(Type::U64)))
                }
                None => Ok(None),
            },
        }
    }

    /// A range value's two bounds, when `expr` is one: a literal
    /// `a..b` or a name bound to a range. `Ok(None)` for anything
    /// else, which the caller lowers the ordinary way.
    pub(super) fn range_value_pair(
        &mut self,
        expr: &ExprRef,
    ) -> Result<Option<[ValueId; 2]>, String> {
        match self.program.expression.get(expr) {
            Some(Expr::Range(start, end)) => {
                let s = self
                    .lower_expr(&start)?
                    .ok_or_else(|| "range start produced no value".to_string())?;
                let e = self
                    .lower_expr(&end)?
                    .ok_or_else(|| "range end produced no value".to_string())?;
                Ok(Some([s, e]))
            }
            Some(Expr::Identifier(sym)) => match self.bindings.get(&sym).cloned() {
                Some(Binding::Range { start, end, ty }) => {
                    let s = self.emit(InstKind::LoadLocal(start), Some(ty)).expect("LoadLocal returns a value");
                    let e = self.emit(InstKind::LoadLocal(end), Some(ty)).expect("LoadLocal returns a value");
                    Ok(Some([s, e]))
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// RANGE-TYPE-ANNOTATION: in a function returning `Range<T>`, a
    /// range in tail position is written to the return pair and left
    /// as the pending tuple the implicit return reads.
    pub(super) fn stage_range_return(&mut self, expr: &ExprRef) -> Result<Option<ValueId>, String> {
        let [start, end] = self
            .range_value_pair(expr)?
            .ok_or_else(|| "a range return needs a range value".to_string())?;
        self.stage_range_pair(start, end)
    }

    fn stage_range_pair(&mut self, start: ValueId, end: ValueId) -> Result<Option<ValueId>, String> {
        let (start_local, end_local, ty) =
            self.range_return.expect("staged only while returning a range");
        self.emit(InstKind::StoreLocal { dst: start_local, src: start }, None);
        self.emit(InstKind::StoreLocal { dst: end_local, src: end }, None);
        self.pending_tuple_value = Some(vec![
            TupleElementBinding {
                index: 0,
                shape: super::bindings::TupleElementShape::Scalar { local: start_local, ty },
            },
            TupleElementBinding {
                index: 1,
                shape: super::bindings::TupleElementShape::Scalar { local: end_local, ty },
            },
        ]);
        Ok(None)
    }

    pub(super) fn lower_call_arg_items(
        &mut self,
        items: &[ExprRef],
        target: Option<crate::ir::FuncId>,
    ) -> Result<(Vec<ValueId>, Vec<ReceiverReload>), String> {
        // The callee's declared parameter types, when the target is
        // known. A compound literal argument follows them to pick the
        // right monomorphisation.
        let param_tys: Vec<Type> = target
            .map(|t| self.module.function(t).params.clone())
            .unwrap_or_default();
        let param_ref_pointee: Vec<Option<Type>> = target
            .map(|t| self.module.function(t).param_ref_pointee.clone())
            .unwrap_or_default();
        // A5-P2: per-param dyn-trait identity. `Some(trait_sym)` means
        // the slot expects a fat pointer; the call site coerces a
        // concrete struct arg into `(null_ptr, vtable_ptr)`.
        let param_dyn_trait: Vec<Option<(DefaultSymbol, bool)>> = target
            .map(|t| self.module.function(t).param_dyn_trait.clone())
            .unwrap_or_default();
        let mut values: Vec<ValueId> = Vec::with_capacity(items.len());
        // CODE-SIZE-SELF-ABI S3: one per pointer-passed argument that
        // had to be copied into a slot; the caller reads them back
        // after emitting the call.
        let mut ptr_arg_reloads: Vec<ReceiverReload> = Vec::new();
        for (arg_idx, a) in items.iter().enumerate() {
            // CONST-ARRAY: an array argument to a `&[T; N]` parameter
            // is its address.
            if param_tys.get(arg_idx) == Some(&Type::U64)
                && let Some(addr) = self.array_ref_arg(a)?
            {
                values.push(addr);
                continue;
            }
            // RANGE-TYPE-ANNOTATION: a range argument crosses as the
            // `(start, end)` pair the callee's signature expects.
            if matches!(param_tys.get(arg_idx), Some(Type::Tuple(_)))
                && let Some([start, end]) = self.range_value_pair(a)?
            {
                values.push(start);
                values.push(end);
                continue;
            }
            // A5-P2: dyn-trait coercion at the call site. When the
            // callee's param at this slot is `&dyn TraitName`, the
            // caller must hand over a fat pointer (data_ptr,
            // vtable_ptr). MVP-A handles empty-struct receivers
            // only — data_ptr = null, vtable_ptr = address of the
            // pre-emitted `toy_vtable_<trait>_<struct>` symbol.
            // The actual arg expression must reduce to a struct
            // identifier (`d`) or its explicit borrow (`&d`); we
            // recover the struct's base name from the binding and
            // look up the vtable through `module.vtables`.
            if let Some((trait_sym, is_mut_dyn)) =
                param_dyn_trait.get(arg_idx).and_then(|t| *t)
            {
                let _ = is_mut_dyn; // used below for the post-call writeback path
                // Unwrap an explicit `&<ident>` borrow if present —
                // both the auto-borrow form (`describe(d)`) and the
                // explicit form (`describe(&d)`) reach the same
                // coercion path.
                let inner_expr_ref = match self.program.expression.get(a) {
                    Some(Expr::Unary(UnaryOp::Borrow | UnaryOp::BorrowMut, inner)) => {
                        inner
                    }
                    _ => *a,
                };
                let (struct_sym, struct_fields, struct_id) = match self
                    .program
                    .expression
                    .get(&inner_expr_ref)
                {
                    Some(Expr::Identifier(sym)) => match self.bindings.get(&sym).cloned() {
                        Some(Binding::Struct { struct_id, fields }) => {
                            let base_name = self
                                .module
                                .struct_defs
                                .get(struct_id.0 as usize)
                                .map(|sd| sd.base_name);
                            (base_name, Some(fields), Some(struct_id))
                        }
                        _ => (None, None, None),
                    },
                    _ => (None, None, None),
                };
                let struct_sym = struct_sym.ok_or_else(|| {
                    "A5-P2: &dyn arg must be a struct-typed identifier (`describe(d)` / `describe(&d)`)"
                        .to_string()
                })?;
                if !self
                    .module
                    .vtables
                    .contains_key(&(trait_sym, struct_sym))
                {
                    return Err("A5-P2: no vtable for the &dyn arg's `impl <trait> for <struct>` pair".to_string());
                }
                // A5-P2-MVP-B: construct the data_ptr leaf. Two cases:
                //   * struct has zero leaves → sentinel `data_ptr = 0`.
                //     Cranelift rejects size-0 stack slots so we skip
                //     allocation; the thunk reads zero leaves and
                //     never dereferences the pointer.
                //   * struct has scalar leaves → allocate a per-call
                //     `dyn_coerce_slots` entry sized to the struct's
                //     natural-sum byte count, store each leaf at its
                //     offset, and use the slot's address as data_ptr.
                let struct_leaves = struct_fields
                    .as_ref()
                    .map(|f| super::bindings::flatten_struct_locals(f))
                    .unwrap_or_default();
                let data_ptr = if struct_leaves.is_empty() {
                    self.emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                        .expect("Const returns a value")
                } else {
                    let struct_id = struct_id.expect("struct_leaves nonempty implies binding");
                    let total_bytes = self
                        .compute_byte_size(Type::Struct(struct_id))
                        .ok_or_else(|| {
                            "A5-P2-MVP-B: cannot compute byte size for &dyn coercion source struct"
                                .to_string()
                        })?;
                    let slot_idx = {
                        let func = self.module.function_mut(self.func_id);
                        let idx = func.dyn_coerce_slots.len() as u32;
                        func.dyn_coerce_slots.push(total_bytes as u32);
                        idx
                    };
                    let slot_addr = self
                        .emit(
                            InstKind::DynCoerceSlotAddr { slot_idx },
                            Some(Type::U64),
                        )
                        .expect("DynCoerceSlotAddr returns a value");
                    // Write each leaf into the slot at its natural-sum
                    // byte offset. The order MUST match the thunk's
                    // PtrRead order — both sides derive from the
                    // module-walking flatten in declaration order,
                    // so a parallel iteration with running offset
                    // stays consistent. For `&mut dyn`, also record
                    // `(offset, leaf_ty)` + dest local so we can read
                    // the post-call leaves back after the outer call.
                    let mut running_offset: u64 = 0;
                    let mut writeback_layout: Vec<(u64, Type)> =
                        Vec::with_capacity(struct_leaves.len());
                    let mut writeback_dests: Vec<crate::ir::LocalId> =
                        Vec::with_capacity(struct_leaves.len());
                    for (local, leaf_ty) in &struct_leaves {
                        // NUM-W-ENUMERATION: the shared width table. This
                        // one had no `f32` arm, so a struct with an `f32`
                        // field could not become a `&dyn Trait`.
                        let Some(leaf_size) = leaf_ty.scalar_byte_size() else {
                            return Err(format!(
                                "A5-P2-MVP-B: unsupported leaf type {} in &dyn coercion",
                                crate::spelling::spell_type(self.module, self.interner, *leaf_ty)
                            ));
                        };
                        let leaf_val = self
                            .emit(InstKind::LoadLocal(*local), Some(*leaf_ty))
                            .expect("LoadLocal returns a value");
                        let off_v = self
                            .emit(
                                InstKind::Const(Const::U64(running_offset)),
                                Some(Type::U64),
                            )
                            .expect("Const returns a value");
                        self.emit(
                            InstKind::PtrWrite {
                                ptr: slot_addr,
                                offset: off_v,
                                value: leaf_val,
                                value_ty: *leaf_ty,
                            },
                            None,
                        );
                        writeback_layout.push((running_offset, *leaf_ty));
                        writeback_dests.push(*local);
                        running_offset = running_offset.saturating_add(leaf_size);
                    }
                    // A5-P2-MVP-C: register a pending writeback for
                    // `&mut dyn` args. The outer-call drain reads each
                    // leaf back from `slot_addr + offset` and
                    // `StoreLocal`s it into the original struct
                    // binding's leaf local. `&dyn` (immutable) args
                    // skip this step entirely.
                    if is_mut_dyn {
                        self.pending_dyn_mut_writebacks.push(super::DynMutWriteback {
                            slot_addr,
                            struct_leaves: writeback_layout,
                            dest_locals: writeback_dests,
                        });
                    }
                    slot_addr
                };
                let vtable_ptr = self
                    .emit(
                        InstKind::VtableAddr {
                            trait_sym,
                            struct_sym,
                        },
                        Some(Type::U64),
                    )
                    .expect("VtableAddr returns a value");
                values.push(data_ptr);
                values.push(vtable_ptr);
                continue;
            }
            // REF-Stage-2 (b)+(c)+(g): a scalar borrow travels as an
            // address. Compound borrows fall through to the
            // leaf-flatten erasure below.
            if let Some(v) = self.borrow_arg_address(a)? {
                values.push(v);
                continue;
            }
            // REF-Stage-2: fall back — peel an explicit borrow so the
            // same identifier-expansion path below runs (compound
            // borrows / non-identifier operands).
            let arg_expr_ref = match self.program.expression.get(a) {
                Some(Expr::Unary(UnaryOp::Borrow | UnaryOp::BorrowMut, inner)) => {
                    inner
                }
                _ => *a,
            };
            // REF-Stage-2 (iv): `T` -> `&T` auto-borrow at the boundary.
            // The frontend type checker already approved passing a
            // value where a `&T` is declared; the lowering has to
            // produce the address. Placed before the identifier
            // expansion below because a scalar-reference slot wants a
            // pointer, never leaves.
            if let Some(ptr) = self.lower_scalar_ref_arg(
                &arg_expr_ref,
                param_ref_pointee.get(arg_idx).copied().flatten(),
            )? {
                values.push(ptr);
                continue;
            }
            // Struct-typed identifier argument: expand into per-field
            // values in declaration order. Anything else flows through
            // `lower_expr`.
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(&arg_expr_ref) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
                    // CODE-SIZE-SELF-ABI S3: a wide `&T` / `&mut T`
                    // parameter takes one address, the same as a wide
                    // receiver does.
                    if target
                        .map(|t| self.module.function(t).ptr_param(arg_idx).is_some())
                        .unwrap_or(false)
                    {
                        let (addr, reload) = self.receiver_address(&leaves)?;
                        values.push(addr);
                        ptr_arg_reloads.push(reload);
                        continue;
                    }
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    // Tuple-typed identifier argument: expand into
                    // one value per leaf scalar, in declaration order
                    // (recursing through compound elements).
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    // Enum-typed identifier argument: same shape as
                    // the function-boundary flattening — tag first,
                    // then each variant's payloads in declaration
                    // order, recursing through nested enum slots.
                    let vs = self.load_enum_locals(&storage);
                    values.extend(vs);
                    continue;
                }
            }
            // JIT-enum-1 residue: an enum-typed field read directly
            // in argument position (`has_next(node.next)`). The read
            // leaves the value graph like every compound read — the
            // field's `EnumStorage` (tag local + per-variant payload
            // slots) already lives in the receiver's binding, so
            // expand its leaves here exactly as the enum *binding*
            // arm above does. Checked on the un-peeled argument so an
            // explicit `&mut <enum field>` borrow keeps falling
            // through to the compound-borrow rejection.
            if matches!(
                self.program.expression.get(a),
                Some(Expr::FieldAccess(..))
            ) && let Ok(super::bindings::FieldChainResult::Enum(storage)) =
                self.resolve_field_chain(a)
            {
                let vs = self.load_enum_locals(&storage);
                values.extend(vs);
                continue;
            }
            // CALL-ARG-COMPOUND-LITERAL: `f(Point { .. })` /
            // `f((1i64, 2i64))` — build the literal into leaf locals
            // and pass those, the same shape a compound binding gets.
            if let Some(leaves) =
                self.lower_compound_literal_arg(param_tys.get(arg_idx).copied(), &arg_expr_ref)?
            {
                let leaves = match target {
                    Some(t) => self.temporary_arg(t, arg_idx, leaves)?,
                    None => leaves,
                };
                values.extend(leaves);
                continue;
            }
            // Note: pass the borrow-peeled ref so explicit `&v` /
            // `&mut v` lowers via the inner expr's normal path.
            let v = self
                .lower_expr(&arg_expr_ref)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            values.push(v);
        }
        Ok((values, ptr_arg_reloads))
    }


    // -- expression lowering -------------------------------------------------------

    /// MODULE-SYSTEM P3: the qualifier a call was written with.
    ///
    /// `a::b::f(..)` records `[a, b]` beside the tree; anything else
    /// is the single segment the node carries. Both resolvers match
    /// a path by its end, so the extra segments narrow the
    /// candidates — and the **frontend uses the same slice**, which
    /// is what keeps the call this lowers and the call that was
    /// type-checked the same one.
    pub(super) fn written_qualifier(&self, nearest: DefaultSymbol) -> Vec<DefaultSymbol> {
        self.written_qualifier_at(self.current_expr.as_ref(), nearest)
    }

    /// As [`Self::written_qualifier`], for a node this frame names
    /// rather than the one being lowered — a `val`'s right-hand side
    /// is reached by intercept, not through `lower_expr`, so
    /// `current_expr` is still the statement above it.
    pub(super) fn written_qualifier_at(
        &self,
        at: Option<&ExprRef>,
        nearest: DefaultSymbol,
    ) -> Vec<DefaultSymbol> {
        if let Some(expr_ref) = at
            && let Some(path) = self.program.call_paths.get(expr_ref)
        {
            return path.clone();
        }
        vec![nearest]
    }

    /// Lower one expression, remembering which it is for DEBUG-OBS D3.
    ///
    /// The site of a diverging terminator is "wherever we are now", and
    /// this is the only layer that knows. Restoring the previous value
    /// on the way out is what makes it a stack rather than a
    /// last-writer-wins field: without it, `a - b`'s underflow guard
    /// would be attributed to `b`.
    pub(super) fn lower_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        if let Some(decls) = self.program.drop_flags.clear_before_expr.get(expr_ref) {
            let decls = decls.clone();
            self.clear_drop_flags(&decls);
        }
        let previous = self.current_expr.replace(*expr_ref);
        let result = self.lower_expr_here(expr_ref);
        self.current_expr = previous;
        result
    }

    /// Source position of the expression being lowered, recorded in the
    /// module's site table (DEBUG-OBS D3).
    ///
    /// The snippet is copied out of the program's `SourceMap` now,
    /// because a compiled binary cannot go looking for the file later
    /// — see `compiler_ir::Site`.
    pub(super) fn current_site(&mut self) -> Option<crate::ir::SiteId> {
        let expr_ref = self.current_expr?;
        self.site_of(&expr_ref)
    }

    /// As [`Self::current_site`], for a named expression.
    pub(super) fn site_of(&mut self, expr_ref: &ExprRef) -> Option<crate::ir::SiteId> {
        let loc = *self.program.location_pool.get_expr_location(expr_ref)?;
        let file = self.program.source_map.get(loc.file);
        let path = match file.map(|f| f.path.as_str()) {
            // The driver names the entry file after type-checking, so
            // an empty name means nobody did — say so rather than
            // inventing one.
            Some("") | None => "<input>",
            Some(path) => path,
        };
        let snippet = file
            .and_then(|f| f.source.lines().nth(loc.line.saturating_sub(1) as usize))
            .map(str::to_string);
        Some(self.module.intern_site(
            path,
            loc.line,
            loc.column,
            loc.offset,
            loc.end_offset.saturating_sub(loc.offset),
            snippet.as_deref(),
        ))
    }

    fn lower_expr_here(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing expr".to_string())?;
        if self.is_unreachable() {
            return Ok(None);
        }
        match expr {
            Expr::Block(stmts) => self.lower_expr_block(&stmts),
            Expr::Int64(v) => Ok(self.emit(InstKind::Const(Const::I64(v)), Some(Type::I64))),
            Expr::UInt64(v) => Ok(self.emit(InstKind::Const(Const::U64(v)), Some(Type::U64))),
            Expr::Float64(v) => Ok(self.emit(InstKind::Const(Const::F64(v)), Some(Type::F64))),
            // SIMD-F32: single-precision literal.
            Expr::Float32(v) => Ok(self.emit(InstKind::Const(Const::F32(v)), Some(Type::F32))),
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::String(sym) => self.lower_expr_string(sym),
            Expr::Identifier(sym) => self.lower_expr_identifier(sym),
            Expr::FieldAccess(obj, field) => {
                self.pending_struct_value = None;
                // DATA-ORIENTED: `ps[i].x` — a field chain rooted at
                // an array element lowers to one leaf load instead of
                // materialising the whole element.
                if let Some(v) = self.try_lower_const_table_leaf(expr_ref)? {
                    return Ok(v);
                }
                if let Some(v) = self.try_lower_array_element_leaf(expr_ref)? {
                    return Ok(v);
                }
                self.lower_field_access(&obj, field)
            }
            Expr::TupleAccess(tuple, index) => {
                self.pending_struct_value = None;
                // Same shortcut for `ts[i].0`-shaped chains.
                if let Some(v) = self.try_lower_const_table_leaf(expr_ref)? {
                    return Ok(v);
                }
                if let Some(v) = self.try_lower_array_element_leaf(expr_ref)? {
                    return Ok(v);
                }
                self.lower_tuple_access(&tuple, index)
            }
            Expr::Range(..) if self.range_return.is_some() => self.stage_range_return(expr_ref),
            Expr::TupleLiteral(elems) => {
                // Tail-position tuple literal — materialise each
                // element into a fresh local and stash the resulting
                // element list as the pending tuple value. The IR
                // never sees a tuple value flow through SSA — the
                // implicit-return path consumes the element locals
                // directly. Non-tail uses (e.g. arithmetic on the
                // result) hit the value-required check downstream.
                self.lower_tuple_literal_tail(elems)
            }
            Expr::StructLiteral(struct_name, fields) => {
                // Tail-position struct literal: materialise each field
                // into a fresh local and stash the resulting field
                // binding list as the pending struct value. The IR
                // never sees a struct value flow through SSA — the
                // implicit-return path consumes the field locals
                // directly.
                self.lower_struct_literal_tail(struct_name, fields)
            }
            Expr::Binary(op, lhs, rhs) => self.lower_binary(&op, &lhs, &rhs),
            Expr::Unary(op, operand) => self.lower_unary(&op, &operand),
            Expr::Assign(lhs, rhs) => self.lower_assign(&lhs, &rhs),
            Expr::IfElifElse(cond, then_blk, elif_pairs, else_blk) => {
                self.lower_if_chain(&cond, &then_blk, &elif_pairs, &else_blk)
            }
            Expr::Call(fn_name, args_ref) => self.lower_call(fn_name, &args_ref),
            Expr::AssociatedFunctionCall(struct_name, fn_name, args) => {
                self.lower_expr_associated_call(struct_name, fn_name, args)
            }
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args, expr_ref),
            Expr::Cast(inner, target_ty) => self.lower_cast(&inner, &target_ty),
            Expr::Match(scrutinee, arms) => self.lower_match(&scrutinee, &arms),
            Expr::MethodCall(obj, method, args) => self.lower_method_call(&obj, method, &args),
            Expr::BuiltinMethodCall(receiver, method, args) => {
                self.lower_expr_builtin_method_call(&receiver, method, &args)
            }
            Expr::SliceAccess(obj, info) => self.lower_slice_access(&obj, &info),
            Expr::SliceAssign(obj, start, end, value) => {
                self.lower_slice_assign(&obj, start.as_ref(), end.as_ref(), &value)
            }
            // NUM-W-AOT (T5 follow-up to Phase 5): narrow integer
            // literals lower to the matching `Const::*` IR
            // instruction. The cranelift codegen consumes that
            // and emits an `iconst` with the right cranelift
            // integer type (I8 / I16 / I32). All arithmetic /
            // comparison / cast paths from there reuse the
            // existing wide-int code paths since cranelift's
            // `iadd` etc. pick up width from the operand types.
            Expr::Int8(n) => Ok(self.emit(InstKind::Const(Const::I8(n)), Some(Type::I8))),
            Expr::UInt8(n) => Ok(self.emit(InstKind::Const(Const::U8(n)), Some(Type::U8))),
            Expr::Int16(n) => Ok(self.emit(InstKind::Const(Const::I16(n)), Some(Type::I16))),
            Expr::UInt16(n) => Ok(self.emit(InstKind::Const(Const::U16(n)), Some(Type::U16))),
            Expr::Int32(n) => Ok(self.emit(InstKind::Const(Const::I32(n)), Some(Type::I32))),
            // CHAR-LITERAL-NUM: a char literal the type checker
            // did not narrow is a plain `u32` literal.
            Expr::UInt32(n) | Expr::CharLiteral(n) => {
                Ok(self.emit(InstKind::Const(Const::U32(n)), Some(Type::U32)))
            }
            // #121 Phase B-rest Item 2: `with allocator = expr { body }`.
            // Push the allocator handle, increment the with-scope
            // depth so `terminate_return` / `break` / `continue`
            // know to emit cleanup pops, then lower the body. On
            // a normal (linear) exit emit the matching pop here;
            // on an early exit the cleanup helpers already
            // emitted it before terminating, so we just decrement
            // the depth without a duplicate pop.
            Expr::With(allocator_expr, body_expr) => self.lower_expr_with(&allocator_expr, &body_expr),
            // Closures Phase 5b: a `Expr::Closure` literal in
            // expression position (e.g. as a HOF argument:
            // `apply(fn(x: i64) -> i64 { x }, 5i64)`). We lift the
            // closure to an anonymous top-level function on the fly
            // and emit `FuncAddr` to yield its runtime address as
            // a `Type::U64` value. The caller of `lower_expr` is
            // responsible for matching the value to a fn-typed
            // parameter slot. Captures are still unsupported —
            // body lowering will fail with "undefined identifier"
            // if the closure body references an outer-scope local.
            Expr::Closure { params, return_type, body, .. } => {
                self.lift_closure_inline(&params, &return_type, &body)
            }
            // MODULE-CONST-PATH: `segfile::DATA_AT` -- a module's `const`
            // named with its qualifier. The type checker has verified the
            // qualifier (`check_module_paths`), and consts are flattened
            // by name, so the last segment is the const. A qualified name
            // never names a local, so the bindings are not consulted.
            // (An enum's unit variant, the other two-segment path, was
            // handled by its own arm above.)
            Expr::QualifiedIdentifier(ref path)
                if path.len() >= 2
                    && !self.enum_defs.contains_key(&path[0])
                    && path.last().is_some_and(|name| self.const_values.contains_key(name)) =>
            {
                let c = self.const_values[path.last().expect("checked by the guard")];
                self.pending_struct_value = None;
                let ty = c.ty();
                Ok(self.emit(InstKind::Const(c), Some(ty)))
            }
            other => Err(format!(
                "compiler MVP cannot lower {} yet",
                crate::spelling::describe_expr(self.interner, &other)
            )),
        }
    }

    /// Phase 5 (汎用 RAII): every block opens a fresh drop scope.
    /// `Drop`-impling bindings registered in the body get drained at
    /// exit (linear here; early `return` / `break` / `continue` paths
    /// emit drops via `terminate_return` / `Stmt::Break` /
    /// `Stmt::Continue` before terminating). Errors propagate without
    /// running drops — same panic-safety policy the interpreter uses.
    fn lower_expr_block(&mut self, stmts: &[StmtRef]) -> Result<Option<ValueId>, String> {
        // The function body is the one block lowered while `drop_scopes`
        // is still empty; nested blocks (`if` arms, `val x = { .. }`,
        // `with` bodies) are entered with at least one scope already
        // on the stack.
        let is_function_body = self.drop_scopes.is_empty();
        self.enter_drop_scope();
        // Restore any binding this block shadows. `bindings` is a flat
        // map, so a `var x` inside a block was permanently overwriting
        // an outer `x` — `interpreter/example/scope.t` returned 1011
        // from the AOT (inner value + 1) against 101 from the
        // interpreter and the JIT.
        //
        // Only *overwritten* entries are restored; bindings the block
        // introduces are left in place. Dropping those as well is what
        // scoping would really mean, but `let_lowering`'s scalar-type
        // inference reaches for them after the block has been lowered
        // (`val total: u64 = with allocator = a { ... x }` resolves `x`
        // that way), so removing them breaks programs that work today.
        // Narrowing to the shadowing case fixes the wrong answer
        // without disturbing that.
        let shadowed_bindings = self.bindings.clone();
        let mut last: Option<ValueId> = None;
        for s in stmts {
            last = self.lower_stmt(s)?;
            if self.is_unreachable() {
                break;
            }
        }
        // A struct binding used as the function's tail expression is
        // moved out — the caller takes ownership — so its auto-drop
        // must not fire when the function-body scope pops below.
        // Without this, `fn new() -> Region { var r = ...; r }` drops
        // `r` (freeing its pointer) and then returns the dangling value.
        // Nested blocks are excluded: their tail feeds a `val` binding
        // or a branch, which *copies*, so the source binding still owns
        // its value and must be dropped.
        if is_function_body
            && let Some(fields) = &self.pending_struct_value
        {
            let leaves = flatten_struct_locals(fields);
            if let Some(top) = self.drop_scopes.last_mut() {
                top.retain(|t| t.field_locals.as_slice() != leaves.as_slice());
            }
        }
        self.pop_and_emit_drops()?;
        for (sym, binding) in shadowed_bindings {
            self.bindings.insert(sym, binding);
        }
        Ok(last)
    }

    /// String literals in value position emit `ConstStr`, which
    /// materialises a pointer-sized handle to the shared `.rodata`
    /// blob (the same one `PrintStr` uses for `print("literal")`).
    fn lower_expr_string(&mut self, sym: DefaultSymbol) -> Result<Option<ValueId>, String> {
        let bytes_len = self
            .interner
            .resolve(sym)
            .map(|s| s.len() as u64)
            .unwrap_or(0);
        Ok(self.emit(
            InstKind::ConstStr { message: sym, bytes_len },
            Some(Type::Str),
        ))
    }

    /// Bare-identifier use: dispatch through the binding table to load
    /// the right local / pending-compound storage.
    fn lower_expr_identifier(&mut self, sym: DefaultSymbol) -> Result<Option<ValueId>, String> {
        match self.bindings.get(&sym).cloned() {
            // RANGE-FOR: a range carries no single value. Reading its
            // bounds, copying it into a name, iterating and printing it
            // each have their own path; reaching here means some other
            // use (an argument, a return, an operand).
            Some(Binding::Range { start, end, ty }) if self.range_return.is_some() => {
                let s = self.emit(InstKind::LoadLocal(start), Some(ty)).expect("LoadLocal returns a value");
                let e = self.emit(InstKind::LoadLocal(end), Some(ty)).expect("LoadLocal returns a value");
                self.stage_range_pair(s, e)
            }
            Some(Binding::ArrayRef { .. }) => Err(format!(
                "compiler MVP cannot use the borrowed array `{}` as a value here \
                 (index it, or pass it on to another `&[T; N]` parameter)",
                self.interner.resolve(sym).unwrap_or("?")
            )),
            Some(Binding::Range { .. }) => Err(format!(
                "compiler MVP cannot use range `{}` as a value here (read `.start` / `.end`, \
                 iterate it with `for`, or print it)",
                self.interner.resolve(sym).unwrap_or("?")
            )),
            Some(Binding::Scalar { local, ty }) => {
                self.pending_struct_value = None;
                Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
            }
            Some(Binding::RefScalar { local, pointee_ty, .. }) => {
                // REF-Stage-2 (g): a `&T` / `&mut T` parameter
                // binding is auto-dereferenced when read in
                // value position. Load the pointer from the
                // local, then dereference it via LoadRef to
                // the pointee scalar.
                self.pending_struct_value = None;
                let ptr = self
                    .emit(InstKind::LoadLocal(local), Some(Type::U64))
                    .ok_or_else(|| "RefScalar load: LoadLocal returned no value".to_string())?;
                Ok(self.emit(InstKind::LoadRef { ptr, ty: pointee_ty }, Some(pointee_ty)))
            }
            Some(Binding::Struct { fields, .. }) => {
                // Tail-position use: stash the struct's field
                // list so `emit_implicit_return` can return it.
                // Non-tail uses (e.g. `5 + p`) will fail at
                // arithmetic lowering when no scalar value
                // materialises.
                self.pending_struct_value = Some(fields);
                Ok(None)
            }
            Some(Binding::Tuple { elements }) => {
                // Tail-position use: stash the elements list
                // so `emit_implicit_return` can pull element
                // values out for a tuple-returning function.
                // Non-tail uses fall through to errors when a
                // scalar value is later required.
                self.pending_tuple_value = Some(elements);
                Ok(None)
            }
            Some(Binding::Enum(storage)) => {
                // Tail-position use: stash the enum storage
                // so `emit_implicit_return` can flatten it
                // into a multi-value Return for an enum-
                // returning function. Other uses (passing to
                // a function, explicit Return) handle the
                // binding via a direct lookup, so the
                // channel is purely for the tail-implicit-
                // return path.
                self.pending_enum_value = Some(storage);
                Ok(None)
            }
            Some(Binding::Array { .. }) => {
                // Bare-identifier use of an array binding is
                // not supported in expression position yet —
                // arrays don't flow through the IR's value
                // graph. The user must access an element.
                Err(format!(
                    "compiler MVP cannot use array `{}` as a value; access an element with `{}[i]`",
                    self.interner.resolve(sym).unwrap_or("?"),
                    self.interner.resolve(sym).unwrap_or("?"),
                ))
            }
            Some(Binding::FunctionPtr { local, .. }) => {
                // Closures Phase 5b: bare use of a fn-pointer
                // binding loads the U64 address. Used when
                // forwarding a HOF parameter to another HOF
                // call (`apply(g, x)` inside a body whose
                // `g` is itself a FunctionPtr param).
                self.pending_struct_value = None;
                Ok(self.emit(InstKind::LoadLocal(local), Some(Type::U64)))
            }
            Some(Binding::DynTraitObj { .. }) => {
                // A5-P2: bare use of a `&dyn Trait` value as an
                // expression isn't supported in MVP-A. The intended
                // use is method-call dispatch (handled in
                // `lower_method_call`); plumbing the fat pointer
                // around as a value would require a compound IR
                // shape that the value graph doesn't carry yet.
                Err(format!(
                    "compiler MVP cannot use `&dyn Trait` binding `{}` as a value (call a trait method on it instead)",
                    self.interner.resolve(sym).unwrap_or("?"),
                ))
            }
            None => {
                // Closures Phase 5b: a `val f = fn(...)` binding
                // registers the lifted FuncId in
                // `closure_bindings` (Phase 5a) without an
                // entry in `bindings`. When such a name is used
                // in expression position (passing the closure
                // to a HOF), emit FuncAddr to materialise the
                // function's runtime address as a U64.
                if let Some(link) = self.closure_bindings.get(&sym).copied() {
                    self.pending_struct_value = None;
                    // Phase 6b: env-based ABI — every
                    // closure binding has an env_ptr
                    // (Some) regardless of capture set.
                    // Surfacing the closure as a value
                    // returns the env_ptr; the receiving
                    // HOF uses CallIndirect against it
                    // (codegen loads fn_ptr from env+0).
                    if let Some(env_ptr) = link.env_ptr {
                        return Ok(Some(env_ptr));
                    }
                    // Defensive fallback (should never fire
                    // post-Phase-6b — every closure binding
                    // emits MakeClosure on entry): expose
                    // the bare fn pointer.
                    return Ok(self.emit(
                        InstKind::FuncAddr { target: link.func_id },
                        Some(Type::U64),
                    ));
                }
                // Fall back to top-level `const` lookup. This
                // mirrors what the type-checker does: a name
                // that wasn't introduced by a local binding
                // can still resolve to a global const value.
                if let Some(c) = self.const_values.get(&sym).copied() {
                    self.pending_struct_value = None;
                    let ty = c.ty();
                    return Ok(self.emit(InstKind::Const(c), Some(ty)));
                }
                Err(format!(
                    "undefined identifier `{}`",
                    self.interner.resolve(sym).unwrap_or("?")
                ))
            }
        }
    }

    /// Module-qualified call (`math::add(args)`): when the qualifier
    /// doesn't refer to a struct / enum and the function exists in the
    /// (post-import) main function table, treat it as a plain `Call`.
    /// Module integration flattens imported `pub fn`s in, so the bare
    /// lookup hits without needing the qualifier. Real associated
    /// calls (`Container::new(...)`) keep the unsupported reject path
    /// below.
    fn lower_expr_associated_call(
        &mut self,
        struct_name: DefaultSymbol,
        fn_name: DefaultSymbol,
        args: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        // STDLIB-TRAIT-BASE B5: `T::assoc()` inside a monomorphised
        // body names a type parameter, and every lookup below expects
        // a declared type -- or, for a primitive, the canonical name
        // its impls register under. Substituting the qualifier here
        // covers the expression position; `lower_let` does the same
        // for its own early-return intercepts, which read the node
        // before this is reached.
        let struct_name = self
            .concrete_type_param_name(struct_name)
            .unwrap_or(struct_name);
        let is_struct = self.struct_defs.contains_key(&struct_name)
            || self.enum_defs.contains_key(&struct_name);
        // Qualified call (`math::add(args)`): try
        // `(Some(struct_name), fn_name)` first so cross-
        // module collisions resolve unambiguously. Fall back
        // to the bare lookup so legacy code paths that
        // pre-date the per-module key still work — e.g.
        // an integration that didn't record a module path
        // (None entry) but flattened the function into the
        // table by name.
        let target_opt = if !is_struct {
            // A generic module function (`random::shuffle(&mut v)`) is
            // not in the function index under its bare name at all --
            // each instantiation is minted on demand from the
            // template, which is what `resolve_call_target` does for
            // the unqualified spelling. Without this the lookups below
            // find nothing and the call is rejected as unsupported,
            // so the same function worked written one way and not the
            // other.
            if self.generic_funcs.contains_key(&fn_name) {
                Some(self.resolve_call_target_from_args(fn_name, &args)?)
            } else {
                let written = self.written_qualifier(struct_name);
                self.module
                    .lookup_function(Some(&written), fn_name)
                    .or_else(|| self.lookup_fn_here(None, fn_name))
            }
        } else {
            None
        };
        // STDLIB-TRAIT-BASE B5: an associated function on a
        // *primitive* (`u64::default()`, reached from `T::default()`
        // in a monomorphised body). `impl Default for u64` registers
        // under the canonical name symbol, exactly as `impl Ord for
        // u64` does -- the method registry already holds it, and only
        // the lookup was missing, because a primitive is neither a
        // struct nor a module.
        //
        // Note this name cannot be *written*: every primitive's name
        // is a reserved keyword, so `u64::default()` is a parse error.
        // It exists only as the substituted form of `T::default()`.
        let target_opt = target_opt.or_else(|| {
            crate::method_registry::lookup_method_func(
                self.method_func_ids,
                struct_name,
                fn_name,
                &[],
            )
        });
        if let Some(target) = target_opt {
            let ret_ty = self.module.function(target).return_type;
            if matches!(ret_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                return Err(format!(
                    "compiler MVP cannot use a compound-returning module call (`{}::{}`) in expression position; bind the result with `val`",
                    self.interner.resolve(struct_name).unwrap_or("?"),
                    self.interner.resolve(fn_name).unwrap_or("?"),
                ));
            }
            // Lower the arguments through the same per-item path a
            // bare call uses, with the callee known. Flattening
            // compound arguments into leaves is only part of what it
            // does -- it also hands a `&T` parameter an address
            // rather than a value, which is what a `&mut Vec<T>`
            // parameter needs (`random::shuffle(&mut v)` reported
            // "call argument produced no value" without it).
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(&args, Some(target))?;
            let result_ty = if ret_ty.produces_value() {
                Some(ret_ty)
            } else {
                None
            };
            // REF-Stage-2 (ii): a callee that mutates a compound
            // through `&mut` returns the changed leaves, and the
            // caller's locals only see them if the call is emitted
            // with the writeback destinations attached.
            // The callee has to be named here too. A `&mut`
            // parameter wide enough to travel as an address declares
            // no writeback slots -- it writes through the pointer --
            // so counting its leaves makes the caller's dest list
            // disagree with the callee's shape. The arguments above
            // already pass `Some(target)`; these have to agree with
            // them or the call cannot be built at all.
            let writeback_dests =
                if !self.module.function(target).self_writeback_types.is_empty() {
                    self.collect_compound_writeback_dests_for(&args, Some(target), 0)?
                } else {
                    Vec::new()
                };
            if !writeback_dests.is_empty() {
                let expected = self.module.function(target).self_writeback_types.len();
                if writeback_dests.len() != expected {
                    return Err(format!(
                        "internal error: call to `{}::{}` has {} writeback dests but callee declared {} writeback returns",
                        self.interner.resolve(struct_name).unwrap_or("?"),
                        self.interner.resolve(fn_name).unwrap_or("?"),
                        writeback_dests.len(),
                        expected,
                    ));
                }
                let ret_dest = result_ty
                    .map(|ty| self.module.function_mut(self.func_id).add_local(ty));
                self.emit(
                    InstKind::CallWithSelfWriteback {
                        target,
                        args: arg_values,
                        ret_dest,
                        ret_ty: result_ty,
                        self_dests: writeback_dests,
                    },
                    None,
                );
                for r in ptr_arg_reloads {
                    r.apply(self);
                }
                return match (ret_dest, result_ty) {
                    (Some(local), Some(ty)) => {
                        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                    }
                    _ => Ok(None),
                };
            }
            let out = self.emit(
                InstKind::Call { target, args: arg_values },
                result_ty,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(out);
        }
        Err(format!(
            "compiler MVP cannot lower {} yet",
            crate::spelling::describe_expr(
                self.interner,
                &Expr::AssociatedFunctionCall(struct_name, fn_name, args)
            )
        ))
    }

    /// `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}` arms used to live
    /// here, lowering directly to `UnaryOp::{Abs, Sqrt}` cranelift
    /// instructions. Step F removed them — `x.abs()` / `x.sqrt()` now
    /// resolve through the prelude's extension-trait impls and reach
    /// `lower_method_call`'s primitive-receiver path (Step D), which
    /// emits a regular call into the prelude's wrapper body that
    /// forwards to `__extern_*` (resolved by `libm_import_name_for`
    /// to the matching libm symbol). String / `is_null` methods stay
    /// interpreter-only as before.
    fn lower_expr_builtin_method_call(
        &mut self,
        receiver: &ExprRef,
        method: frontend::ast::BuiltinMethod,
        args: &[ExprRef],
    ) -> Result<Option<ValueId>, String> {
        // TEST-TOOL T1: `a.concat(b)` reaches the lowering as either
        // `MethodCall` (what string interpolation builds, handled in
        // `try_lower_str_concat_call`) or `BuiltinMethodCall` (what
        // the type checker rewrites a `str` receiver's `concat` into,
        // and what the `assert_eq` parser macro emits). Both are the
        // same `toy_str_concat`; only the first was lowerable, so an
        // `assert_eq` could not be compiled.
        if matches!(method, frontend::ast::BuiltinMethod::StrConcat) {
            if args.len() != 1 {
                return Err(format!(
                    "str.concat takes 1 argument, got {}",
                    args.len()
                ));
            }
            let a = self
                .lower_expr(receiver)?
                .ok_or_else(|| "str.concat receiver produced no value".to_string())?;
            let b = self
                .lower_expr(&args[0])?
                .ok_or_else(|| "str.concat argument produced no value".to_string())?;
            return Ok(self.emit(InstKind::StrConcat { a, b }, Some(Type::Str)));
        }
        // DIAG-DEBUG-FMT-OK: `BuiltinMethod`'s Debug spelling is the
        // method's own name (`StrConcat`, `Substring`), which is what
        // names the gap here.
        Err(format!(
            "compiler MVP cannot lower builtin method yet: {method:?}"
        ))
    }

    /// `with allocator = expr { body }` lowering. #121 Phase B-rest
    /// Item 2: push the allocator handle, increment the with-scope
    /// depth so `terminate_return` / `break` / `continue` know to emit
    /// cleanup pops, then lower the body. On a normal (linear) exit
    /// emit the matching pop here; on an early exit the cleanup
    /// helpers already emitted it before terminating, so we just
    /// decrement the depth without a duplicate pop.
    fn lower_expr_with(
        &mut self,
        allocator_expr: &ExprRef,
        body_expr: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // Phase 5 (Design A scope-bound): detect a
        // **temporary form** for an inline allocator and
        // auto-release the slot at scope exit. Today the
        // recognised forms are `Arena::new()` (no args) and
        // `FixedBuffer::new(<capacity>)` (1 arg). `Global`
        // aliases the process-wide default and needs no
        // drop. Other shapes fall through to the existing
        // wrapper-struct auto-extract path below.
        enum InlineAlloc {
            Arena,
            FixedBuffer(frontend::ast::ExprRef),
        }
        let inline_kind: Option<InlineAlloc> = match self.program.expression.get(allocator_expr) {
            Some(Expr::AssociatedFunctionCall(struct_sym, fn_sym, args)) => {
                let s = self.interner.resolve(struct_sym);
                let f = self.interner.resolve(fn_sym);
                if f == Some("new") && s == Some("Arena") && args.is_empty() {
                    Some(InlineAlloc::Arena)
                } else if f == Some("new") && s == Some("FixedBuffer") && args.len() == 1 {
                    Some(InlineAlloc::FixedBuffer(args[0]))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(kind) = inline_kind {
            // Materialize the temporary as a struct (no synthetic
            // binding name needed — we work directly with the
            // allocated field locals) and register it for auto-drop
            // in a fresh drop scope local to this `with` block.
            // After body exit, popping the drop scope fires the
            // user-defined `drop()` method on the wrapper.
            let (struct_name_str, args_for_call): (&str, Vec<ExprRef>) = match kind {
                InlineAlloc::Arena => ("Arena", vec![]),
                InlineAlloc::FixedBuffer(cap_ref) => ("FixedBuffer", vec![cap_ref]),
            };
            let new_str = "new";
            let struct_sym = self.interner.get(struct_name_str).ok_or_else(|| {
                format!("with: stdlib `{}` symbol not interned", struct_name_str)
            })?;
            let new_sym = self
                .interner
                .get(new_str)
                .ok_or_else(|| "with: `new` symbol not interned".to_string())?;
            let struct_id = self.resolve_struct_instance(struct_sym, None)?;
            let func_id = self
                .resolve_struct_method_func_id(struct_sym, new_sym, struct_id, &[])?
                .ok_or_else(|| {
                    format!(
                        "with: missing FuncId for {}::new",
                        self.interner.resolve(struct_sym).unwrap_or("?")
                    )
                })?;
            let target_ret = self.module.function(func_id).return_type;
            let ret_struct_id = match target_ret {
                crate::ir::Type::Struct(id) => id,
                _ => {
                    return Err(format!(
                        "with: `{}::new` does not return a struct",
                        self.interner.resolve(struct_sym).unwrap_or("?")
                    ))
                }
            };
            // Open a drop scope so the wrapper's `drop()` fires
            // right after the with body exits, not at the
            // enclosing block.
            self.enter_drop_scope();
            // Allocate field locals for the constructed struct,
            // register them for drop, emit CallStruct.
            let field_bindings = self.allocate_struct_fields(ret_struct_id);
            let dests: Vec<crate::ir::LocalId> =
                super::bindings::flatten_struct_locals(&field_bindings)
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect();
            self.register_drop_for_struct_binding(ret_struct_id, &field_bindings);
            let mut arg_values: Vec<ValueId> = Vec::with_capacity(args_for_call.len());
            for a in &args_for_call {
                let v = self
                    .lower_expr(a)?
                    .ok_or_else(|| "with: temporary ctor arg produced no value".to_string())?;
                arg_values.push(v);
            }
            self.emit(
                InstKind::CallStruct {
                    target: func_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            // Find the unique `Allocator`-typed field on the
            // wrapper template and load its local as the with
            // handle.
            let template = self.struct_defs.get(&struct_sym).ok_or_else(|| {
                format!(
                    "with: missing frontend template for `{}`",
                    self.interner.resolve(struct_sym).unwrap_or("?")
                )
            })?;
            let mut alloc_field_name: Option<String> = None;
            for (fname, fty) in &template.fields {
                if matches!(fty, frontend::type_decl::TypeDecl::Allocator) {
                    alloc_field_name = Some(fname.clone());
                }
            }
            let alloc_fname = alloc_field_name.ok_or_else(|| {
                format!(
                    "with: `{}` has no Allocator field",
                    self.interner.resolve(struct_sym).unwrap_or("?")
                )
            })?;
            let fb = field_bindings
                .iter()
                .find(|f| f.name == alloc_fname)
                .ok_or_else(|| "with: Allocator field not found in field bindings".to_string())?;
            let local = match &fb.shape {
                super::bindings::FieldShape::Scalar { local, .. } => *local,
                other => {
                    return Err(format!(
                        "with: Allocator field has unexpected shape ({})",
                        super::bindings::field_shape_name(other)
                    ))
                }
            };
            let handle = self
                .emit(InstKind::LoadLocal(local), Some(crate::ir::Type::U64))
                .expect("LoadLocal returns a value");
            self.emit(InstKind::AllocPush { handle }, None);
            self.with_scope_depth += 1;
            self.with_scope_arena_drops
                .push(super::WithScopeCleanup::None);
            let body_value = self.lower_expr(body_expr)?;
            if !self.is_unreachable() {
                self.emit(InstKind::AllocPop, None);
            }
            self.with_scope_depth -= 1;
            self.with_scope_arena_drops.pop();
            // Fire the user-defined `drop()` on the temporary.
            self.pop_and_emit_drops()?;
            return Ok(body_value);
        }

        // STDLIB-alloc-trait: when the allocator expression
        // resolves to a struct value (a wrapper that impls
        // `Alloc`), look up its single `Allocator`-typed
        // field and emit a LoadLocal of that field instead
        // of trying to lower the struct as a single value.
        // The type checker (`visit_with`) has already
        // verified the conformance + uniqueness.
        //
        // Detection: `value_scalar` returns None for struct
        // bindings (struct values aren't single SSA scalars),
        // so probe via `resolve_field_chain` — if it returns
        // a Struct chain result, take the auto-extract path;
        // otherwise fall through to the scalar handle path.
        let chain_opt = self.resolve_field_chain(allocator_expr).ok();
        let handle = if let Some(super::bindings::FieldChainResult::Struct { struct_id, fields }) = chain_opt {
            // Identify the `Allocator`-typed field by walking
            // the *frontend* StructTemplate (which preserves
            // the source-level `TypeDecl::Allocator`)
            // rather than the IR-level `StructDef.fields`
            // (where `Allocator` and other `u64` fields both
            // lower to `Type::U64` and become indistinguishable).
            // The type checker has already verified there's
            // exactly one such field.
            let base_name = self.module.struct_def(struct_id).base_name;
            let template = self.struct_defs.get(&base_name).ok_or_else(|| {
                format!(
                    "with-allocator: missing frontend template for struct `{}`",
                    self.interner.resolve(base_name).unwrap_or("?")
                )
            })?;
            let mut alloc_field_name: Option<String> = None;
            let mut alloc_field_count = 0;
            for (fname, fty) in &template.fields {
                if matches!(fty, frontend::type_decl::TypeDecl::Allocator) {
                    alloc_field_count += 1;
                    alloc_field_name = Some(fname.clone());
                }
            }
            if alloc_field_count != 1 {
                return Err(format!(
                    "with-allocator: struct `{}` must have exactly one Allocator-typed field, got {}",
                    self.interner.resolve(base_name).unwrap_or("?"),
                    alloc_field_count
                ));
            }
            let fname = alloc_field_name.unwrap();
            let fb = fields
                .iter()
                .find(|f| f.name == fname)
                .ok_or_else(|| format!(
                    "with-allocator: struct binding missing field `{}`",
                    fname
                ))?;
            let local = match &fb.shape {
                super::bindings::FieldShape::Scalar { local, .. } => *local,
                other => return Err(format!(
                    "with-allocator: Allocator field has unexpected shape ({})",
                    super::bindings::field_shape_name(other)
                )),
            };
            self.emit(InstKind::LoadLocal(local), Some(crate::ir::Type::U64))
                .expect("LoadLocal returns a value")
        } else {
            self
                .lower_expr(allocator_expr)?
                .ok_or_else(|| "with-allocator handle expression produced no value".to_string())?
        };
        self.emit(InstKind::AllocPush { handle }, None);
        self.with_scope_depth += 1;
        self.with_scope_arena_drops.push(super::WithScopeCleanup::None);
        let body_value = self.lower_expr(body_expr)?;
        if !self.is_unreachable() {
            self.emit(InstKind::AllocPop, None);
        }
        self.with_scope_depth -= 1;
        self.with_scope_arena_drops.pop();
        Ok(body_value)
    }

    /// Lower the user-facing builtins this MVP supports. Today that's
    /// just `panic("literal")` and `assert(cond, "literal")`. Both are
    /// restricted to a string-literal message because the codegen lays
    /// the message bytes into a static data segment; non-literal
    /// messages would require formatting at runtime.
    /// Source position of an allocation site (MEMORY_PROFILING M2).
    ///
    /// A `SiteId` rather than a packed position, so the report can
    /// name the *file* too: the packed form both runtimes key on is
    /// derived from it at the point of recording, and the tree-walker
    /// — which has no module — computes the same number from the same
    /// location pool.
    pub(super) fn alloc_site(&mut self, call_ref: &ExprRef) -> Option<crate::ir::SiteId> {
        self.site_of(call_ref)
    }

    pub(super) fn lower_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
        call_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::HeapAlloc
            | BuiltinFunction::HeapFree
            | BuiltinFunction::HeapRealloc
            | BuiltinFunction::PtrRead
            | BuiltinFunction::PtrReadTyped(_)
            | BuiltinFunction::PtrRef
            | BuiltinFunction::PtrRefTyped(_)
            | BuiltinFunction::PtrWrite
            | BuiltinFunction::PtrOffset
            | BuiltinFunction::SoaRead
            | BuiltinFunction::SoaWrite => self.lower_builtin_heap_and_pointer(func, args, call_ref),
            BuiltinFunction::StrLen
            | BuiltinFunction::StrToPtr
            | BuiltinFunction::StrFromBytes
            | BuiltinFunction::PtrIsNull
            | BuiltinFunction::PtrEq
            | BuiltinFunction::NullPtr => self.lower_builtin_str_and_ptr_conversion(func, args),
            BuiltinFunction::MemStat(_)
            | BuiltinFunction::RecordAllocatorLayout
            | BuiltinFunction::MemCopy
            | BuiltinFunction::MemMove
            | BuiltinFunction::MemSet
            | BuiltinFunction::MemEq
            | BuiltinFunction::MemFind
            | BuiltinFunction::MemFindSeq
            | BuiltinFunction::CurrentAllocator
            | BuiltinFunction::DefaultAllocator => self.lower_builtin_allocator_and_memory(func, args),
            BuiltinFunction::SizeOf
            | BuiltinFunction::SizeOfType(_)
            | BuiltinFunction::ToString
            | BuiltinFunction::Backtrace
            | BuiltinFunction::Format => self.lower_builtin_reflection(func, args),
            BuiltinFunction::Panic
            | BuiltinFunction::Assert
            | BuiltinFunction::Print
            | BuiltinFunction::Println
            | BuiltinFunction::EPrint
            | BuiltinFunction::EPrintln => self.lower_builtin_diagnostics(func, args),
            BuiltinFunction::Abs
            | BuiltinFunction::Min | BuiltinFunction::Max => self.lower_builtin_numeric(func, args),
            BuiltinFunction::Simd(op) => self.lower_builtin_simd(op, args),
            // No catch-all: `MemMove` / `MemSet` were the last two
            // builtins without a lowering (MEMORY-ACCESS M0), so the
            // match is exhaustive and a new `BuiltinFunction` now
            // fails to compile here instead of failing at run time
            // with "compiler MVP cannot lower builtin yet".
        }
    }

    /// Heap allocation and raw pointer access -- the builtins that reach
    /// the allocator or dereference a `ptr`.
    fn lower_builtin_heap_and_pointer(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
        call_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::HeapAlloc => {
                // #121 Phase A: lower to InstKind::HeapAlloc which
                // codegen turns into a libc malloc call. Default
                // global allocator only — `with allocator = ...`
                // scope plumbing comes in a later phase.
                expect_args(args, 1, "__builtin_heap_alloc takes 1 arg (size)")?;
                let size = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_alloc size produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                let site = self.alloc_site(call_ref);
                Ok(self.emit(InstKind::HeapAlloc { size, binding, site }, Some(Type::U64)))
            }
            BuiltinFunction::HeapFree => {
                expect_args(args, 1, "__builtin_heap_free takes 1 arg (ptr)")?;
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_free ptr produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                Ok(self.emit(InstKind::HeapFree { ptr, binding }, None))
            }
            BuiltinFunction::HeapRealloc => {
                expect_args(args, 2, "__builtin_heap_realloc takes 2 args (ptr, new_size)")?;
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_realloc ptr produced no value".to_string())?;
                let new_size = self.lower_expr(&args[1])?
                    .ok_or_else(|| "heap_realloc new_size produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                // The site attributes a *null* resize — the allocation
                // shape stdlib collections grow through (M2 + D2).
                let site = self.alloc_site(call_ref);
                Ok(self.emit(
                    InstKind::HeapRealloc { ptr, new_size, binding, site },
                    Some(Type::U64),
                ))
            }
            // The untyped read is a parse error (MEMORY-ACCESS); the
            // variant only names the builtin for the parser.
            BuiltinFunction::PtrRead => Err(
                "internal error: an untyped `__builtin_ptr_read` reached lowering".to_string(),
            ),
            // ELEMENT-BORROW E1: a borrow lowers exactly like the read
            // it borrows from — references erase here. The difference
            // lives in the type checker, which keeps drop glue off the
            // binding that names it.
            BuiltinFunction::PtrRef => {
                Err(
                    "`__builtin_ptr_ref(p, off)` carries no width; write \
                     `__builtin_ptr_ref::<TYPE>(p, off)`"
                        .to_string(),
                )
            }
            BuiltinFunction::PtrRefTyped(ty) | BuiltinFunction::PtrReadTyped(ty) => {
                // MEMORY-ACCESS M1: `__builtin_ptr_read::<T>(p, off)`.
                // The width is the written type, so unlike the arm
                // above this is an ordinary expression -- it needs no
                // annotation and no binding around it.
                //
                // A compound `T` still needs the destination binding
                // its leaves are stored into, so it is handled in
                // `let_lowering.rs` and only the scalar case reaches
                // here; a compound read in expression position gets
                // the same "bind it first" answer other compound
                // expressions get in the compiled lanes.
                expect_args(args, 2, "__builtin_ptr_read::<T> takes 2 args (pointer, offset)")?;
                let elem_ty = self
                    .lower_scalar_with_subst(ty)
                    .or_else(|| self.lower_type_arg(ty))
                    .ok_or_else(|| {
                        "__builtin_ptr_read::<T>: unknown element type".to_string()
                    })?;
                if matches!(elem_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                    return Err(
                        "__builtin_ptr_read::<T> with a struct / tuple / enum `T` must be                          bound by a `val` (the read expands into one load per leaf)"
                            .to_string(),
                    );
                }
                let ptr = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_read base produced no value".to_string())?;
                let offset = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_read offset produced no value".to_string())?;
                Ok(self.emit(InstKind::PtrRead { ptr, offset, elem_ty }, Some(elem_ty)))
            }
            BuiltinFunction::PtrWrite => {
                // `__builtin_ptr_write(ptr, offset, value)` — the
                // value's IR type is captured here so codegen can
                // pick the matching `store.<cl_ty>`. The interpreter
                // routes through a typed_slots map; the AOT path
                // takes a direct address and trusts the type-checker
                // to keep reads/writes type-consistent at the same
                // offset (`Dict<K, V>` always reads/writes K-typed
                // values to `keys` and V-typed values to `vals`, so
                // there's no tag-mismatch worry in well-typed code).
                //
                // AOT-COMPOUND-PTR-RW: when `value` is a compound
                // (struct / tuple) the call expands into one
                // `PtrWrite` per leaf scalar at `offset + leaf_off`.
                // `compute_leaf_layout` walks the type to a flat
                // `(byte_offset, scalar_type)` list, and the leaf
                // values come from the binding's leaf locals. Pre-
                // existing scalar callers fall through the single
                // `PtrWrite` path unchanged.
                expect_args(args, 3, "__builtin_ptr_write takes 3 args (ptr, offset, value)")?;
                let value_ty = self
                    .value_scalar(&args[2])
                    .ok_or_else(|| {
                        "__builtin_ptr_write value type unsupported (needs scalar or struct/tuple/enum)"
                            .to_string()
                    })?;
                if matches!(value_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                    let columns = self.soa_columns(value_ty).ok_or_else(|| {
                        format!(
                            "__builtin_ptr_write: unable to compute leaf layout for `{}`",
                            crate::spelling::spell_type(self.module, self.interner, value_ty)
                        )
                    })?;
                    // Resolve the value identifier to its binding
                    // and pull leaf locals via `flatten_struct_locals`
                    // / `flatten_tuple_element_locals` (same paths
                    // method-call argument flattening uses).
                    let leaf_locals =
                        self.compound_leaf_locals(&args[2], "__builtin_ptr_write")?;
                    if leaf_locals.len() != columns.len() {
                        return Err(format!(
                            "__builtin_ptr_write: leaf count mismatch ({} locals vs {} layout entries) — binding likely doesn't match the type-checker's view of `{}`",
                            leaf_locals.len(),
                            columns.len(),
                            crate::spelling::spell_type(self.module, self.interner, value_ty)
                        ));
                    }
                    let ptr = self.lower_expr(&args[0])?
                        .ok_or_else(|| "ptr_write ptr produced no value".to_string())?;
                    let address = self.lower_buffer_address(args, false)?;
                    for (column, (local, _local_ty)) in columns.iter().zip(leaf_locals.iter()) {
                        let leaf_ty = column.2;
                        let off_v = self.emit_leaf_offset(&address, column);
                        let value = self
                            .emit(InstKind::LoadLocal(*local), Some(leaf_ty))
                            .expect("LoadLocal returns a value");
                        self.emit(
                            InstKind::PtrWrite { ptr, offset: off_v, value, value_ty: leaf_ty },
                            None,
                        );
                    }
                    return Ok(None);
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_write ptr produced no value".to_string())?;
                let offset = self.lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_write offset produced no value".to_string())?;
                let value = self.lower_expr(&args[2])?
                    .ok_or_else(|| "ptr_write value produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::PtrWrite { ptr, offset, value, value_ty },
                    None,
                ))
            }
            // DATA-ORIENTED Phase 2: `__builtin_soa_read(p, i, cap)` in
            // expression position. Like `__builtin_ptr_read`, the
            // element type is the annotation's, so the useful form is
            // the let binding `let_lowering.rs` intercepts; anything
            // else has no shape to read into.
            BuiltinFunction::SoaRead => Err(
                "compiler MVP requires `val NAME: TYPE = __builtin_soa_read(...)` \
                 (the element type is taken from the annotation, exactly as for \
                 __builtin_ptr_read)"
                    .to_string(),
            ),
            // `__builtin_soa_write(p, i, cap, value)` — one `PtrWrite`
            // per column of `value`'s type, at
            // `prefix_j * cap + i * stride_j` (see `soa.rs`). The
            // scalar case is one column with `prefix = 0`, which is
            // the plain `i * sizeof(T)` address.
            BuiltinFunction::SoaWrite => {
                expect_args(
                    args,
                    4,
                    "__builtin_soa_write takes 4 args (base, index, cap, value)",
                )?;
                let value_ty = self.value_scalar(&args[3]).ok_or_else(|| {
                    "__builtin_soa_write value type unsupported (needs scalar or struct/tuple/enum)"
                        .to_string()
                })?;
                let columns = self.soa_columns(value_ty).ok_or_else(|| {
                    format!(
                        "__builtin_soa_write: unable to compute column layout for `{}`",
                        crate::spelling::spell_type(self.module, self.interner, value_ty)
                    )
                })?;
                let ptr = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "soa_write base produced no value".to_string())?;
                let address = self.lower_buffer_address(args, true)?;
                // A compound value already lives in leaf locals (the
                // same ones the compound `__builtin_ptr_write`
                // expansion reads); a scalar is one value.
                let leaf_values: Vec<ValueId> =
                    if matches!(value_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                        let leaf_locals =
                            self.compound_leaf_locals(&args[3], "__builtin_soa_write")?;
                        if leaf_locals.len() != columns.len() {
                            return Err(format!(
                                "__builtin_soa_write: leaf count mismatch ({} locals vs {} columns) for `{}`",
                                leaf_locals.len(),
                                columns.len(),
                                crate::spelling::spell_type(self.module, self.interner, value_ty)
                            ));
                        }
                        leaf_locals
                            .iter()
                            .zip(columns.iter())
                            .map(|((local, _), (_, _, leaf_ty))| {
                                self.emit(InstKind::LoadLocal(*local), Some(*leaf_ty))
                                    .expect("LoadLocal returns a value")
                            })
                            .collect()
                    } else {
                        vec![self
                            .lower_expr(&args[3])?
                            .ok_or_else(|| "soa_write value produced no value".to_string())?]
                    };
                for (column, value) in columns.iter().zip(leaf_values.iter()) {
                    let offset = self.emit_leaf_offset(&address, column);
                    self.emit(
                        InstKind::PtrWrite { ptr, offset, value: *value, value_ty: column.2 },
                        None,
                    );
                }
                Ok(None)
            }
            BuiltinFunction::PtrOffset => {
                // `__builtin_ptr_offset(base, offset) -> ptr` is a plain
                // address addition: `ptr` is u64 in the IR, so lowering
                // to `BinOp::Add` reuses every backend's integer add
                // without a new instruction.
                expect_args(args, 2, "__builtin_ptr_offset takes 2 args (base, offset)")?;
                let base = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_offset base produced no value".to_string())?;
                let offset = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_offset offset produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::BinOp {
                        op: crate::ir::BinOp::Add,
                        lhs: base,
                        rhs: offset,
                    },
                    Some(Type::U64),
                ))
            }
            _ => unreachable!("lower_builtin_heap_and_pointer was handed a builtin it does not own"),
        }
    }

    /// The `str` <-> `ptr` boundary, plus the pointer predicates that only
    /// look at addresses.
    fn lower_builtin_str_and_ptr_conversion(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::StrLen => {
                // `__builtin_str_len(s: str) -> u64` — emits an
                // `InstKind::StrLen` that codegen lowers to a libc
                // `strlen` call. The per-literal `.rodata` layout
                // (`[bytes][NUL][u64 len]`) keeps the trailing NUL
                // so strlen's walk terminates correctly; the stored
                // u64 len at the layout's tail is informational
                // for now.
                expect_args(args, 1, "__builtin_str_len takes 1 arg (str)")?;
                let v = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "str_len arg produced no value".to_string())?;
                Ok(self.emit(InstKind::StrLen { value: v }, Some(Type::U64)))
            }
            BuiltinFunction::StrToPtr => {
                // `__builtin_str_to_ptr(s: str) -> ptr`. AOT
                // representation: `Type::Str` is already a pointer-
                // sized handle (i64) into the `.rodata` blob (or a
                // heap-allocated copy). Returning the same value
                // with a `Type::U64` annotation is identity at
                // cranelift level (`ir_to_cranelift_ty(Str)` = I64
                // = `ir_to_cranelift_ty(U64)`); the user's `ptr`
                // can then be fed into `__builtin_ptr_read(p, i)`
                // with a `val: u8` annotation to walk the bytes.
                expect_args(args, 1, "__builtin_str_to_ptr takes 1 arg (str)")?;
                let v = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "str_to_ptr arg produced no value".to_string())?;
                // The str runtime value points at the u64 len field
                // (see ConstStr codegen). Layout `[bytes][NUL][u64
                // len LE]`: byte_start = len_field_addr - 1 (NUL)
                // - len. Compute as a single chain of
                // `load.i64(s, 0)` + `iadd_imm(-1)` + `isub`.
                let len = self
                    .emit(InstKind::StrLen { value: v }, Some(Type::U64))
                    .expect("StrLen returns a value");
                let one = self
                    .emit(
                        InstKind::Const(crate::ir::Const::U64(1)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                let nul_offset = self
                    .emit(
                        InstKind::BinOp {
                            op: crate::ir::BinOp::Add,
                            lhs: len,
                            rhs: one,
                        },
                        Some(Type::U64),
                    )
                    .expect("Add returns a value");
                Ok(self.emit(
                    InstKind::BinOp {
                        op: crate::ir::BinOp::Sub,
                        lhs: v,
                        rhs: nul_offset,
                    },
                    Some(Type::U64),
                ))
            }
            BuiltinFunction::StrFromBytes => {
                expect_args(args, 2, "__builtin_str_from_bytes takes 2 args (ptr, u64)")?;
                let p = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "str_from_bytes ptr produced no value".to_string())?;
                let n = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "str_from_bytes len produced no value".to_string())?;
                Ok(self.emit(InstKind::StrFromBytes { ptr: p, len: n }, Some(Type::Str)))
            }
            BuiltinFunction::PtrIsNull => {
                expect_args(args, 1, "__builtin_ptr_is_null takes 1 arg (ptr)")?;
                let p = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_is_null arg produced no value".to_string())?;
                Ok(self.emit(InstKind::PtrIsNull { ptr: p }, Some(Type::Bool)))
            }
            BuiltinFunction::PtrEq => {
                expect_args(args, 2, "__builtin_ptr_eq takes 2 args (ptr, ptr)")?;
                let a = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_eq arg 0 produced no value".to_string())?;
                let b = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_eq arg 1 produced no value".to_string())?;
                Ok(self.emit(InstKind::PtrEq { a, b }, Some(Type::Bool)))
            }
            BuiltinFunction::NullPtr => {
                expect_args(args, 0, "__builtin_null_ptr takes no args")?;
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(0)), Some(Type::U64)))
            }
            _ => unreachable!("lower_builtin_str_and_ptr_conversion was handed a builtin it does not own"),
        }
    }

    /// Allocation counters, allocator handles, and the bulk memory
    /// operations.
    fn lower_builtin_allocator_and_memory(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::MemStat(stat) => {
                if !args.is_empty() {
                    return Err(format!(
                        "{} takes no args, got {}",
                        stat.builtin_name(),
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::MemStat { stat: stat.code() }, Some(Type::U64)))
            }
            BuiltinFunction::RecordAllocatorLayout => {
                expect_args(
                    args,
                    5,
                    "__builtin_record_allocator_layout takes 5 args (name, managed, live, free_blocks, largest)",
                )?;
                let name = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "record_allocator_layout name produced no value".to_string())?;
                let managed = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "record_allocator_layout managed produced no value".to_string())?;
                let live = self
                    .lower_expr(&args[2])?
                    .ok_or_else(|| "record_allocator_layout live produced no value".to_string())?;
                let free_blocks = self
                    .lower_expr(&args[3])?
                    .ok_or_else(|| "record_allocator_layout free_blocks produced no value".to_string())?;
                let largest = self
                    .lower_expr(&args[4])?
                    .ok_or_else(|| "record_allocator_layout largest produced no value".to_string())?;
                self.emit(
                    InstKind::RecordAllocatorLayout { name, managed, live, free_blocks, largest },
                    None,
                );
                Ok(None)
            }
            BuiltinFunction::MemCopy => {
                // `__builtin_mem_copy(src: ptr, dest: ptr, size: u64)`
                // — emit `InstKind::MemCopy` which codegen lowers
                // to a libc memcpy call (with (dest, src, n)
                // argument-order swap).
                expect_args(args, 3, "__builtin_mem_copy takes 3 args (src, dest, size)")?;
                let src = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_copy src produced no value".to_string())?;
                let dest = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_copy dest produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_copy size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemCopy { src, dest, size }, None))
            }
            BuiltinFunction::MemMove => {
                // `__builtin_mem_move(src: ptr, dest: ptr, size: u64)`
                // — memcpy's overlap-tolerant sibling. Same argument
                // order, same swap in codegen.
                expect_args(args, 3, "__builtin_mem_move takes 3 args (src, dest, size)")?;
                let src = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_move src produced no value".to_string())?;
                let dest = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_move dest produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_move size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemMove { src, dest, size }, None))
            }
            BuiltinFunction::MemSet => {
                // `__builtin_mem_set(dest: ptr, byte: u8, size: u64)`
                // — libc memset. The byte reaches codegen as a `u8`
                // value and is widened there to libc's `int`.
                expect_args(args, 3, "__builtin_mem_set takes 3 args (dest, byte, size)")?;
                let dest = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_set dest produced no value".to_string())?;
                let byte = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_set byte produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_set size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemSet { dest, byte, size }, None))
            }
            BuiltinFunction::MemEq => {
                // MEMORY-ACCESS M3: `__builtin_mem_eq(a, b, size) -> bool`.
                expect_args(args, 3, "__builtin_mem_eq takes 3 args (a, b, size)")?;
                let a = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_eq a produced no value".to_string())?;
                let b = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_eq b produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_eq size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemEq { a, b, size }, Some(Type::Bool)))
            }
            BuiltinFunction::MemFind => {
                // `__builtin_mem_find(p, len, byte) -> u64`.
                expect_args(args, 3, "__builtin_mem_find takes 3 args (p, len, byte)")?;
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_find pointer produced no value".to_string())?;
                let len = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_find length produced no value".to_string())?;
                let byte = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_find byte produced no value".to_string())?;
                Ok(self.emit(InstKind::MemFind { ptr, len, byte }, Some(Type::U64)))
            }
            BuiltinFunction::MemFindSeq => {
                // `__builtin_mem_find_seq(hay, hay_len, needle, needle_len) -> u64`.
                expect_args(
                    args,
                    4,
                    "__builtin_mem_find_seq takes 4 args (hay, hay_len, needle, needle_len)",
                )?;
                let hay = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_find_seq haystack produced no value".to_string())?;
                let hay_len = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_find_seq haystack length produced no value".to_string())?;
                let needle = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_find_seq needle produced no value".to_string())?;
                let needle_len = self.lower_expr(&args[3])?
                    .ok_or_else(|| "mem_find_seq needle length produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::MemFindSeq { hay, hay_len, needle, needle_len },
                    Some(Type::U64),
                ))
            }
            BuiltinFunction::CurrentAllocator => {
                // #121 Phase B-min: read the top of the runtime
                // active-allocator stack (or 0 when empty).
                expect_args(args, 0, "__builtin_current_allocator takes no args")?;
                Ok(self.emit(InstKind::AllocCurrent, Some(Type::U64)))
            }
            BuiltinFunction::DefaultAllocator => {
                // #121 Phase B-min: the default global allocator is
                // represented as the sentinel u64 = 0. The heap path
                // already routes 0-handles to libc malloc.
                expect_args(args, 0, "__builtin_default_allocator takes no args")?;
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(0)), Some(Type::U64)))
            }
            _ => unreachable!("lower_builtin_allocator_and_memory was handed a builtin it does not own"),
        }
    }

    /// Questions a value can answer about itself: its size, its rendering,
    /// its formatted rendering.
    fn lower_builtin_reflection(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::SizeOf => {
                // `__builtin_sizeof(value) -> u64` — at AOT we
                // resolve the byte size at lower time from the
                // value's IR type via `value_scalar`. The active
                // monomorph subst already shows on the value's
                // type because parameter / let bindings store the
                // substituted type. Compound types (struct /
                // tuple / enum) aren't reached today by the
                // user-space collections that drive this; reject
                // them with a precise message rather than
                // silently summing fields.
                expect_args(args, 1, "__builtin_sizeof takes 1 arg")?;
                let arg_ty = self
                    .value_scalar(&args[0])
                    .ok_or_else(|| {
                        "__builtin_sizeof: could not infer arg type at AOT".to_string()
                    })?;
                let size = self.compute_byte_size(arg_ty).ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower __builtin_sizeof of type `{}`",
                        crate::spelling::spell_type(self.module, self.interner, arg_ty)
                    )
                })?;
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(size)), Some(Type::U64)))
            }
            BuiltinFunction::SizeOfType(ty_decl) => {
                // POINTER P1: `__builtin_sizeof::<T>() -> u64` — the
                // type argument form. The written type resolves
                // through the active monomorph subst (a generic
                // parameter becomes the concrete argument this
                // instance was instantiated with) and the size is a
                // compile-time constant, exactly like the value form.
                // Compound types instantiate on demand through
                // `lower_type_with_subst`, so `sizeof::<Point>()` and
                // `sizeof::<Cell<u64>>()` answer from the module's
                // instantiated defs.
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_sizeof::<T> takes no arguments, got {}",
                        args.len()
                    ));
                }
                let subst = self.active_subst.clone();
                let ty = self
                    .lower_type_with_subst(ty_decl, &subst)
                    .ok_or_else(|| {
                        format!(
                            "__builtin_sizeof::<T>: cannot lower the type argument `{}` \
                             at AOT (unknown type or unresolvable generic parameter)",
                            crate::spelling::spell_type_decl(self.interner, ty_decl)
                        )
                    })?;
                let size = self.compute_byte_size(ty).ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower __builtin_sizeof of type `{}`",
                        crate::spelling::spell_type_decl(self.interner, ty_decl)
                    )
                })?;
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(size)), Some(Type::U64)))
            }
            BuiltinFunction::Backtrace => {
                // DEBUG-OBS D5. The engines each read their own stack;
                // the IR only says "ask for it here".
                expect_args(args, 0, "__builtin_backtrace takes no arguments")?;
                Ok(self.emit(InstKind::Backtrace, Some(Type::Str)))
            }
            BuiltinFunction::ToString => {
                // STR-INTERP-AOT: lower to `InstKind::ToString`,
                // which codegen turns into a call to the matching
                // `toy_to_string_<ty>` runtime helper. The arg's
                // IR type is captured here so codegen can pick the
                // right helper without re-inferring later.
                //
                // STR-INTERP-COMPOUND: when arg resolves to a
                // struct identifier, expand into a per-field
                // `ToString(scalar)` + `StrConcat` chain matching
                // the interpreter's `Object::to_display_string`
                // formatting (`TypeName { name: value, ... }`,
                // fields in alphabetical order). Format prefixes
                // are emitted as `ConstStrBytes` so we don't have
                // to round-trip them through the immutable
                // interner.
                expect_args(args, 1, "__builtin_to_string takes 1 argument")?;
                if let Some(Type::Struct(struct_id)) = self.value_scalar(&args[0]) {
                    return self.lower_struct_to_string(struct_id, &args[0]);
                }
                // STR-INTERP-COMPOUND-EXTEND-ENUM: enum-typed
                // identifier — the tag + payload locals live in the
                // `EnumStorage` binding, and the variant is chosen at
                // runtime via a tag-dispatch chain (same shape the
                // print path already uses).
                if let Some(Type::Enum(enum_id)) = self.value_scalar(&args[0]) {
                    return self.lower_enum_to_string(enum_id, &args[0]);
                }
                // RANGE-FOR: `"{r}"` renders `start..end`, the
                // tree-walker's `Object::Range` form.
                if let Some(Expr::Identifier(sym)) = self.program.expression.get(&args[0])
                    && let Some(Binding::Range { start, end, ty }) = self.bindings.get(&sym).cloned()
                {
                    let mut parts = Vec::with_capacity(3);
                    for (i, local) in [start, end].into_iter().enumerate() {
                        if i == 1 {
                            parts.push(
                                self.emit(
                                    InstKind::ConstStrBytes { bytes: b"..".to_vec() },
                                    Some(Type::Str),
                                )
                                .expect("ConstStrBytes returns a value"),
                            );
                        }
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        parts.push(
                            self.emit(InstKind::ToString { value: v, value_ty: ty }, Some(Type::Str))
                                .expect("ToString returns a value"),
                        );
                    }
                    let mut acc = parts[0];
                    for part in &parts[1..] {
                        acc = self
                            .emit(InstKind::StrConcat { a: acc, b: *part }, Some(Type::Str))
                            .expect("StrConcat returns a value");
                    }
                    return Ok(Some(acc));
                }
                // Tuple-typed identifier — `value_scalar` can't
                // surface a Tuple shape (the binding doesn't carry
                // `tuple_id`), so peek the binding directly.
                if let Some(Expr::Identifier(sym)) =
                    self.program.expression.get(&args[0])
                    && matches!(self.bindings.get(&sym), Some(Binding::Tuple { .. })) {
                        return self.lower_tuple_to_string(&args[0]);
                    }
                // Tuple-typed field / element (`"{o.pair}"`) — same
                // blind spot, resolved through the field chain.
                if let Some(arg_expr) = self.program.expression.get(&args[0])
                    && matches!(arg_expr, Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _))
                    && matches!(
                        self.resolve_field_chain(&args[0]),
                        Ok(super::bindings::FieldChainResult::Tuple { .. })
                    )
                {
                    return self.lower_tuple_to_string(&args[0]);
                }
                let arg_value = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "__builtin_to_string arg produced no value".to_string())?;
                let value_ty = self
                    .value_ir_type_for(arg_value)
                    .ok_or_else(|| {
                        "__builtin_to_string: could not infer arg IR type at lower time".to_string()
                    })?;
                if matches!(
                    value_ty,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_) | Type::Unit
                ) {
                    return Err(format!(
                        "compiler MVP cannot lower __builtin_to_string of compound type `{}` \
                         yet — interpolation supports primitives + struct only at AOT",
                        crate::spelling::spell_type(self.module, self.interner, value_ty)
                    ));
                }
                Ok(self.emit(
                    InstKind::ToString { value: arg_value, value_ty },
                    Some(Type::Str),
                ))
            }
            BuiltinFunction::Format => {
                // STR-INTERP-FMT: `__builtin_format(value, spec)`.
                // The spec argument is always the parser's packed
                // constant, so it is read out here and travels as an
                // immediate on the instruction rather than as a
                // value — nothing downstream has to keep a register
                // alive for it.
                expect_args(args, 2, "__builtin_format takes 2 arguments")?;
                let Some(Expr::UInt64(spec)) = self.program.expression.get(&args[1]) else {
                    return Err(
                        "__builtin_format: spec must be the parser-generated u64 constant"
                            .to_string(),
                    );
                };
                let arg_value = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "__builtin_format arg produced no value".to_string())?;
                let value_ty = self
                    .value_ir_type_for(arg_value)
                    .ok_or_else(|| {
                        "__builtin_format: could not infer arg IR type at lower time".to_string()
                    })?;
                if matches!(
                    value_ty,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_) | Type::Unit
                ) {
                    return Err(format!(
                        "__builtin_format of compound type `{}` reached lowering \
                         (the type checker only allows primitives)",
                        crate::spelling::spell_type(self.module, self.interner, value_ty)
                    ));
                }
                Ok(self.emit(
                    InstKind::Format { value: arg_value, value_ty, spec },
                    Some(Type::Str),
                ))
            }
            _ => unreachable!("lower_builtin_reflection was handed a builtin it does not own"),
        }
    }

    /// Builtins that talk to the user or stop the program.
    fn lower_builtin_diagnostics(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Panic => {
                expect_args(args, 1, "panic expects 1 argument")?;
                let site = self.current_site();
                // A literal keeps the interned form: the message needs
                // no code at all, and every existing panic is one.
                match self.expect_string_literal(&args[0], "panic") {
                    Ok(msg_sym) => {
                        self.terminate(Terminator::Panic { message: msg_sym, site });
                    }
                    // ERROR_MODEL E3: anything else is a str *value*,
                    // which `PanicStr` already carries -- it exists so
                    // a contract violation can name the values its
                    // predicate saw. `expect(msg)` is the same need
                    // from the other direction: the caller's message
                    // is the only thing that says why the value had to
                    // be there, and the literal-only rule was throwing
                    // it away on every backend.
                    Err(_) => {
                        let msg = self
                            .lower_expr(&args[0])?
                            .ok_or_else(|| "panic message produced no value".to_string())?;
                        self.terminate(Terminator::PanicStr { message: msg, site });
                    }
                }
                Ok(None)
            }
            BuiltinFunction::Assert => {
                expect_args(args, 2, "assert expects 2 arguments")?;
                // TEST-TOOL T1: the message need not be a literal.
                // `assert_eq` desugars to an `assert` whose message is
                // built from the two values, so requiring a literal
                // here meant **no `test` block containing an
                // `assert_eq` could be compiled at all** — the lanes
                // that ship were the ones that could not be tested,
                // and the bugs `poc/logsearch` actually hit were
                // backend-specific. `panic` already took either form
                // through `PanicStr` (ERROR_MODEL E3); this is the
                // same fallback.
                let literal = self.expect_string_literal(&args[1], "assert").ok();
                let cond = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "assert condition produced no value".to_string())?;
                let pass = self.fresh_block();
                let fail = self.fresh_block();
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: pass,
                    else_blk: fail,
                });
                // Failure block: panic with the assertion message.
                //
                // A computed message is lowered **here**, inside the
                // block that only runs on failure. The language says
                // the message is evaluated only when the condition is
                // false (`docs/language.md`), and building it costs a
                // string concatenation per assertion — hoisting it
                // above the branch would make every passing assert pay
                // for the report it does not print.
                self.switch_to(fail);
                let site = self.current_site();
                match literal {
                    Some(msg_sym) => {
                        self.terminate(Terminator::Panic { message: msg_sym, site })
                    }
                    None => {
                        let msg = self
                            .lower_expr(&args[1])?
                            .ok_or_else(|| "assert message produced no value".to_string())?;
                        self.terminate(Terminator::PanicStr { message: msg, site });
                    }
                }
                // Continue lowering after the assert in the success block.
                self.switch_to(pass);
                Ok(None)
            }
            BuiltinFunction::Print => self.lower_print(args, false),
            BuiltinFunction::Println => self.lower_print(args, true),
            // RUNTIME-LIB P0-A: the same rendering, stamped for the
            // error stream. `print_stderr` is restored afterwards
            // because a compound value's rendering fans out through
            // many emitters and nothing else may inherit the flag.
            BuiltinFunction::EPrint | BuiltinFunction::EPrintln => {
                let newline = matches!(func, BuiltinFunction::EPrintln);
                let saved = self.print_stderr;
                self.print_stderr = true;
                let result = self.lower_print(args, newline);
                self.print_stderr = saved;
                result
            }
            _ => unreachable!("lower_builtin_diagnostics was handed a builtin it does not own"),
        }
    }

    /// The numeric builtins the lowerer still handles directly.
    fn lower_builtin_numeric(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Abs => {
                expect_args(args, 1, "abs expects 1 argument")?;
                let operand = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "abs operand produced no value".to_string())?;
                // Result type matches the operand: i64 -> i64,
                // f64 -> f64. Codegen branches on the operand IR
                // type to pick `fabs` vs the integer select chain.
                let result_ty = self
                    .value_ir_type_for(operand)
                    .filter(|t| matches!(t, Type::I64 | Type::F64))
                    .ok_or_else(|| {
                        "abs expects an i64 or f64 operand".to_string()
                    })?;
                Ok(self.emit(
                    InstKind::UnaryOp { op: crate::ir::UnaryOp::Abs, operand },
                    Some(result_ty),
                ))
            }
            // NOTE: f64 math arms (Sqrt/Pow and Sin..=Ceil) lived
            // here before Phase 4. Each is now declared as
            // `extern fn __extern_*_f64` in math.t and lowered
            // through `lower/program::libm_import_name_for` —
            // the call site emits a regular cranelift call against
            // the imported libm symbol.
            BuiltinFunction::Min | BuiltinFunction::Max => {
                if args.len() != 2 {
                    let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                    return Err(format!("{name} expects 2 arguments, got {}", args.len()));
                }
                let lhs = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "min/max arg0 produced no value".to_string())?;
                let rhs = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "min/max arg1 produced no value".to_string())?;
                let result_ty = self
                    .value_ir_type_for(lhs)
                    .ok_or_else(|| "min/max operand type unknown".to_string())?;
                let op = if matches!(func, BuiltinFunction::Min) {
                    crate::ir::BinOp::Min
                } else {
                    crate::ir::BinOp::Max
                };
                Ok(self.emit(
                    InstKind::BinOp { op, lhs, rhs },
                    Some(result_ty),
                ))
            }
            _ => unreachable!("lower_builtin_numeric was handed a builtin it does not own"),
        }
    }


}
