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

use frontend::ast::{BuiltinFunction, Expr, ExprRef, UnaryOp};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::FunctionLower;
use crate::ir::{Const, InstKind, Terminator, Type, ValueId};

impl<'a> FunctionLower<'a> {
    pub(super) fn lower_call_args(&mut self, args_ref: &ExprRef) -> Result<Vec<ValueId>, String> {
        self.lower_call_args_with_target(args_ref, None)
    }

    /// Variant that knows the callee's `param_is_ref` flags so it
    /// can forward the pointer when a bare `RefScalar` identifier
    /// is passed to a `&T` parameter (instead of dereferencing —
    /// the default path that would corrupt
    /// `outer(x: &u64) -> u64 { inner(x) }`-style chains).
    pub(super) fn lower_call_args_with_target(
        &mut self,
        args_ref: &ExprRef,
        target: Option<crate::ir::FuncId>,
    ) -> Result<Vec<ValueId>, String> {
        let param_is_ref: Vec<bool> = target
            .map(|t| self.module.function(t).param_is_ref.clone())
            .unwrap_or_default();
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let items: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        let mut values: Vec<ValueId> = Vec::with_capacity(items.len());
        for (arg_idx, a) in items.iter().enumerate() {
            // REF-Stage-2 (b)+(c)+(g): explicit `&<var>` / `&mut <var>`
            // borrow of a SCALAR local emits an `AddressOf` and marks
            // the local as address-taken so codegen allocates it in a
            // stack slot. The function's `&T` / `&mut T` parameter on
            // the other side reads / writes through this pointer via
            // LoadRef / StoreRef.
            //
            // Compound borrows (struct / tuple / enum) still go through
            // the leaf-flatten erasure path — those are out of scope
            // for the scalar-only true-pointer phase.
            if let Some(Expr::Unary(op, inner)) = self.program.expression.get(a) {
                if matches!(op, UnaryOp::Borrow | UnaryOp::BorrowMut) {
                    if let Some(Expr::Identifier(sym)) = self.program.expression.get(&inner) {
                        if let Some(Binding::Scalar { local, ty }) =
                            self.bindings.get(&sym).cloned()
                        {
                            if matches!(
                                ty,
                                Type::I64 | Type::U64 | Type::F64 | Type::Bool
                                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                                    | Type::I32 | Type::U32
                            ) {
                                self.module
                                    .function_mut(self.func_id)
                                    .address_taken_locals
                                    .insert(local);
                                let v = self
                                    .emit(InstKind::AddressOf { local }, Some(Type::U64))
                                    .expect("AddressOf returns a value");
                                values.push(v);
                                continue;
                            }
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
                            values.push(v);
                            continue;
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
                    ) {
                        if let Ok(super::bindings::FieldChainResult::Scalar { local, ty }) =
                            self.resolve_field_chain(&inner)
                        {
                            if matches!(
                                ty,
                                Type::I64 | Type::U64 | Type::F64 | Type::Bool
                                    | Type::I8 | Type::U8 | Type::I16 | Type::U16
                                    | Type::I32 | Type::U32
                            ) {
                                self.module
                                    .function_mut(self.func_id)
                                    .address_taken_locals
                                    .insert(local);
                                let v = self
                                    .emit(InstKind::AddressOf { local }, Some(Type::U64))
                                    .expect("AddressOf returns a value");
                                values.push(v);
                                continue;
                            }
                        }
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
                    {
                        if matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                            if let Some(Expr::Identifier(arr_sym)) =
                                self.program.expression.get(&arr_expr)
                            {
                                if let Some(Binding::Array { element_ty, slot, .. }) =
                                    self.bindings.get(&arr_sym).cloned()
                                {
                                    if matches!(
                                        element_ty,
                                        Type::I64 | Type::U64 | Type::F64 | Type::Bool
                                            | Type::I8 | Type::U8 | Type::I16 | Type::U16
                                            | Type::I32 | Type::U32
                                    ) {
                                        if let Some(idx_ref) = info.start {
                                            let idx_v = self
                                                .lower_expr(&idx_ref)?
                                                .ok_or_else(|| {
                                                    "array index produced no value".to_string()
                                                })?;
                                            let v = self
                                                .emit(
                                                    InstKind::ArrayElemAddr {
                                                        slot,
                                                        index: idx_v,
                                                        elem_ty: element_ty,
                                                    },
                                                    Some(Type::U64),
                                                )
                                                .expect("ArrayElemAddr returns a value");
                                            values.push(v);
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // REF-Stage-2: fall back — peel an explicit borrow so the
            // same identifier-expansion path below runs (compound
            // borrows / non-identifier operands).
            let arg_expr_ref = match self.program.expression.get(a) {
                Some(Expr::Unary(op, inner))
                    if matches!(op, UnaryOp::Borrow | UnaryOp::BorrowMut) =>
                {
                    inner
                }
                _ => *a,
            };
            // Struct-typed identifier argument: expand into per-field
            // values in declaration order. Anything else flows through
            // `lower_expr`.
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(&arg_expr_ref) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
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
                // REF-Stage-2 (iv): bare identifier of a `RefScalar`
                // binding being passed to a `&T` parameter — forward
                // the pointer (not the dereferenced value). Without
                // this, `outer(x: &u64) { inner(x) }` would `LoadRef`
                // x to a u64 value and pass it where `inner` expects
                // a pointer, segfaulting at the next `LoadRef`.
                if let Some(Binding::RefScalar { local, .. }) =
                    self.bindings.get(&sym).cloned()
                {
                    if param_is_ref.get(arg_idx).copied().unwrap_or(false) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(Type::U64))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                        continue;
                    }
                }
                // REF-Stage-2 (iv): T -> &T auto-borrow at the AOT
                // boundary. The frontend type checker already
                // approved the conversion (passing a `T` value to a
                // `&T` parameter); the lowering needs to materialise
                // the address. Same shape as the explicit `&<var>`
                // path: mark the local address-taken and emit
                // `AddressOf`.
                if let Some(Binding::Scalar { local, ty }) =
                    self.bindings.get(&sym).cloned()
                {
                    if param_is_ref.get(arg_idx).copied().unwrap_or(false)
                        && matches!(
                            ty,
                            Type::I64 | Type::U64 | Type::F64 | Type::Bool
                                | Type::I8 | Type::U8 | Type::I16 | Type::U16
                                | Type::I32 | Type::U32
                        )
                    {
                        self.module
                            .function_mut(self.func_id)
                            .address_taken_locals
                            .insert(local);
                        let v = self
                            .emit(InstKind::AddressOf { local }, Some(Type::U64))
                            .expect("AddressOf returns a value");
                        values.push(v);
                        continue;
                    }
                }
            }
            // Note: pass the borrow-peeled ref so explicit `&v` /
            // `&mut v` lowers via the inner expr's normal path.
            let v = self
                .lower_expr(&arg_expr_ref)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            values.push(v);
        }
        Ok(values)
    }


    // -- expression lowering -------------------------------------------------------

    pub(super) fn lower_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing expr".to_string())?;
        if self.is_unreachable() {
            return Ok(None);
        }
        match expr {
            Expr::Block(stmts) => {
                // Phase 5 (汎用 RAII): every block opens a fresh
                // drop scope. `Drop`-impling bindings registered
                // in the body get drained at exit (linear here;
                // early `return` / `break` / `continue` paths
                // emit drops via `terminate_return` /
                // `Stmt::Break` / `Stmt::Continue` before
                // terminating). Errors propagate without running
                // drops — same panic-safety policy the
                // interpreter uses.
                self.enter_drop_scope();
                let mut last: Option<ValueId> = None;
                for s in &stmts {
                    last = self.lower_stmt(s)?;
                    if self.is_unreachable() {
                        break;
                    }
                }
                self.pop_and_emit_drops()?;
                Ok(last)
            }
            Expr::Int64(v) => Ok(self.emit(InstKind::Const(Const::I64(v)), Some(Type::I64))),
            Expr::UInt64(v) => Ok(self.emit(InstKind::Const(Const::U64(v)), Some(Type::U64))),
            Expr::Float64(v) => Ok(self.emit(InstKind::Const(Const::F64(v)), Some(Type::F64))),
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::String(sym) => {
                // String literals in value position emit `ConstStr`,
                // which materialises a pointer-sized handle to the
                // shared `.rodata` blob (the same one `PrintStr` uses
                // for `print("literal")`).
                let bytes_len = self
                    .interner
                    .resolve(sym)
                    .map(|s| s.as_bytes().len() as u64)
                    .unwrap_or(0);
                Ok(self.emit(
                    InstKind::ConstStr { message: sym, bytes_len },
                    Some(Type::Str),
                ))
            }
            Expr::Identifier(sym) => {
                match self.bindings.get(&sym).cloned() {
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
                            // Phase 6: capturing closures can't yet be
                            // surfaced as a fn-pointer value (would
                            // need a heap-allocated thunk that bundles
                            // env_ptr + fn_ptr). Reject explicitly so
                            // the user gets a precise message instead
                            // of a downstream signature mismatch.
                            if link.env_ptr.is_some() {
                                return Err(format!(
                                    "compiler MVP: capturing closure `{}` cannot yet be passed as a function value (Phase 5b/6 wiring); call it directly instead",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
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
            Expr::FieldAccess(obj, field) => {
                self.pending_struct_value = None;
                self.lower_field_access(&obj, field)
            }
            Expr::TupleAccess(tuple, index) => {
                self.pending_struct_value = None;
                self.lower_tuple_access(&tuple, index)
            }
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
                // Module-qualified call (`math::add(args)`): when the
                // qualifier doesn't refer to a struct / enum and the
                // function exists in the (post-import) main function
                // table, treat it as a plain `Call`. Module
                // integration flattens imported `pub fn`s in, so the
                // bare lookup hits without needing the qualifier.
                // Real associated calls (`Container::new(...)`) keep
                // the unsupported reject path below.
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
                    self.module
                        .lookup_function(Some(struct_name), fn_name)
                        .or_else(|| self.module.lookup_function(None, fn_name))
                } else {
                    None
                };
                if let Some(target) = target_opt {
                    let ret_ty = self.module.function(target).return_type;
                    if matches!(ret_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                        return Err(format!(
                            "compiler MVP cannot use a compound-returning module call (`{}::{}`) in expression position; bind the result with `val`",
                            self.interner.resolve(struct_name).unwrap_or("?"),
                            self.interner.resolve(fn_name).unwrap_or("?"),
                        ));
                    }
                    let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
                    for a in &args {
                        let v = self
                            .lower_expr(a)?
                            .ok_or_else(|| {
                                "module function arg produced no value".to_string()
                            })?;
                        arg_values.push(v);
                    }
                    let result_ty = if ret_ty.produces_value() {
                        Some(ret_ty)
                    } else {
                        None
                    };
                    return Ok(self.emit(
                        InstKind::Call { target, args: arg_values },
                        result_ty,
                    ));
                }
                Err(format!(
                    "compiler MVP cannot lower expression yet: {:?}",
                    Expr::AssociatedFunctionCall(struct_name, fn_name, args)
                ))
            }
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args),
            Expr::Cast(inner, target_ty) => self.lower_cast(&inner, &target_ty),
            Expr::Match(scrutinee, arms) => self.lower_match(&scrutinee, &arms),
            Expr::MethodCall(obj, method, args) => self.lower_method_call(&obj, method, &args),
            Expr::BuiltinMethodCall(_receiver, method, _args) => {
                // NOTE: `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}`
                // arms used to live here, lowering directly to
                // `UnaryOp::{Abs, Sqrt}` cranelift instructions.
                // Step F removed them — `x.abs()` / `x.sqrt()` now
                // resolve through the prelude's extension-trait
                // impls and reach `lower_method_call`'s
                // primitive-receiver path (Step D), which emits a
                // regular call into the prelude's wrapper body that
                // forwards to `__extern_*` (resolved by
                // `libm_import_name_for` to the matching libm
                // symbol). String / `is_null` methods stay
                // interpreter-only as before.
                Err(format!(
                    "compiler MVP cannot lower builtin method yet: {:?}",
                    method
                ))
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
            Expr::UInt32(n) => Ok(self.emit(InstKind::Const(Const::U32(n)), Some(Type::U32))),
            // #121 Phase B-rest Item 2: `with allocator = expr { body }`.
            // Push the allocator handle, increment the with-scope
            // depth so `terminate_return` / `break` / `continue`
            // know to emit cleanup pops, then lower the body. On
            // a normal (linear) exit emit the matching pop here;
            // on an early exit the cleanup helpers already
            // emitted it before terminating, so we just decrement
            // the depth without a duplicate pop.
            Expr::With(allocator_expr, body_expr) => {
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
                let inline_kind: Option<InlineAlloc> = match self.program.expression.get(&allocator_expr) {
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
                    // #121 Phase B-rest leftover (1): the **raw builtin**
                    // forms `__builtin_arena_allocator()` and
                    // `__builtin_fixed_buffer_allocator(cap)` are
                    // shorthand for `Arena::new()` / `FixedBuffer::new()`
                    // — let them auto-drop too so users don't have to
                    // call `__builtin_arena_drop` manually.
                    Some(Expr::BuiltinCall(frontend::ast::BuiltinFunction::ArenaAllocator, args))
                        if args.is_empty() =>
                    {
                        Some(InlineAlloc::Arena)
                    }
                    Some(Expr::BuiltinCall(frontend::ast::BuiltinFunction::FixedBufferAllocator, args))
                        if args.len() == 1 =>
                    {
                        Some(InlineAlloc::FixedBuffer(args[0]))
                    }
                    _ => None,
                };
                if let Some(kind) = inline_kind {
                    // Construct the handle inline so we don't have
                    // to lower the associated-function call as a
                    // struct-returning temporary binding.
                    let (handle, cleanup) = match kind {
                        InlineAlloc::Arena => {
                            let h = self
                                .emit(InstKind::AllocArena, Some(crate::ir::Type::U64))
                                .expect("AllocArena returns a value");
                            (h, super::WithScopeCleanup::ArenaDrop(h))
                        }
                        InlineAlloc::FixedBuffer(cap_ref) => {
                            let cap_v = self
                                .lower_expr(&cap_ref)?
                                .ok_or_else(|| "FixedBuffer::new(cap): capacity produced no value".to_string())?;
                            let h = self
                                .emit(
                                    InstKind::AllocFixedBuffer { capacity: cap_v },
                                    Some(crate::ir::Type::U64),
                                )
                                .expect("AllocFixedBuffer returns a value");
                            (h, super::WithScopeCleanup::FixedBufferDrop(h))
                        }
                    };
                    self.emit(InstKind::AllocPush { handle }, None);
                    self.with_scope_depth += 1;
                    self.with_scope_arena_drops.push(cleanup);
                    let body_value = self.lower_expr(&body_expr)?;
                    if !self.is_unreachable() {
                        // Linear exit: matching pop + drop here.
                        // Early exits (`return` / `break` /
                        // `continue`) already issued both via
                        // `emit_with_scope_cleanup` so we don't
                        // duplicate.
                        self.emit(InstKind::AllocPop, None);
                        match cleanup {
                            super::WithScopeCleanup::ArenaDrop(h) => {
                                self.emit(InstKind::AllocArenaDrop { handle: h }, None);
                            }
                            super::WithScopeCleanup::FixedBufferDrop(h) => {
                                self.emit(InstKind::AllocFixedBufferDrop { handle: h }, None);
                            }
                            super::WithScopeCleanup::None => {}
                        }
                    }
                    self.with_scope_depth -= 1;
                    self.with_scope_arena_drops.pop();
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
                let chain_opt = self.resolve_field_chain(&allocator_expr).ok();
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
                            "with-allocator: Allocator field has unexpected shape {:?}",
                            other
                        )),
                    };
                    self.emit(InstKind::LoadLocal(local), Some(crate::ir::Type::U64))
                        .expect("LoadLocal returns a value")
                } else {
                    self
                        .lower_expr(&allocator_expr)?
                        .ok_or_else(|| "with-allocator handle expression produced no value".to_string())?
                };
                self.emit(InstKind::AllocPush { handle }, None);
                self.with_scope_depth += 1;
                self.with_scope_arena_drops.push(super::WithScopeCleanup::None);
                let body_value = self.lower_expr(&body_expr)?;
                if !self.is_unreachable() {
                    self.emit(InstKind::AllocPop, None);
                }
                self.with_scope_depth -= 1;
                self.with_scope_arena_drops.pop();
                Ok(body_value)
            }
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
            Expr::Closure { params, return_type, body } => {
                self.lift_closure_inline(&params, &return_type, &body)
            }
            other => Err(format!(
                "compiler MVP cannot lower expression yet: {:?}",
                other
            )),
        }
    }

    /// Lower the user-facing builtins this MVP supports. Today that's
    /// just `panic("literal")` and `assert(cond, "literal")`. Both are
    /// restricted to a string-literal message because the codegen lays
    /// the message bytes into a static data segment; non-literal
    /// messages would require formatting at runtime.
    pub(super) fn lower_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Panic => {
                if args.len() != 1 {
                    return Err(format!("panic expects 1 argument, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[0], "panic")?;
                self.terminate(Terminator::Panic { message: msg_sym });
                Ok(None)
            }
            BuiltinFunction::Assert => {
                if args.len() != 2 {
                    return Err(format!("assert expects 2 arguments, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[1], "assert")?;
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
                self.switch_to(fail);
                self.terminate(Terminator::Panic { message: msg_sym });
                // Continue lowering after the assert in the success block.
                self.switch_to(pass);
                Ok(None)
            }
            BuiltinFunction::Print => self.lower_print(args, false),
            BuiltinFunction::Println => self.lower_print(args, true),
            BuiltinFunction::Abs => {
                if args.len() != 1 {
                    return Err(format!("abs expects 1 argument, got {}", args.len()));
                }
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
            BuiltinFunction::HeapAlloc => {
                // #121 Phase A: lower to InstKind::HeapAlloc which
                // codegen turns into a libc malloc call. Default
                // global allocator only — `with allocator = ...`
                // scope plumbing comes in a later phase.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_heap_alloc takes 1 arg (size), got {}",
                        args.len()
                    ));
                }
                let size = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_alloc size produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                Ok(self.emit(InstKind::HeapAlloc { size, binding }, Some(Type::U64)))
            }
            BuiltinFunction::HeapRealloc => {
                if args.len() != 2 {
                    return Err(format!(
                        "__builtin_heap_realloc takes 2 args (ptr, new_size), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_realloc ptr produced no value".to_string())?;
                let new_size = self.lower_expr(&args[1])?
                    .ok_or_else(|| "heap_realloc new_size produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                Ok(self.emit(InstKind::HeapRealloc { ptr, new_size, binding }, Some(Type::U64)))
            }
            BuiltinFunction::HeapFree => {
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_heap_free takes 1 arg (ptr), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_free ptr produced no value".to_string())?;
                let binding = self.classify_active_allocator_binding();
                Ok(self.emit(InstKind::HeapFree { ptr, binding }, None))
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
                if args.len() != 3 {
                    return Err(format!(
                        "__builtin_ptr_write takes 3 args (ptr, offset, value), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_write ptr produced no value".to_string())?;
                let offset = self.lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_write offset produced no value".to_string())?;
                // Peek the value's IR type before lowering so the
                // store width is preserved; lower the value as usual.
                let value_ty = self
                    .value_scalar(&args[2])
                    .ok_or_else(|| {
                        "__builtin_ptr_write value type unsupported (needs scalar)"
                            .to_string()
                    })?;
                let value = self.lower_expr(&args[2])?
                    .ok_or_else(|| "ptr_write value produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::PtrWrite { ptr, offset, value, value_ty },
                    None,
                ))
            }
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
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_sizeof takes 1 arg, got {}",
                        args.len()
                    ));
                }
                let arg_ty = self
                    .value_scalar(&args[0])
                    .ok_or_else(|| {
                        "__builtin_sizeof: could not infer arg type at AOT".to_string()
                    })?;
                let size = match arg_ty {
                    Type::I8 | Type::U8 | Type::Bool => 1u64,
                    Type::I16 | Type::U16 => 2,
                    Type::I32 | Type::U32 => 4,
                    Type::I64 | Type::U64 | Type::F64 | Type::Str => 8,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_) | Type::Unit => {
                        return Err(format!(
                            "compiler MVP cannot lower __builtin_sizeof of compound type \
                             {arg_ty:?} yet"
                        ));
                    }
                };
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(size)), Some(Type::U64)))
            }
            BuiltinFunction::MemCopy => {
                // `__builtin_mem_copy(src: ptr, dest: ptr, size: u64)`
                // — emit `InstKind::MemCopy` which codegen lowers
                // to a libc memcpy call (with (dest, src, n)
                // argument-order swap).
                if args.len() != 3 {
                    return Err(format!(
                        "__builtin_mem_copy takes 3 args (src, dest, size), got {}",
                        args.len()
                    ));
                }
                let src = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_copy src produced no value".to_string())?;
                let dest = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_copy dest produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_copy size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemCopy { src, dest, size }, None))
            }
            BuiltinFunction::StrLen => {
                // `__builtin_str_len(s: str) -> u64` — emits an
                // `InstKind::StrLen` that codegen lowers to a libc
                // `strlen` call. The per-literal `.rodata` layout
                // (`[bytes][NUL][u64 len]`) keeps the trailing NUL
                // so strlen's walk terminates correctly; the stored
                // u64 len at the layout's tail is informational
                // for now.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_str_len takes 1 arg (str), got {}",
                        args.len()
                    ));
                }
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
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_str_to_ptr takes 1 arg (str), got {}",
                        args.len()
                    ));
                }
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
            BuiltinFunction::PtrRead => {
                // `__builtin_ptr_read(ptr, offset)` — return type comes
                // from the surrounding `val`/`var` annotation. The
                // generic version (no annotation) is rejected here so
                // the user gets a clear error pointing at the missing
                // type hint. The let-binding lowering path
                // (`let_lowering.rs::lower_let`) handles
                // `val x: T = __builtin_ptr_read(...)` directly and
                // never reaches this arm.
                Err(
                    "compiler MVP requires `val NAME: TYPE = __builtin_ptr_read(...)` \
                     (the read width is taken from the annotation; bare expression-position \
                     uses are not supported in AOT yet)"
                        .to_string(),
                )
            }
            BuiltinFunction::DefaultAllocator => {
                // #121 Phase B-min: the default global allocator is
                // represented as the sentinel u64 = 0. The heap path
                // already routes 0-handles to libc malloc.
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_default_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(0)), Some(Type::U64)))
            }
            BuiltinFunction::CurrentAllocator => {
                // #121 Phase B-min: read the top of the runtime
                // active-allocator stack (or 0 when empty).
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_current_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::AllocCurrent, Some(Type::U64)))
            }
            BuiltinFunction::PtrIsNull => {
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_ptr_is_null takes 1 arg (ptr), got {}",
                        args.len()
                    ));
                }
                let p = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_is_null arg produced no value".to_string())?;
                Ok(self.emit(InstKind::PtrIsNull { ptr: p }, Some(Type::Bool)))
            }
            BuiltinFunction::ArenaAllocator => {
                // #121 Phase B-rest Item 1: allocate an arena slot
                // in the runtime registry and return its handle.
                // The handle is a non-zero u64 so heap_alloc /
                // realloc / free can dispatch on it.
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_arena_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::AllocArena, Some(Type::U64)))
            }
            BuiltinFunction::ArenaDrop => {
                // #121 Phase B-rest Item 2 follow-up: explicit
                // arena bulk-free. Caller hands in the handle
                // returned by `__builtin_arena_allocator()`.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_arena_drop takes 1 arg (handle), got {}",
                        args.len()
                    ));
                }
                let h = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "arena_drop handle arg produced no value".to_string())?;
                self.emit(InstKind::AllocArenaDrop { handle: h }, None);
                Ok(None)
            }
            BuiltinFunction::FixedBufferDrop => {
                // Phase 5: explicit fixed_buffer bulk-free.
                // Caller hands in the handle returned by
                // `__builtin_fixed_buffer_allocator(cap)`.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_fixed_buffer_drop takes 1 arg (handle), got {}",
                        args.len()
                    ));
                }
                let h = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "fixed_buffer_drop handle arg produced no value".to_string())?;
                self.emit(InstKind::AllocFixedBufferDrop { handle: h }, None);
                Ok(None)
            }
            BuiltinFunction::FixedBufferAllocator => {
                // #121 Phase B-rest Item 1: capacity-limited allocator.
                // Subsequent allocations through this handle that
                // would exceed `capacity` return 0 (null).
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_fixed_buffer_allocator takes 1 arg (capacity), got {}",
                        args.len()
                    ));
                }
                let cap = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "fixed_buffer capacity arg produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::AllocFixedBuffer { capacity: cap },
                    Some(Type::U64),
                ))
            }
            other => Err(format!(
                "compiler MVP cannot lower builtin yet: {:?}",
                other
            )),
        }
    }

}
