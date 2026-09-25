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

use super::array_layout::{leaf_scalar_count, leaf_type_at};
use super::bindings::{
    flatten_enum_storage_locals, flatten_struct_locals, flatten_tuple_element_locals, ArrayStorage,
    Binding, FieldChainResult, TupleElementBinding,
};
use super::FunctionLower;
use crate::ir::{Const, EnumId, InstKind, LocalId, StructId, Type, ValueId};

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
        // DEBUG-OBS D3/D4: "where we are" is the right-hand side.
        // Most of this function dispatches on the rhs *shape* and
        // emits the call itself rather than going through
        // `lower_expr`, which is the only other place that records
        // it — so a compound-returning call had no position, and the
        // frame it entered was attributed to whatever had been
        // lowered last. A backtrace said `make (called at line 14)`
        // for a call on line 10, while the tree-walker said 10.
        let outer_expr = self.current_expr.replace(*rhs_ref);
        let result = self.lower_let_inner(name, annotation, rhs_ref);
        self.current_expr = outer_expr;
        result
    }

    fn lower_let_inner(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // CONTRACT-ELISION: this binding takes over the name from here
        // on, so whatever a `requires` clause proved about a parameter
        // of the same name no longer describes what a guard site would
        // read. Dropped before the rhs is lowered, which is the
        // conservative order — a guard inside the rhs referring to the
        // parameter keeps its check.
        self.facts.shadowed(name);
        let rhs = self
            .program
            .expression
            .get(rhs_ref)
            .ok_or_else(|| "let rhs missing".to_string())?;
        // STDLIB-TRAIT-BASE B5: inside a monomorphised body, `T::assoc()`
        // names a type parameter where every path below expects a
        // declared type. Substituting the qualifier here — on the local
        // copy, never in the pool, which is shared by every instance of
        // the template — lets all of them work unchanged.
        let rhs = self.substitute_assoc_qualifier(rhs);
        // Closures Phase 5a: `val name = fn(params) -> R { body }`
        // lifts to a synthesized top-level function. The closure
        // literal isn't materialised as a runtime value (Phase 5a
        // doesn't model fn-pointer values yet); instead we register
        // `name -> FuncId` in `closure_bindings` so a subsequent
        // `name(args)` call resolves to a direct `Call`. Captures
        // are rejected up-front because Phase 5a can't pass them
        // into the lifted body — those land in Phase 6.
        if let Expr::Closure { params, return_type, body, captures_by_ref } = rhs.clone() {
            return self.lift_closure_binding(
                name,
                &params,
                &return_type,
                &body,
                captures_by_ref,
            );
        }
        // Tuple-literal RHS: allocate one local per element. Like
        // structs, tuples never flow through the IR's value graph;
        // the only way to consume one is via `t.N` element access on a
        // bound name. The parser desugars `val (a, b) = e` into
        // `val tmp = e; val a = tmp.0; val b = tmp.1`, so this branch
        // also handles destructuring.
        if let Expr::TupleLiteral(elems) = rhs.clone() {
            return self.lower_let_tuple_literal(name, elems);
        }
        // RANGE-FOR: `val r = a..b`, or a copy of another range.
        if let Expr::Range(start, end) = rhs.clone() {
            return self.lower_let_range(name, &start, &end);
        }
        if let Expr::Identifier(src) = rhs.clone()
            && let Some(Binding::Range { start, end, ty }) = self.bindings.get(&src).cloned()
        {
            let func = self.module.function_mut(self.func_id);
            let (new_start, new_end) = (func.add_local(ty), func.add_local(ty));
            for (from, to) in [(start, new_start), (end, new_end)] {
                let v = self
                    .emit(InstKind::LoadLocal(from), Some(ty))
                    .expect("LoadLocal returns a value");
                self.emit(InstKind::StoreLocal { dst: to, src: v }, None);
            }
            self.bindings.insert(name, Binding::Range { start: new_start, end: new_end, ty });
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
        // fixed-length array binding whose backing slots mirror the
        // source layout (an explicit annotation decides otherwise —
        // DATA-ORIENTED: `val sub: [P; 2] = soa_ps[1..3]` re-layouts,
        // no annotation keeps the source's placement).
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone()
            && matches!(info.slice_type, frontend::ast::SliceType::RangeSlice) {
                // Only an *array* annotation has an opinion on
                // placement. An unannotated `val` carries
                // `TypeDecl::Unknown` rather than `None`, so asking
                // `is_soa()` about every annotation answers "AoS" for
                // the no-annotation case and silently drops the
                // source's layout on the floor.
                let dst_soa = annotation.and_then(|a| match a {
                    TypeDecl::Array(_, _, soa) => Some(*soa),
                    _ => None,
                });
                return self.lower_let_range_slice(name, arr_obj, info, dst_soa);
            }
        // Compound-element array read: `val p: Point = arr[i]`.
        // Allocate the right binding shape and load each leaf
        // directly into its locals via the same per-leaf
        // ArrayLoad sequence `lower_slice_access` would emit, so
        // chain access (`p.x`) and field-by-field reads work
        // through the existing struct-binding path.
        if let Expr::SliceAccess(arr_obj, info) = rhs.clone()
            && matches!(info.slice_type, frontend::ast::SliceType::SingleElement)
                && let Some(result) =
                    self.lower_let_slice_single_element(name, arr_obj, info)?
                {
                    return Ok(result);
                }
        if let Expr::ArrayLiteral(elems) = rhs.clone() {
            // DATA-ORIENTED: a `soa [T; N]` annotation chooses the
            // binding's backing-storage shape. Layout is not type
            // identity, so the checker has already accepted the pair
            // either way; this is the one place the flag matters.
            let soa = annotation.is_some_and(|a| a.is_soa());
            // The element annotation, when there is one: a *generic*
            // enum element (`[Option<i64>; 3]`) cannot be resolved
            // from the literal alone — `Option::Some(1i64)` names the
            // enum but not its instantiation.
            let element_annotation = match annotation {
                Some(TypeDecl::Array(elements, _, _)) => elements.first().cloned(),
                _ => None,
            };
            return self.lower_let_array_literal(name, elems, soa, element_annotation);
        }
        // Enum-construction RHS. `Enum::Variant` (unit) parses as a
        // `QualifiedIdentifier(vec![enum, variant])`; `Enum::Variant(args)`
        // parses as `AssociatedFunctionCall(enum, variant, args)`.
        // Either way the lowering allocates an `Enum` binding (tag local
        // + per-variant payload locals) and stores the chosen tag plus
        // the supplied arguments in this variant's payload slots.
        //
        // FROM-INTO-ENUM-ERR: `Enum::Variant(args)` and
        // `Enum::method(args)` parse to the same `AssociatedFunctionCall`
        // shape, so the tuple-variant dispatch only fires when the name
        // is a *declared variant*; anything else falls through to the
        // enum associated-function intercept below
        // (`MyErr::from(e)` from the `?` cross-error conversion).
        if let Expr::QualifiedIdentifier(path) = rhs.clone()
            && path.len() == 2 && self.enum_defs.contains_key(&path[0]) {
                return self.lower_let_enum_unit_variant(name, annotation, &path);
            }
        if let Expr::AssociatedFunctionCall(enum_name, variant_name, args) = rhs.clone()
            && self.enum_defs.contains_key(&enum_name)
            && self.enum_variant_index(&enum_name, &variant_name).is_some()
        {
                return self.lower_let_enum_tuple_variant(
                    name,
                    annotation,
                    enum_name,
                    variant_name,
                    args,
                );
            }
        // Composite enum-producing RHS: `if`-chain / `match` / block
        // whose every branch ends in an enum construction or an enum
        // binding identifier of the same enum. Pre-allocate the
        // shared target locals once and have each branch write into
        // them; cranelift's `def_var` walk turns the per-branch
        // writes into proper SSA at the merge.
        //
        // RUNTIME-IO: when detection cannot see through a branch, the
        // annotation takes over — `val e: IoError = match r {
        // Result::Err(e) => e, ... }` (the canonical "extract the
        // error value" shape) has an arm body that is a bare
        // pattern-bound identifier, and arm bindings are not in scope
        // at detection time. The checker has already unified every
        // arm against the annotation, so committing to the enum
        // composite path on its say-so is sound; at lowering time the
        // arm binding exists and the identifier arm becomes a plain
        // storage copy. Restricted to the composite shapes on purpose
        // — calls and literals have their own paths further down.
        let enum_source = match self.detect_enum_result(rhs_ref) {
            Some(crate::compound_storage::BranchShape::Produces(source)) => Some(source),
            // Detection saw nothing, or saw only diverging branches
            // (which bind nothing). Either way the annotation is the
            // remaining source, as before.
            _ => {
                if matches!(rhs, Expr::Match(..) | Expr::IfElifElse(..) | Expr::Block(..)) {
                    self.annotation_enum_base(annotation)
                        .map(crate::compound_storage::ShapeSource::Base)
                } else {
                    None
                }
            }
        };
        if let Some(source) = enum_source {
            return self.lower_let_enum_composite(name, annotation, rhs_ref, source);
        }
        // COMPOUND-BLOCK-RHS: the same for a struct- or tuple-producing
        // `if` chain, `match`, or block. Restricted to those three
        // shapes on purpose — a literal, a binding or a call produces
        // its value in one place and the paths below bind it without
        // an intermediate copy. Before this, every one of these was
        // `val/var rhs produced no value`, because each branch lowered
        // into its own locals and none of them was the binding's.
        if matches!(
            rhs,
            Expr::IfElifElse(..) | Expr::Match(..) | Expr::Block(..)
        ) {
            if let Some(crate::compound_storage::BranchShape::Produces(source)) =
                self.detect_struct_result(rhs_ref)
            {
                return self.lower_let_struct_composite(name, annotation, rhs_ref, source);
            }
            if let Some(crate::compound_storage::BranchShape::Produces(shape)) =
                self.detect_tuple_result(rhs_ref)
            {
                return self.lower_let_tuple_composite(name, rhs_ref, shape);
            }
        }
        // Struct-literal RHS: allocate one local per field (recursing
        // into nested struct fields), evaluate each field expression,
        // store into the matching local. The IR layer never sees a
        // struct value — we decompose at the lowering boundary.
        if let Expr::StructLiteral(struct_name, fields) = rhs {
            return self.lower_let_struct_literal(name, annotation, struct_name, fields);
        }
        // DICT-AOT-NEW: `var d: Dict<i64, u64> = Dict::new()` —
        // associated function call on a generic struct. The
        // type args come from the val annotation; the method
        // body is monomorphised through the same machinery
        // `lower_method_call` uses for `obj.m()` (Phase R3 +
        // X), with `self_type = Type::Struct(<resolved id>)`
        // and an empty arg list (the function takes no
        // receiver). Currently scoped to struct-returning
        // associated functions (e.g. `new() -> Self`); scalar
        // and other compound returns route through the regular
        // `Expr::AssociatedFunctionCall` handling below.
        if let Expr::AssociatedFunctionCall(struct_name, fn_name, ref args_vec) = rhs.clone()
            && self.struct_defs.contains_key(&struct_name)
                && let Some(result) = self.lower_let_struct_associated_call(
                    name,
                    annotation,
                    struct_name,
                    fn_name,
                    args_vec,
                )? {
                    return Ok(result);
                }
        // FROM-INTO-ENUM-ERR: enum-target associated function RHS
        // (`val e: MyErr = MyErr::from(s)` — exactly what the `?`
        // cross-error conversion emits). The variant construction
        // intercept above only fires for declared variant names now,
        // so reaching here with an enum qualifier means the name is an
        // associated function from an `impl ... for <Enum>` block.
        // Same registry lookup as the struct path; the return shaping
        // is shared with the plain compound-call path (`CallEnum`
        // covers `from`'s enum `Self` return).
        if let Expr::AssociatedFunctionCall(enum_name, fn_name, ref args_vec) = rhs.clone()
            && self.enum_defs.contains_key(&enum_name)
                && let Some(result) = self.lower_let_enum_associated_call(
                    name,
                    annotation,
                    enum_name,
                    fn_name,
                    args_vec,
                )? {
                    return Ok(result);
                }
        // OP-OVERLOAD-EXTEND Phase 4: unary operator overload at
        // let-rhs context (`val r: Vec3 = -a`). Compound `Self`
        // return needs `CallStruct` into a fresh binding, same
        // as the binary arith overload below — handle here so
        // `lower_unary`'s `Result<Option<ValueId>, _>` shape
        // doesn't have to deal with multi-leaf returns.
        if let Expr::Unary(unary_op, operand_ref) = rhs.clone()
            && let Some(result) =
                self.lower_let_unary_overload(name, unary_op, operand_ref)?
            {
                return Ok(result);
            }
        // Operator overload (Phase B continuation): arithmetic
        // overloads (`a + b` / `a - b` / `a * b` / `a / b` /
        // `a % b`) for matching struct values dispatch to the
        // user-defined `add` / `sub` / `mul` / `div` / `rem`
        // method. The frontend type checker has already vetted
        // shape compatibility; here we resolve the method's
        // FuncId, flatten both struct receivers' leaf locals
        // (mirrors `try_lower_struct_eq`'s shape) and emit
        // `CallStruct` into a fresh binding for the
        // compound `Self` return. The let-rhs path is the
        // primary user shape (`val c = a + b`) — chained
        // expression-position uses (`a + b + c`) are a future
        // extension that needs the call result to flow back as
        // an `Expr::Binary` ValueId.
        if let Expr::Binary(op, lhs_ref, rhs_ref) = rhs.clone()
            && let Some(result) =
                self.lower_let_binary_overload(name, op, lhs_ref, rhs_ref)?
            {
                return Ok(result);
            }
        // Primitive-receiver compound-returning method RHS:
        // `val s: String = lit.to_string()`. Mirrors the
        // struct/enum-receiver path below — but routes through
        // `primitive_target_sym_for_ir_type` (the same lookup
        // `lower_method_call`'s Step D extension-trait path uses)
        // because primitive bindings don't carry a struct_id we
        // could feed to `resolve_method_target`. Compound-returning
        // primitive methods (e.g. `str::to_string -> Vec<u8>`)
        // would otherwise fall through to `lower_method_call`'s
        // Step D path and bail at the compound-return guard.
        if let Expr::MethodCall(recv, method_sym, method_args) = rhs.clone()
            && let Some(result) = self.lower_let_primitive_method_compound(
                name,
                recv,
                method_sym,
                &method_args,
            )? {
                return Ok(result);
            }
        // Compound-returning method call RHS: `val q = p.swap()`.
        // Resolves the receiver / method target the same way
        // `lower_method_call` does, then routes the multi-result
        // through `CallStruct` / `CallTuple` / `CallEnum` into a
        // freshly-allocated binding. Mirrors the per-target
        // branches below for plain function calls.
        if let Expr::MethodCall(recv, method_sym, method_args) = rhs.clone()
            && let Some(result) = self.lower_let_struct_enum_method_compound(
                name,
                recv,
                method_sym,
                method_args,
            )? {
                return Ok(result);
            }
        // A5-P2-MVP-D/E: `val name = m.method()` where `m: &dyn Trait`
        // and the trait method returns a compound type (struct,
        // tuple, or enum). The dyn-trait dispatch emits the
        // matching `CallIndirectFn{Struct,Tuple,Enum}` and parks the
        // result in one of `pending_struct_value` /
        // `pending_tuple_value` / `pending_enum_value`; this helper
        // dispatches on whichever channel got filled and installs
        // the binding under `name`.
        if let Expr::MethodCall(recv, method_sym, method_args) = rhs.clone()
            && let Some(result) =
                self.lower_let_dyn_method_compound_return(name, recv, method_sym, method_args)?
        {
            return Ok(result);
        }
        // Tuple-returning call RHS: `val pair = make_pair()`. Same
        // shape as struct-returning calls, just routed through
        // CallTuple. Detect early so the parser-desugared
        // `val (a, b) = make_pair()` (which becomes
        // `val tmp = make_pair(); val a = tmp.0; val b = tmp.1`) is
        // also handled here without special-casing destructuring.
        if let Expr::Call(fn_name, args_ref) = rhs.clone()
            && let Some(result) =
                self.lower_let_call_tuple_or_enum(name, annotation, fn_name, &args_ref)?
            {
                return Ok(result);
            }
        // Struct-returning call RHS: `val p = make_point()`. Allocate
        // a struct binding and use `CallStruct` so codegen can route
        // the multi-return values into the per-field locals.
        if let Expr::Call(fn_name, args_ref) = rhs
            && let Some(result) =
                self.lower_let_call_struct(name, annotation, fn_name, &args_ref)?
            {
                return Ok(result);
            }
        // RUNTIME-IO: module-qualified compound-returning call RHS
        // (`val r = io::read_file(p)`). `Expr::AssociatedFunctionCall`
        // with a non-struct qualifier is a module call; the
        // expression-position path (`lower_expr_associated_call`)
        // rejects compound returns, and without this intercept the
        // let-rhs fell through to it even though the bare-name form
        // above works. Resolve through the module path and reuse the
        // exact same Call{Tuple,Enum,Struct} routing.
        if let Expr::AssociatedFunctionCall(qualifier, fn_name, ref args_vec) = rhs.clone()
            && !self.struct_defs.contains_key(&qualifier)
            && !self.enum_defs.contains_key(&qualifier)
            && let Some(target_id) = self.lookup_fn_here(
                Some(&self.written_qualifier_at(Some(rhs_ref), qualifier)),
                fn_name,
            )
            && let Some(result) =
                self.lower_let_call_compound_target(name, target_id, args_vec)?
            {
                return Ok(result);
            }
        // #121 Phase A: `val name: T = __builtin_ptr_read(p, off)` —
        // the legacy form, whose width comes from the annotation.
        // Without this intercept, lower_builtin_call's PtrRead arm
        // rejects the call with an error pointing at the typed form,
        // but a let-binding always supplies an annotation.
        //
        // AOT-COMPOUND-PTR-RW: when the annotation is a compound
        // (struct / tuple) the call expands into one `PtrRead` per
        // leaf scalar at `off + leaf_off`, then stores each leaf
        // into a freshly-allocated `Binding::Struct` /
        // `Binding::Tuple` local. Mirrors the per-leaf write loop
        // in `expr.rs::PtrWrite`. Pre-existing scalar callers
        // continue through the original single-PtrRead path.
        if let Expr::BuiltinCall(frontend::ast::BuiltinFunction::PtrRead, args) = rhs.clone()
            && args.len() == 2
                && let Some(result) =
                    self.lower_let_builtin_ptr_read(name, annotation, &args, false)?
                {
                    return Ok(result);
                }
        // MEMORY-ACCESS M1: the same read with the width written at
        // the call. The type argument replaces the annotation as the
        // element type -- including for a compound `T`, which is why
        // it routes here rather than through `lower_builtin_call`
        // (the per-leaf expansion needs the destination binding).
        // ELEMENT-BORROW E1: `__builtin_ptr_ref::<T>` rides the same
        // path — a borrow lowers exactly like the read it borrows
        // from, and the binding that catches it differs only in
        // carrying no drop glue.
        if let Expr::BuiltinCall(
            frontend::ast::BuiltinFunction::PtrReadTyped(ty)
            | frontend::ast::BuiltinFunction::PtrRefTyped(ty),
            args,
        ) = rhs.clone()
            && args.len() == 2
                && let Some(result) =
                    self.lower_let_builtin_ptr_read(name, Some(&ty), &args, false)?
                {
                    return Ok(result);
                }
        // DATA-ORIENTED Phase 1: `val ms = ps.mass` — the column
        // window. A field name on an *array* is every element's copy
        // of that field, handed back as a `Column<T>`
        // (`core/std/column.t`). Compiler-side because a stack
        // array's storage has no source-level name to point at.
        if let Expr::FieldAccess(base, field) = rhs.clone()
            && let Some(Expr::Identifier(source_sym)) = self.program.expression.get(&base)
        {
            match self.bindings.get(&source_sym).cloned() {
                Some(Binding::Array { element_ty, length, storage }) => {
                    if let Some(result) =
                        self.lower_let_column_window(name, element_ty, length, &storage, field)?
                    {
                        return Ok(result);
                    }
                }
                // The heap form: the same window over one column of a
                // `SoaVec<T>`'s buffer.
                Some(Binding::Struct { struct_id, fields }) => {
                    if let Some(result) =
                        self.lower_let_soa_vec_column(name, struct_id, &fields, field)?
                    {
                        return Ok(result);
                    }
                }
                _ => {}
            }
        }
        // DATA-ORIENTED Phase 2: `val name: T = __builtin_soa_read(p, i, cap)`
        // is the same read against a column-split buffer — same
        // annotation convention, same per-leaf expansion, different
        // arithmetic for where each leaf lives (`soa.rs`).
        if let Expr::BuiltinCall(frontend::ast::BuiltinFunction::SoaRead, args) = rhs.clone()
            && args.len() == 3
                && let Some(result) =
                    self.lower_let_builtin_ptr_read(name, annotation, &args, true)?
                {
                    return Ok(result);
                }
        // Phase 6b/6c: a Call RHS whose callee returns a function
        // type (`TypeDecl::Function`) lands as a fn-pointer value
        // (Type::U64 in IR). Bind under `Binding::FunctionPtr` so
        // a subsequent `name(args)` dispatches through the
        // env-based CallIndirect path. Without this branch the
        // binding would be a plain `Binding::Scalar { ty: U64 }`
        // and `lower_call` would then fail to find `name` in the
        // function table.
        if let Expr::Call(callee_name, _) = rhs.clone()
            && let Some(result) =
                self.lower_let_call_function_pointer(name, rhs_ref, callee_name)?
            {
                return Ok(result);
            }
        // Compound-typed RHS that already lives in leaf locals: a
        // field / element access (`val inner: Inner = o.i`,
        // `val t = o.pair`) or another compound binding
        // (`val q: Inner = p`). The new name adopts the same locals.
        // Scalar leaves fall through to the scalar path below, which
        // loads the local as a value.
        if matches!(
            rhs,
            Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _) | Expr::Identifier(_)
        ) && let Some(result) = self.lower_let_compound_access(name, rhs_ref)?
        {
            return Ok(result);
        }
        // Scalar fallback (existing behaviour).
        self.lower_let_scalar_fallback(name, annotation, rhs_ref)
    }

    /// Bind a name to a struct- / tuple-typed field, tuple element,
    /// or another compound binding (`val inner = o.i`, `val q = p`).
    /// Returns `Ok(None)` when the rhs is not compound, so the caller
    /// falls through to the scalar path.
    ///
    /// The binding **adopts the same leaf locals** rather than
    /// copying them, so the new name and the source are one value:
    /// writing `inner.a` is writing `o.i.a`. That matches the
    /// interpreter, where compound values share the existing
    /// reference (see "Captures" in `docs/language.md` — the rule is
    /// stated for closures but describes every binding), and it emits
    /// no instructions at all.
    fn lower_let_compound_access(
        &mut self,
        name: DefaultSymbol,
        rhs_ref: &ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        // A bare identifier is looked up directly: `resolve_field_chain`
        // rejects tuple roots (a tuple can't be *stepped into* by
        // field name), but a tuple binding is a perfectly good rhs.
        if let Some(Expr::Identifier(sym)) = self.program.expression.get(rhs_ref) {
            return Ok(match self.bindings.get(&sym).cloned() {
                Some(Binding::Struct { struct_id, fields }) => {
                    self.bindings
                        .insert(name, Binding::Struct { struct_id, fields });
                    Some(None)
                }
                Some(Binding::Tuple { elements }) => {
                    self.bindings.insert(name, Binding::Tuple { elements });
                    Some(None)
                }
                _ => None,
            });
        }
        // DATA-ORIENTED: a field / tuple chain rooted at an array
        // element (`val b = ps[i].y`) names a *scalar leaf*, not a
        // compound — `resolve_field_chain` would reject the
        // SliceAccess root outright. Fall through to the scalar
        // path, whose FieldAccess arm lowers it as one leaf load.
        if self.resolve_array_element_leaf(rhs_ref)?.is_some() {
            return Ok(None);
        }
        match self.resolve_field_chain(rhs_ref)? {
            FieldChainResult::Struct { struct_id, fields } => {
                self.bindings
                    .insert(name, Binding::Struct { struct_id, fields });
                Ok(Some(None))
            }
            FieldChainResult::Tuple { elements } => {
                self.bindings.insert(name, Binding::Tuple { elements });
                Ok(Some(None))
            }
            // JIT-enum-1: `val c = p.color`. Aliased, not copied —
            // the same rule the struct / tuple arms above follow, so
            // the binding names the field's own tag and payload
            // locals rather than a duplicate set.
            FieldChainResult::Enum(storage) => {
                self.bindings.insert(name, Binding::Enum(storage));
                Ok(Some(None))
            }
            // Scalar field — the existing `LoadLocal` path handles it.
            FieldChainResult::Scalar { .. } => Ok(None),
        }
    }

    /// Scalar fallback for `lower_let`: any RHS shape not picked up
    /// by the earlier early-return arms lands here. Lowers the
    /// expression to a single value, infers its scalar type, and
    /// stores it into a freshly-allocated `Binding::Scalar` local.
    fn lower_let_scalar_fallback(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // STDLIB-TRAIT-BASE B5: the annotation names the type argument
        // when nothing else can (`val a: u64 = make()`). The compound
        // paths above set the same hint; this is the scalar half.
        // Through the *active* substitution: inside a monomorphised
        // body an annotation can name a type parameter (`val c: T =
        // ...`), and an empty map cannot lower that.
        let hint = annotation.and_then(|a| self.lower_type_with_active_subst(a));
        // The hint stays set across `value_scalar` too: it asks the
        // same question a second time to size the binding, and would
        // otherwise fail after the instance had already been created.
        let saved = std::mem::replace(&mut self.pending_return_hint, hint);
        let lowered = self.lower_expr(rhs_ref);
        let scalar = lowered
            .as_ref()
            .ok()
            .and_then(|_| self.value_scalar(rhs_ref));
        self.pending_return_hint = saved;
        let v = lowered?.ok_or_else(|| "val/var rhs produced no value".to_string())?;
        // The annotation is the binding's declared type, so it is a
        // better answer than a failure when the rhs cannot say what it
        // produced -- a generic call whose type argument only the
        // annotation names, for instance.
        let scalar = scalar
            .or(hint)
            .ok_or_else(|| "could not infer scalar type for val/var rhs".to_string())?;
        let local = self.module.function_mut(self.func_id).add_local(scalar);
        self.bindings
            .insert(name, Binding::Scalar { local, ty: scalar });
        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
        Ok(None)
    }

    /// `__builtin_ptr_read` RHS helper. Serves both forms: the
    /// legacy `val name: T = __builtin_ptr_read(p, off)`, whose width
    /// is the annotation's, and `__builtin_ptr_read::<T>(p, off)`,
    /// which passes its written type in as `annotation`.
    /// For a compound `T` (struct /
    /// tuple) the call expands into one `PtrRead` per leaf
    /// scalar at `off + leaf_off`, mirroring the per-leaf write
    /// loop in `expr.rs::PtrWrite`. Scalar callers continue
    /// through the original single-PtrRead path. Returns
    /// `Ok(Some(_))` if it dispatched, `Ok(None)` if the
    /// annotation didn't resolve to a usable element type.
    ///
    /// With `soa` set this is `__builtin_soa_read(p, index, cap)`
    /// instead (DATA-ORIENTED Phase 2): same annotation convention and
    /// the same per-leaf expansion, but each leaf is addressed inside
    /// its own column rather than inside the element. Only the offset
    /// differs, so the two share every other line — see
    /// `soa::BufferAddress`.
    fn lower_let_builtin_ptr_read(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        args: &[ExprRef],
        soa: bool,
    ) -> Result<Option<Option<ValueId>>, String> {
        let who = if soa { "__builtin_soa_read" } else { "__builtin_ptr_read" };
        // Resolve the annotation type with the active
        // monomorphisation substitution applied. Without
        // the subst, an annotation that names a generic
        // param (e.g. `K` in
        // `core/std/dict.t::insert`'s
        // `val existing: K = __builtin_ptr_read::<K>(...)`)
        // wouldn't be reachable through `lower_scalar`,
        // which only knows the leaf-primitive `TypeDecl`s.
        let elem_ty = annotation.and_then(|a| self.lower_scalar_with_subst(a));
        // A user-named type (`val n: Node = ...`) is not a scalar and
        // is not in the substitution either, so the step above misses
        // it and the read used to fall through to the "needs a type
        // annotation" error — with the annotation right there. Resolve
        // the name to its monomorphised instance instead; this is the
        // same path type arguments already take.
        let elem_ty = match elem_ty {
            Some(t) => Some(t),
            None => annotation.and_then(|a| self.lower_type_arg(a)),
        };
        if let Some(elem_ty) = elem_ty {
            if matches!(elem_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                let columns = self.soa_columns(elem_ty).ok_or_else(|| {
                    format!(
                        "{who}: unable to compute leaf layout for `{}`",
                        crate::spelling::spell_type(self.module, self.interner, elem_ty)
                    )
                })?;
                let ptr = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| format!("{who}: base produced no value"))?;
                let address = self.lower_buffer_address(args, soa)?;
                // Allocate the destination binding's leaf
                // locals up front so we can store each
                // PtrRead value straight into them in
                // declaration order.
                // DROP-GLUE: no drop registration here — a
                // `ptr_read` copy is an alias of the slot it
                // read; the slot's owner frees it when it dies.
                let (leaf_locals, binding) = match elem_ty {
                    Type::Struct(struct_id) => {
                        let fields = self.allocate_struct_fields(struct_id);
                        let locals = flatten_struct_locals(&fields);
                        (locals, Binding::Struct { struct_id, fields })
                    }
                    Type::Tuple(tuple_id) => {
                        let elements = self.allocate_tuple_elements(tuple_id)?;
                        let locals = flatten_tuple_element_locals(&elements);
                        (locals, Binding::Tuple { elements })
                    }
                    // PTR-READ-ENUM: a tag local plus one slot per
                    // (variant, payload) — the destination shape that
                    // `collect_leaves` walks the buffer in. Reading
                    // fills the inactive variants' slots with whatever
                    // the buffer holds there; the tag is what any
                    // subsequent `match` dispatches on.
                    Type::Enum(enum_id) => {
                        let storage = self.allocate_enum_storage(enum_id);
                        let locals = flatten_enum_storage_locals(&storage);
                        (locals, Binding::Enum(storage))
                    }
                    _ => unreachable!("guarded above"),
                };
                if leaf_locals.len() != columns.len() {
                    return Err(format!(
                        "{who}: leaf count mismatch ({} locals vs {} layout entries) for `{}`",
                        leaf_locals.len(),
                        columns.len(),
                        crate::spelling::spell_type(self.module, self.interner, elem_ty)
                    ));
                }
                for (column, (local, _local_ty)) in columns.iter().zip(leaf_locals.iter()) {
                    let leaf_ty = column.2;
                    let off_v = self.emit_leaf_offset(&address, column);
                    let v = self
                        .emit(
                            InstKind::PtrRead { ptr, offset: off_v, elem_ty: leaf_ty },
                            Some(leaf_ty),
                        )
                        .expect("PtrRead returns a value");
                    self.emit(
                        InstKind::StoreLocal { dst: *local, src: v },
                        None,
                    );
                }
                self.bindings.insert(name, binding);
                return Ok(Some(None));
            }
            let ptr = self
                .lower_expr(&args[0])?
                .ok_or_else(|| format!("{who}: base produced no value"))?;
            // A scalar element is one column with `prefix = 0`, so the
            // SoA address collapses to `index * sizeof(T)` — the same
            // byte the interleaved layout would use. Nothing to split.
            let address = self.lower_buffer_address(args, soa)?;
            let column = (0u64, self.compute_byte_size(elem_ty).unwrap_or(0), elem_ty);
            let offset = self.emit_leaf_offset(&address, &column);
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
            return Ok(Some(None));
        }
        Ok(None)
    }

    /// Function-pointer-returning call RHS helper
    /// (Phase 6b/6c). A Call RHS whose callee returns a
    /// function type (`TypeDecl::Function`) lands as a
    /// fn-pointer value (Type::U64 in IR). Binds under
    /// `Binding::FunctionPtr` so a subsequent `name(args)`
    /// dispatches through the env-based CallIndirect path.
    /// Returns `Ok(Some(_))` if it dispatched, `Ok(None)` if
    /// the callee doesn't return a function type and the
    /// caller should fall through to the scalar fallback.
    fn lower_let_call_function_pointer(
        &mut self,
        name: DefaultSymbol,
        rhs_ref: &ExprRef,
        callee_name: DefaultSymbol,
    ) -> Result<Option<Option<ValueId>>, String> {
        if let Some(callee_id) = self.lookup_fn_here(None, callee_name)
            && let Some(callee_fn) = self
                .program
                .function
                .iter()
                .find(|f| f.name == callee_name)
                && let Some(frontend::type_decl::TypeDecl::Function(p_tys, r_ty)) =
                    callee_fn.return_type.as_ref()
                {
                    let mut ir_param_tys: Vec<Type> = Vec::with_capacity(p_tys.len());
                    let mut ok = true;
                    for pt in p_tys {
                        match super::types::lower_scalar(pt) {
                            Some(t) => ir_param_tys.push(t),
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if let (true, Some(ir_ret_ty)) = (ok, super::types::lower_scalar(r_ty)) {
                        let _ = callee_id; // resolved at the existing scalar fall-through
                        let v = self
                            .lower_expr(rhs_ref)?
                            .ok_or_else(|| "val/var rhs produced no value".to_string())?;
                        let local = self
                            .module
                            .function_mut(self.func_id)
                            .add_local(Type::U64);
                        self.bindings.insert(
                            name,
                            Binding::FunctionPtr {
                                local,
                                param_tys: ir_param_tys,
                                ret_ty: ir_ret_ty,
                            },
                        );
                        self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
                        return Ok(Some(None));
                    }
                }
        Ok(None)
    }

    /// STDLIB-TRAIT-BASE B5: rewrite `T::assoc(args)` to
    /// `Concrete::assoc(args)` using the active monomorphisation.
    ///
    /// Returns the expression unchanged for everything else, which is
    /// every associated call outside a generic body.
    pub(super) fn substitute_assoc_qualifier(&self, expr: Expr) -> Expr {
        let Expr::AssociatedFunctionCall(qualifier, fn_name, args) = expr else {
            return expr;
        };
        match self.concrete_type_param_name(qualifier) {
            Some(concrete) => Expr::AssociatedFunctionCall(concrete, fn_name, args),
            None => Expr::AssociatedFunctionCall(qualifier, fn_name, args),
        }
    }

    /// The declared type a type parameter stands for in the body being
    /// lowered, as the symbol the struct / enum registries are keyed
    /// on. `None` when the symbol is not a type parameter, or stands
    /// for something with no name of its own (a scalar).
    pub(super) fn concrete_type_param_name(&self, sym: DefaultSymbol) -> Option<DefaultSymbol> {
        match self.active_subst.get(&sym).copied()? {
            Type::Struct(id) => Some(self.module.struct_def(id).base_name),
            Type::Enum(id) => Some(self.module.enum_def(id).base_name),
            // A primitive names its impls under its canonical
            // spelling, read from NUM-W-ENUMERATION's single list
            // (`impl Default for u64` registers under `"u64"`).
            scalar => TypeDecl::PRIMITIVE_IMPL_TARGETS
                .iter()
                .find(|(decl, _)| crate::types::lower_scalar(decl) == Some(scalar))
                .and_then(|(_, name)| self.interner.get(name)),
        }
    }


    /// STDLIB-TRAIT-BASE B5: lower an annotation with the body's own
    /// monomorphisation applied, so `val c: T = ...` inside a generic
    /// body resolves to whatever `T` is here.
    fn lower_type_with_active_subst(&mut self, ty: &TypeDecl) -> Option<Type> {
        if let Some(t) = self.lower_scalar_with_subst(ty) {
            return Some(t);
        }
        let subst: std::collections::HashMap<DefaultSymbol, Type> = self.active_subst.clone();
        self.lower_type_with_subst(ty, &subst)
    }

    /// Tuple- or enum-returning call RHS helper. Allocates the
    /// matching binding shape and emits a `CallTuple` /
    /// `CallEnum` so codegen can route multi-return slots
    /// into the per-leaf locals. Returns `Ok(Some(_))` if the
    /// callee returns Tuple / Enum, `Ok(None)` if not (caller
    /// should fall through to the struct-returning variant).
    fn lower_let_call_tuple_or_enum(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        if let Some(target_id) =
            self.lookup_or_instantiate_call_target(fn_name, annotation, args_ref)
        {
            let items: Vec<ExprRef> = self.call_arg_items(args_ref)?;
            return self.lower_let_call_compound_target(name, target_id, &items);
        }
        Ok(None)
    }

    /// The callee a `val x = f(...)` names, including a **generic
    /// template** that has not been instantiated yet.
    ///
    /// The compound-return intercepts looked the name up with
    /// `module.lookup_function`, which only knows names that already
    /// exist -- and a generic function exists only per instantiation.
    /// So `val q: P = dup(&p)` on `fn dup<T: Clone>(v: &T) -> T` fell
    /// past every compound path to the plain expression one, which
    /// refuses a struct-returning call and told the user to bind it
    /// with `val` -- which is what they had written
    /// (STDLIB-TRAIT-BASE B1).
    fn lookup_or_instantiate_call_target(
        &mut self,
        fn_name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        args_ref: &ExprRef,
    ) -> Option<crate::ir::FuncId> {
        if let Some(id) = self.lookup_fn_here(None, fn_name) {
            return Some(id);
        }
        if !self.generic_funcs.contains_key(&fn_name) {
            return None;
        }
        // STDLIB-TRAIT-BASE B5: the annotation is what names `T` when
        // the arguments cannot (`val p: P = make()`). Handed down as a
        // hint rather than a parameter because every intermediate call
        // in the resolution chain would otherwise have to carry it.
        // Through the *active* substitution: inside a monomorphised
        // body an annotation can name a type parameter (`val c: T =
        // ...`), and an empty map cannot lower that.
        let hint = annotation.and_then(|a| self.lower_type_with_active_subst(a));
        let saved = std::mem::replace(&mut self.pending_return_hint, hint);
        let resolved = self.resolve_call_target(fn_name, args_ref).ok();
        self.pending_return_hint = saved;
        resolved
    }

    /// Shared tail of the compound-returning call intercepts (bare
    /// calls and RUNTIME-IO's module-qualified calls alike): given a
    /// resolved callee whose return type is a tuple / enum / struct,
    /// allocate the matching binding shape and emit the matching
    /// `Call*` so codegen routes the multi-return values into the
    /// binding's leaf locals. Returns `Ok(None)` when the return type
    /// is scalar, so the caller falls through to the next intercept.
    fn lower_let_call_compound_target(
        &mut self,
        name: DefaultSymbol,
        target_id: crate::ir::FuncId,
        args_items: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        let target_ret = self.module.function(target_id).return_type;
        if let Type::Tuple(tuple_id) = target_ret {
            let element_bindings = self.allocate_tuple_elements(tuple_id)?;
            let mut dests: Vec<LocalId> = flatten_tuple_element_locals(&element_bindings)
                .into_iter()
                .map(|(local, _)| local)
                .collect();
            // REF-Stage-2 (ii-let-rhs): if the callee declares
            // writeback returns from compound `&mut T` params,
            // append those dests so the caller-side bindings
            // receive the modified leaves alongside the
            // tuple result.
            if !self.module.function(target_id).self_writeback_types.is_empty() {
                dests.extend(self.collect_compound_writeback_dests_for(args_items, Some(target_id), 0)?);
            }
            self.bindings.insert(
                name,
                Binding::Tuple { elements: element_bindings },
            );
            // CODE-SIZE-SELF-ABI: the callee has to be named here, or
            // a wide `&mut T` parameter is expanded leaf by leaf into
            // a signature that takes one address. Every other call
            // site passes it; these three (the `val x = f(..)` shapes
            // whose return is compound) passed `None`, which is why
            // `verify_call_arity` was the thing that noticed.
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_items, Some(target_id))?;
            self.emit(
                InstKind::CallTuple {
                    target: target_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(Some(None));
        }
        if let Type::Enum(enum_id) = target_ret {
            // Enum-returning call: pre-allocate the binding's
            // storage tree, flatten it into the CallEnum dest
            // list (tag first, then each variant's payloads
            // in declaration order, recursing through nested
            // enum slots). Codegen then routes the multi-
            // return slots straight into our locals.
            let storage = self.allocate_enum_storage(enum_id);
            let mut dests = Self::flatten_enum_dests(&storage);
            // REF-Stage-2 (ii-let-rhs): see Tuple branch.
            if !self.module.function(target_id).self_writeback_types.is_empty() {
                dests.extend(self.collect_compound_writeback_dests_for(args_items, Some(target_id), 0)?);
            }
            self.bindings
                .insert(name, Binding::Enum(storage));
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_items, Some(target_id))?;
            self.emit(
                InstKind::CallEnum {
                    target: target_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(Some(None));
        }
        if let Type::Struct(struct_id) = target_ret {
            let field_bindings = self.allocate_struct_fields(struct_id);
            // CallStruct dests are the leaf scalar locals in
            // declaration order — exactly what the cranelift
            // multi-result call gives us back.
            let mut dests: Vec<LocalId> = flatten_struct_locals(&field_bindings)
                .into_iter()
                .map(|(l, _)| l)
                .collect();
            // REF-Stage-2 (ii-let-rhs): if the callee declares
            // writeback returns from compound `&mut T` params,
            // append those dests after the struct fields so
            // the caller-side `&mut <var>` bindings absorb
            // the modified leaves in the canonical order
            // (`flatten_compound_leaf_types` ensures both
            // sides use the same shape).
            if !self.module.function(target_id).self_writeback_types.is_empty() {
                dests.extend(self.collect_compound_writeback_dests_for(args_items, Some(target_id), 0)?);
            }
            self.register_drop_for_struct_binding(struct_id, &field_bindings);
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
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_items, Some(target_id))?;
            self.emit(
                InstKind::CallStruct {
                    target: target_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(Some(None));
        }
        Ok(None)
    }

    /// Struct-returning call RHS helper. Allocates a struct
    /// binding and uses `CallStruct` so codegen can route the
    /// multi-return values into the per-field locals.
    fn lower_let_call_struct(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        if let Some(target_id) =
            self.lookup_or_instantiate_call_target(fn_name, annotation, args_ref)
        {
            let items: Vec<ExprRef> = self.call_arg_items(args_ref)?;
            return self.lower_let_call_compound_target(name, target_id, &items);
        }
        Ok(None)
    }

    /// Extract a call's argument list as plain items. The plain-call
    /// RHS shapes carry their args as an `Expr::ExprList` in the
    /// expression pool; the module-qualified shape carries a
    /// `Vec<ExprRef>` directly. Both funnel into
    /// `lower_call_arg_items`.
    fn call_arg_items(&self, args_ref: &ExprRef) -> Result<Vec<ExprRef>, String> {
        match self.program.expression.get(args_ref) {
            Some(Expr::ExprList(items)) => Ok(items.clone()),
            _ => Err("call args missing".to_string()),
        }
    }

    /// The enum a `val` annotation names, when any (`val e: IoError =
    /// match ...`). The parser emits `Struct(Name<...>)` for any
    /// `Name<...>` spelling and `Identifier(Name)` for bare names, so
    /// all three spellings are checked against the enum table.
    pub(super) fn annotation_enum_base(
        &self,
        annotation: Option<&TypeDecl>,
    ) -> Option<DefaultSymbol> {
        let ty = annotation?;
        let sym = match ty {
            TypeDecl::Enum(sym, _) | TypeDecl::Struct(sym, _) | TypeDecl::Identifier(sym) => *sym,
            _ => return None,
        };
        if self.enum_defs.contains_key(&sym) {
            Some(sym)
        } else {
            None
        }
    }

    /// The index of `variant_name` in the enum template's declared
    /// variants, when it is one. `Enum::Variant(args)` and
    /// `Enum::method(args)` parse to the same `AssociatedFunctionCall`
    /// shape, so the let-rhs dispatch needs this to tell construction
    /// from an associated function call (FROM-INTO-ENUM-ERR).
    pub(super) fn enum_variant_index(
        &self,
        enum_name: &DefaultSymbol,
        variant_name: &DefaultSymbol,
    ) -> Option<usize> {
        let template = self.enum_defs.get(enum_name)?;
        template.variants.iter().position(|v| v.name == *variant_name)
    }

    /// Enum associated-function-call RHS helper (`val e: MyErr =
    /// MyErr::from(s)`, FROM-INTO-ENUM-ERR). Mirrors
    /// `lower_let_struct_associated_call`: resolves the enum instance
    /// from the annotation, looks the function up in the same method
    /// registry (generic impls of generic enums instantiate through
    /// the same template machinery), and shapes the return through
    /// the shared `lower_let_call_compound_target` — `from` returns
    /// the enum itself, which is exactly the `CallEnum` case. Scalar
    /// returns get a regular `Call` + scalar binding, mirroring the
    /// struct helper. Returns `Ok(None)` when the registry has no
    /// such function, so the caller falls through (and the eventual
    /// reject names the expression).
    fn lower_let_enum_associated_call(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        enum_name: DefaultSymbol,
        fn_name: DefaultSymbol,
        args_vec: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        let enum_id = self.resolve_enum_instance(enum_name, annotation)?;
        let recv_type_args = self.module.enum_def(enum_id).type_args.clone();
        let func_id = match super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            enum_name,
            fn_name,
            &recv_type_args,
        ) {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => id,
            Some(super::method_registry::ResolvedMethodTarget::Template(template)) => {
                self.instantiate_generic_method_with_self_type(
                    enum_name,
                    fn_name,
                    &template,
                    Type::Enum(enum_id),
                    recv_type_args.clone(),
                    args_vec,
                )?
            }
            None => return Ok(None),
        };
        if let Some(result) = self.lower_let_call_compound_target(name, func_id, args_vec)? {
            return Ok(Some(result));
        }
        // Scalar return — emit a regular Call.
        let target_ret = self.module.function(func_id).return_type;
        if target_ret.produces_value() {
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_vec, Some(func_id))?;
            let v = self
                .emit(
                    InstKind::Call { target: func_id, args: arg_values },
                    Some(target_ret),
                )
                .expect("Call returns a value");
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            let local = self
                .module
                .function_mut(self.func_id)
                .add_local(target_ret);
            self.bindings.insert(
                name,
                Binding::Scalar { local, ty: target_ret },
            );
            self.emit(InstKind::StoreLocal { dst: local, src: v }, None);
            return Ok(Some(None));
        }
        Ok(None)
    }

    /// Struct/enum-receiver compound-returning method RHS helper
    /// (`val q = p.swap()`). Resolves the receiver / method
    /// target via `resolve_method_target` then routes the
    /// multi-result through `CallStruct` / `CallTuple` /
    /// `CallEnum` into a freshly-allocated binding. Returns
    /// `Ok(Some(_))` if it dispatched, `Ok(None)` if the
    /// receiver / method doesn't match this shape and the
    /// caller should fall through.
    /// A5-P2-MVP-D/E: `val name = m.method()` where `m: &dyn Trait`
    /// and the trait method returns a compound type. Calls into
    /// `lower_method_call` once — the dyn-trait dispatch path
    /// picks the matching `CallIndirectFn{Struct,Tuple,Enum}` based
    /// on the trait method's return type and parks the result in
    /// one of `pending_struct_value` / `pending_tuple_value` /
    /// `pending_enum_value`. This helper then adopts whichever
    /// channel got filled under `name`, so a single dispatch
    /// covers all three compound-return shapes without re-emitting
    /// IR. Returns `Ok(Some(_))` when it dispatched, `Ok(None)`
    /// when the receiver isn't a `Binding::DynTraitObj` so the
    /// caller can keep looking for the right arm.
    fn lower_let_dyn_method_compound_return(
        &mut self,
        name: DefaultSymbol,
        recv: ExprRef,
        method_sym: DefaultSymbol,
        method_args: Vec<ExprRef>,
    ) -> Result<Option<Option<ValueId>>, String> {
        // Cheap precondition check — only proceed if the receiver
        // resolves to a `Binding::DynTraitObj`. Anything else
        // belongs to a different lower path.
        let recv_expr = self
            .program
            .expression
            .get(&recv)
            .ok_or_else(|| "dyn method-call receiver missing".to_string())?;
        let recv_sym = match recv_expr {
            Expr::Identifier(s) => s,
            _ => return Ok(None),
        };
        if !matches!(self.bindings.get(&recv_sym), Some(Binding::DynTraitObj { .. })) {
            return Ok(None);
        }
        // Snapshot pending-value channels so we can tell which one
        // the dispatched method newly fills.
        let had_struct = self.pending_struct_value.is_some();
        let had_tuple = self.pending_tuple_value.is_some();
        let had_enum = self.pending_enum_value.is_some();
        // Delegate to the regular method-call lowering. For a
        // `Binding::DynTraitObj` receiver it routes through
        // `lower_dyn_method_call`, which sets the matching pending
        // channel based on the trait method's return type.
        self.lower_method_call(&recv, method_sym, &method_args)?;
        if !had_struct
            && let Some(fields) = self.pending_struct_value.take() {
                let outer_struct_id =
                    self.recover_last_dyn_struct_return_id().ok_or_else(|| {
                        "A5-P2-MVP-D: could not recover struct id for dyn method's struct return"
                            .to_string()
                    })?;
                self.bindings.insert(
                    name,
                    Binding::Struct {
                        struct_id: outer_struct_id,
                        fields,
                    },
                );
                return Ok(Some(None));
            }
        if !had_tuple
            && let Some(elements) = self.pending_tuple_value.take() {
                self.bindings
                    .insert(name, Binding::Tuple { elements });
                return Ok(Some(None));
            }
        if !had_enum
            && let Some(storage) = self.pending_enum_value.take() {
                self.bindings.insert(name, Binding::Enum(storage));
                return Ok(Some(None));
            }
        // The trait method returned a scalar / Unit; no compound
        // pending channel was set by our call. Fall back to the
        // regular scalar path by signalling "didn't handle".
        Ok(None)
    }

    /// Helper for `lower_let_dyn_method_compound_return`: walk back
    /// from the current block's tail to find the most recent
    /// `CallIndirectFnStruct` and return its `ret_struct_id`.
    fn recover_last_dyn_struct_return_id(
        &self,
    ) -> Option<crate::ir::StructId> {
        let func = self.module.function(self.func_id);
        let block_id = self.current_block?;
        let blk = func.blocks.iter().find(|b| b.id == block_id)?;
        for inst in blk.instructions.iter().rev() {
            if let InstKind::CallIndirectFnStruct { ret_struct_id, .. } = &inst.kind {
                return Some(*ret_struct_id);
            }
        }
        None
    }

    fn lower_let_struct_enum_method_compound(
        &mut self,
        name: DefaultSymbol,
        recv: ExprRef,
        method_sym: DefaultSymbol,
        method_args: Vec<ExprRef>,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(call) =
            self.prepare_compound_method_call(&recv, method_sym, &method_args)?
        else {
            return Ok(None);
        };
        let reload = call.reload;
        // Allocate the binding the result lands in, then emit the call
        // with its leaf locals as destinations (writeback slots last).
        match call.ret {
            Type::Struct(struct_id) => {
                let fields = self.allocate_struct_fields(struct_id);
                let mut dests: Vec<LocalId> = flatten_struct_locals(&fields)
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect();
                dests.extend(call.writeback_dests.iter().copied());
                self.register_drop_for_struct_binding(struct_id, &fields);
                self.bindings
                    .insert(name, Binding::Struct { struct_id, fields });
                self.emit(
                    InstKind::CallStruct {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
            }
            Type::Tuple(tuple_id) => {
                let elements = self.allocate_tuple_elements(tuple_id)?;
                let mut dests: Vec<LocalId> = flatten_tuple_element_locals(&elements)
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect();
                dests.extend(call.writeback_dests.iter().copied());
                self.bindings.insert(name, Binding::Tuple { elements });
                self.emit(
                    InstKind::CallTuple {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
            }
            Type::Enum(enum_id) => {
                let storage = self.allocate_enum_storage(enum_id);
                let mut dests = Self::flatten_enum_dests(&storage);
                dests.extend(call.writeback_dests.iter().copied());
                self.bindings.insert(name, Binding::Enum(storage));
                self.emit(
                    InstKind::CallEnum {
                        target: call.target,
                        args: call.args,
                        dests,
                    },
                    None,
                );
            }
            _ => unreachable!("prepare_compound_method_call guards the return shape"),
        }
        // CODE-SIZE-SELF-ABI: the call is emitted here rather than in
        // `prepare_compound_method_call`, so the receiver slot it
        // materialised is read back here too.
        reload.apply(self);
        Ok(Some(None))
    }

    /// Primitive-receiver compound-returning method RHS helper
    /// (`val s: String = lit.to_string()`). Routes through
    /// `primitive_target_sym_for_ir_type` because primitive
    /// bindings don't carry a struct_id we could feed to
    /// `resolve_method_target`. Returns `Ok(Some(_))` if it
    /// dispatched, `Ok(None)` if the receiver / method doesn't
    /// match this shape and the caller should fall through.
    fn lower_let_primitive_method_compound(
        &mut self,
        name: DefaultSymbol,
        recv: ExprRef,
        method_sym: DefaultSymbol,
        method_args: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        if let Some(recv_ty) = self.value_scalar(&recv)
            && let Some(target_sym) =
                super::method_call::primitive_target_sym_for_ir_type(recv_ty, self.interner)
                && let Some(func_id) = super::method_registry::lookup_method_func(
                    self.method_func_ids, target_sym, method_sym, &[],
                ) {
                    let target_ret = self.module.function(func_id).return_type;
                    if matches!(
                        target_ret,
                        Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)
                    ) {
                        let recv_value = self
                            .lower_expr(&recv)?
                            .ok_or_else(|| {
                                "primitive method receiver produced no value".to_string()
                            })?;
                        let mut all_args: Vec<ValueId> = vec![recv_value];
                        for (arg_idx, a) in method_args.iter().enumerate() {
                            // `T` -> `&T` auto-borrow. The receiver
                            // always occupies slot 0, so this
                            // argument's entry is `1 + arg_idx`.
                            // Without it `250u8.checked_add(x)` for
                            // `fn checked_add(&self, other: &Self)`
                            // passed the value where the callee reads
                            // a pointer, and the addition silently
                            // used 0.
                            if let Some(ptr) = self.lower_scalar_ref_arg(
                                a,
                                self.module
                                    .function(func_id)
                                    .param_ref_pointee
                                    .get(1 + arg_idx)
                                    .copied()
                                    .flatten(),
                            )? {
                                all_args.push(ptr);
                                continue;
                            }
                            let arg_expr_ref = match self.program.expression.get(a) {
                                Some(Expr::Unary(frontend::ast::UnaryOp::Borrow | frontend::ast::UnaryOp::BorrowMut, inner)) => {
                                    inner
                                }
                                _ => *a,
                            };
                            if let Some(Expr::Identifier(sym)) =
                                self.program.expression.get(&arg_expr_ref)
                            {
                                if let Some(Binding::Struct { fields, .. }) =
                                    self.bindings.get(&sym).cloned()
                                {
                                    for (local, ty) in flatten_struct_locals(&fields) {
                                        let v = self
                                            .emit(InstKind::LoadLocal(local), Some(ty))
                                            .expect("LoadLocal returns a value");
                                        all_args.push(v);
                                    }
                                    continue;
                                }
                                if let Some(Binding::Tuple { elements }) =
                                    self.bindings.get(&sym).cloned()
                                {
                                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                                        let v = self
                                            .emit(InstKind::LoadLocal(local), Some(ty))
                                            .expect("LoadLocal returns a value");
                                        all_args.push(v);
                                    }
                                    continue;
                                }
                                if let Some(Binding::Enum(storage)) =
                                    self.bindings.get(&sym).cloned()
                                {
                                    let vs = self.load_enum_locals(&storage);
                                    all_args.extend(vs);
                                    continue;
                                }
                            }
                            let v = self
                                .lower_expr(&arg_expr_ref)?
                                .ok_or_else(|| {
                                    "method argument produced no value".to_string()
                                })?;
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
                                self.register_drop_for_struct_binding(struct_id, &fields);
                                self.bindings.insert(
                                    name,
                                    Binding::Struct { struct_id, fields },
                                );
                                self.emit(
                                    InstKind::CallStruct {
                                        target: func_id,
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
                                        target: func_id,
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
                                        target: func_id,
                                        args: all_args,
                                        dests,
                                    },
                                    None,
                                );
                            }
                            _ => unreachable!("guard ensured compound return"),
                        }
                        return Ok(Some(None));
                    }
                }
        Ok(None)
    }

    /// Binary arithmetic / bitwise operator overload RHS helper
    /// (`val c = a + b`). Delegates to
    /// [`Self::emit_binary_overload`] and binds its result.
    fn lower_let_binary_overload(
        &mut self,
        name: DefaultSymbol,
        op: frontend::ast::Operator,
        lhs_ref: ExprRef,
        rhs_ref: ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some((struct_id, fields)) = self.emit_binary_overload(op, lhs_ref, rhs_ref)? else {
            return Ok(None);
        };
        self.register_drop_for_struct_binding(struct_id, &fields);
        self.bindings
            .insert(name, Binding::Struct { struct_id, fields });
        Ok(Some(None))
    }

    /// Resolve a struct's operator-overload method, evaluate both
    /// operands, and emit the call into fresh leaf locals.
    ///
    /// Returns the destination struct instance and its field bindings,
    /// or `Ok(None)` when the operator or the left operand's type does
    /// not name an overload and the caller should fall through.
    ///
    /// The result lives in locals rather than in an SSA value because
    /// a struct always does. Which is the whole of OP-OVERLOAD-CHAIN:
    /// the only position that used to allocate those locals was a
    /// `val` RHS, so `a + b` had nowhere to go anywhere else.
    pub(super) fn emit_binary_overload(
        &mut self,
        op: frontend::ast::Operator,
        lhs_ref: ExprRef,
        rhs_ref: ExprRef,
    ) -> Result<Option<(crate::ir::StructId, Vec<super::bindings::FieldBinding>)>, String> {
        let op_method: Option<&'static str> = match op {
            frontend::ast::Operator::IAdd => Some("add"),
            frontend::ast::Operator::ISub => Some("sub"),
            frontend::ast::Operator::IMul => Some("mul"),
            frontend::ast::Operator::IDiv => Some("div"),
            frontend::ast::Operator::IMod => Some("rem"),
            frontend::ast::Operator::BitwiseAnd => Some("bitand"),
            frontend::ast::Operator::BitwiseOr => Some("bitor"),
            frontend::ast::Operator::BitwiseXor => Some("bitxor"),
            frontend::ast::Operator::LeftShift => Some("shl"),
            frontend::ast::Operator::RightShift => Some("shr"),
            _ => None,
        };
        let Some(method_name) = op_method else {
            return Ok(None);
        };
        let Some(Type::Struct(struct_id)) = self.value_scalar(&lhs_ref) else {
            return Ok(None);
        };
        let target_sym = self.module.struct_def(struct_id).base_name;
        let Some(method_sym) = self.interner.get(method_name) else {
            return Ok(None);
        };
        let Some(func_id) =
            self.resolve_struct_method_func_id(target_sym, method_sym, struct_id, &[rhs_ref])?
        else {
            return Ok(None);
        };
        // OP-OVERLOAD-CHAIN: both operands go through the ordinary
        // compound-argument path, which is what the method call they
        // become would use anyway. It takes a binding, a literal, a
        // compound-returning call — and, since that path knows about
        // overloads too, another overloaded operator, which is what
        // makes `a + b + c` work.
        //
        // Before this it required a bare identifier on each side, so a
        // chain and a literal operand were both
        // `must be a bare identifier (MVP)`.
        // CODE-SIZE-SELF-ABI: `a + b` is `add(&a, &b)` -- both slots
        // can be pointer-passed, so each operand is lowered against
        // the one it fills.
        let (mut all_args, lhs_reload) = self.lower_arg_values_for(&lhs_ref, Some(func_id), 0)?;
        let (rhs_args, rhs_reload) = self.lower_arg_values_for(&rhs_ref, Some(func_id), 1)?;
        all_args.extend(rhs_args);
        let target_ret = self.module.function(func_id).return_type;
        let Type::Struct(dest_struct_id) = target_ret else {
            return Err(format!(
                "operator overload: {} method must return Self (got `{}`)",
                method_name,
                crate::spelling::spell_type(self.module, self.interner, target_ret)
            ));
        };
        let fields = self.allocate_struct_fields(dest_struct_id);
        let dests: Vec<LocalId> = flatten_struct_locals(&fields)
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        self.emit(
            InstKind::CallStruct {
                target: func_id,
                args: all_args,
                dests,
            },
            None,
        );
        lhs_reload.apply(self);
        rhs_reload.apply(self);
        Ok(Some((dest_struct_id, fields)))
    }

    /// Unary operator overload RHS helper (`val r: Vec3 = -a`).
    /// Delegates to [`Self::emit_unary_overload`] and binds its result.
    fn lower_let_unary_overload(
        &mut self,
        name: DefaultSymbol,
        unary_op: frontend::ast::UnaryOp,
        operand_ref: ExprRef,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some((struct_id, fields)) = self.emit_unary_overload(unary_op, operand_ref)? else {
            return Ok(None);
        };
        self.register_drop_for_struct_binding(struct_id, &fields);
        self.bindings
            .insert(name, Binding::Struct { struct_id, fields });
        Ok(Some(None))
    }

    /// The unary twin of [`Self::emit_binary_overload`]: resolve
    /// `neg` / `bitnot` / `not` on the operand's struct, evaluate the
    /// operand through the compound-argument path, and emit the call
    /// into fresh leaf locals.
    pub(super) fn emit_unary_overload(
        &mut self,
        unary_op: frontend::ast::UnaryOp,
        operand_ref: ExprRef,
    ) -> Result<Option<(crate::ir::StructId, Vec<super::bindings::FieldBinding>)>, String> {
        let unary_method: Option<&'static str> = match unary_op {
            frontend::ast::UnaryOp::Negate => Some("neg"),
            frontend::ast::UnaryOp::BitwiseNot => Some("bitnot"),
            frontend::ast::UnaryOp::LogicalNot => Some("not"),
            _ => None,
        };
        let Some(method_name) = unary_method else {
            return Ok(None);
        };
        let Some(Type::Struct(struct_id)) = self.value_scalar(&operand_ref) else {
            return Ok(None);
        };
        let target_sym = self.module.struct_def(struct_id).base_name;
        let Some(method_sym) = self.interner.get(method_name) else {
            return Ok(None);
        };
        let Some(func_id) =
            self.resolve_struct_method_func_id(target_sym, method_sym, struct_id, &[])?
        else {
            return Ok(None);
        };
        // OP-OVERLOAD-CHAIN: see `emit_binary_overload` — the operand
        // takes the ordinary compound-argument path, so `-(a + b)` and
        // `-V { .. }` reach the same place a bare identifier did.
        let (all_args, operand_reload) =
            self.lower_arg_values_for(&operand_ref, Some(func_id), 0)?;
        let target_ret = self.module.function(func_id).return_type;
        let Type::Struct(dest_struct_id) = target_ret else {
            return Err(format!(
                "unary overload: {} method must return Self (got `{}`)",
                method_name,
                crate::spelling::spell_type(self.module, self.interner, target_ret)
            ));
        };
        let fields = self.allocate_struct_fields(dest_struct_id);
        let dests: Vec<LocalId> = flatten_struct_locals(&fields)
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        self.emit(
            InstKind::CallStruct {
                target: func_id,
                args: all_args,
                dests,
            },
            None,
        );
        operand_reload.apply(self);
        Ok(Some((dest_struct_id, fields)))
    }

    /// Struct associated-function-call RHS helper
    /// (`var d: Dict<i64, u64> = Dict::new()`). Looks up the
    /// associated function in the method registry (non-generic
    /// `method_func_ids` first, then `generic_methods` for
    /// templates) and dispatches a `CallStruct` for compound
    /// `Self` returns or a regular `Call` for scalar returns.
    /// Returns `Ok(Some(_))` if it handled the binding,
    /// `Ok(None)` if the caller should fall through to the
    /// generic associated-function path below.
    fn lower_let_struct_associated_call(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        struct_name: DefaultSymbol,
        fn_name: DefaultSymbol,
        args_vec: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        let struct_id = self.resolve_struct_instance(struct_name, annotation)?;
        let recv_type_args = self
            .module
            .struct_def(struct_id)
            .type_args
            .clone();
        // Look up the associated function across both registries.
        // Generic templates live in `generic_methods`; non-generic
        // ones in `method_func_ids`. The Dict::new path always
        // hits the generic registry because dict.t's
        // `impl<K, V> Dict<K, V>` carries (K, V) onto
        // every method. Non-generic impls of generic structs
        // (`impl Vec<u8>` in `core/std/collections/vec.t`) land
        // in `method_func_ids`. CONCRETE-IMPL Phase 2c: the
        // unified dispatch resolves a lone concrete spec only after
        // the generic template — the concrete impl wins for
        // receivers it exactly matches, the template for the rest.
        let func_id = match super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            struct_name,
            fn_name,
            &recv_type_args,
        ) {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => id,
            Some(super::method_registry::ResolvedMethodTarget::Template(template)) => {
                self.instantiate_generic_method_with_self_type(
                    struct_name,
                    fn_name,
                    &template,
                    Type::Struct(struct_id),
                    recv_type_args.clone(),
                    args_vec,
                )?
            }
            None => return Ok(None),
        };
        let target_ret = self.module.function(func_id).return_type;
        if let Type::Struct(ret_struct_id) = target_ret {
            let field_bindings = self.allocate_struct_fields(ret_struct_id);
            let dests: Vec<LocalId> = flatten_struct_locals(&field_bindings)
                .into_iter()
                .map(|(l, _)| l)
                .collect();
            self.register_drop_for_struct_binding(ret_struct_id, &field_bindings);
            self.bindings.insert(
                name,
                Binding::Struct {
                    struct_id: ret_struct_id,
                    fields: field_bindings,
                },
            );
            // ASSOC-FN-REF-ARG: ask the callee. Lowering the
            // arguments one at a time with no target meant nothing
            // knew that a `&Vec<String>` parameter wants an address,
            // and an explicit `&v` argument produced no value at all
            // (`String::join(&parts, &sep)` did not compile). The
            // free-function path has always passed the target; these
            // three did not.
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_vec, Some(func_id))?;
            self.emit(
                InstKind::CallStruct {
                    target: func_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(Some(None));
        }
        // Enum return — `fn try_from_raw(p: ptr) -> Option<Self>` and
        // friends. Same shape as the plain compound-call path's enum
        // branch: pre-allocate the binding's storage tree, flatten it
        // into the `CallEnum` dest list, bind as an enum so a later
        // `match` sees an enum scrutinee. Without this arm the
        // associated call fell through to a path that bound the name
        // as the *receiver* struct, and the `match` failed with
        // "enum-variant pattern is only valid against an enum
        // scrutinee" (GENERIC-IN-ENUM-PAYLOAD).
        if let Type::Enum(ret_enum_id) = target_ret {
            let storage = self.allocate_enum_storage(ret_enum_id);
            let dests = Self::flatten_enum_dests(&storage);
            self.bindings.insert(name, Binding::Enum(storage));
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_vec, Some(func_id))?;
            self.emit(
                InstKind::CallEnum {
                    target: func_id,
                    args: arg_values,
                    dests,
                },
                None,
            );
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            return Ok(Some(None));
        }
        // Scalar return — emit a regular Call.
        if target_ret.produces_value() {
            let (arg_values, ptr_arg_reloads) =
                self.lower_call_arg_items(args_vec, Some(func_id))?;
            let v = self
                .emit(
                    InstKind::Call { target: func_id, args: arg_values },
                    Some(target_ret),
                )
                .expect("Call returns a value");
            for r in ptr_arg_reloads {
                r.apply(self);
            }
            let local = self
                .module
                .function_mut(self.func_id)
                .add_local(target_ret);
            self.bindings.insert(
                name,
                Binding::Scalar { local, ty: target_ret },
            );
            self.emit(
                InstKind::StoreLocal { dst: local, src: v },
                None,
            );
            return Ok(Some(None));
        }
        Ok(None)
    }

    /// Struct-literal RHS helper (`val p = Point { x: 1, y: 2 }`).
    /// Allocates one local per field (recursing into nested
    /// struct fields), evaluates each field expression, stores
    /// into the matching local.
    fn lower_let_struct_literal(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        struct_name: DefaultSymbol,
        fields: Vec<(DefaultSymbol, ExprRef)>,
    ) -> Result<Option<ValueId>, String> {
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
        self.register_drop_for_struct_binding(struct_id, &field_bindings);
        self.bindings.insert(
            name,
            Binding::Struct {
                struct_id,
                fields: field_bindings.clone(),
            },
        );
        self.store_struct_literal_fields(struct_id, &field_bindings, &fields)?;
        Ok(None)
    }

    /// Enum unit-variant RHS helper (`val c = Color::Red`).
    /// Allocates an `Enum` binding and stores the chosen tag.
    fn lower_let_enum_unit_variant(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        path: &[DefaultSymbol],
    ) -> Result<Option<ValueId>, String> {
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
        Ok(None)
    }

    /// Enum tuple-variant RHS helper (`val s = Shape::Circle(5i64)`).
    /// Allocates an `Enum` binding and stores tag + payload.
    fn lower_let_enum_tuple_variant(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        enum_name: DefaultSymbol,
        variant_name: DefaultSymbol,
        args: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
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
        Ok(None)
    }

    /// Composite enum-producing RHS helper: `if`-chain / `match` /
    /// block whose every branch ends in an enum construction or an
    /// enum binding identifier of the same enum. Pre-allocates the
    /// shared target locals and dispatches each branch through
    /// `lower_into_enum_storage`.
    fn lower_let_enum_composite(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
        source: crate::compound_storage::ShapeSource<EnumId>,
    ) -> Result<Option<ValueId>, String> {
        use crate::compound_storage::ShapeSource;
        // Annotation first, detected instance as the fallback — the
        // same order `lower_let_struct_composite` uses, and for the
        // same reason: an `Option<i64>` read out of an enum payload
        // has no annotation to instantiate from.
        let enum_id = match source {
            ShapeSource::Base(base_name) => self.resolve_enum_instance(base_name, annotation)?,
            ShapeSource::Instance(id) => {
                let base_name = self.module.enum_def(id).base_name;
                self.resolve_enum_instance(base_name, annotation)
                    .unwrap_or(id)
            }
        };
        let storage = self.allocate_enum_storage(enum_id);
        self.bindings
            .insert(name, Binding::Enum(storage.clone()));
        self.lower_into_enum_storage(rhs_ref, &storage)?;
        Ok(None)
    }

    /// COMPOUND-BLOCK-RHS: struct counterpart of
    /// `lower_let_enum_composite`. Pre-allocates the binding's field
    /// locals and threads every branch into them, so the branches
    /// converge on the locals the name is bound to rather than each
    /// writing its own set.
    fn lower_let_struct_composite(
        &mut self,
        name: DefaultSymbol,
        annotation: Option<&TypeDecl>,
        rhs_ref: &ExprRef,
        source: crate::compound_storage::ShapeSource<StructId>,
    ) -> Result<Option<ValueId>, String> {
        use crate::compound_storage::ShapeSource;
        // The annotation stays the first source: it is what the
        // programmer wrote, and it is how every generic struct got
        // instantiated here before. A detected instance is the answer
        // for the case that used to have none — a `Vec<u64>` read out
        // of an enum payload with no annotation to instantiate from.
        let struct_id = match source {
            ShapeSource::Base(base_name) => self.resolve_struct_instance(base_name, annotation)?,
            ShapeSource::Instance(id) => {
                let base_name = self.module.struct_def(id).base_name;
                self.resolve_struct_instance(base_name, annotation)
                    .unwrap_or(id)
            }
        };
        let fields = self.allocate_struct_fields(struct_id);
        // The binding owns the value whichever branch built it, so it
        // gets the same auto-drop registration a literal rhs would —
        // see `lower_let_struct_literal`.
        self.register_drop_for_struct_binding(struct_id, &fields);
        self.bindings.insert(
            name,
            Binding::Struct {
                struct_id,
                fields: fields.clone(),
            },
        );
        self.lower_into_struct_fields(rhs_ref, struct_id, &fields)?;
        Ok(None)
    }

    /// Tuple counterpart of `lower_let_struct_composite`. The element
    /// shapes come from whichever branch detection could read them
    /// off; every branch then writes those same locals.
    fn lower_let_tuple_composite(
        &mut self,
        name: DefaultSymbol,
        rhs_ref: &ExprRef,
        shape: crate::compound_storage::TupleShapeSource,
    ) -> Result<Option<ValueId>, String> {
        use crate::compound_storage::TupleShapeSource;
        let elements = match shape {
            TupleShapeSource::Binding(elements) => elements,
            TupleShapeSource::Interned(tuple_id) => self.allocate_tuple_elements(tuple_id)?,
            TupleShapeSource::Literal(elems) => {
                let mut out: Vec<TupleElementBinding> = Vec::with_capacity(elems.len());
                for (i, elem_ref) in elems.iter().enumerate() {
                    let elem_ty = self.infer_tuple_element_type(elem_ref).ok_or_else(|| {
                        format!("compiler MVP could not infer type for tuple element #{i}")
                    })?;
                    let shape = self.allocate_tuple_element_shape(elem_ty)?;
                    out.push(TupleElementBinding { index: i, shape });
                }
                out
            }
        };
        self.register_drop_for_tuple_binding(&elements);
        self.bindings.insert(
            name,
            Binding::Tuple {
                elements: elements.clone(),
            },
        );
        self.lower_into_tuple_elements(rhs_ref, &elements)?;
        Ok(None)
    }

    /// Array-literal RHS helper (`val arr = [a, b, c]`). Allocates
    /// the backing storage (interleaved, or one slot per leaf for a
    /// `soa` annotation — `allocate_array_storage`) and stores each
    /// element through `store_array_element`.
    fn lower_let_array_literal(
        &mut self,
        name: DefaultSymbol,
        elems: Vec<ExprRef>,
        soa: bool,
        element_annotation: Option<TypeDecl>,
    ) -> Result<Option<ValueId>, String> {
        if elems.is_empty() {
            return Err(
                "compiler MVP cannot infer element type for empty array literal".to_string(),
            );
        }
        // Element type comes from the first element. Scalars
        // resolve via `value_scalar`; struct literals resolve
        // through the struct table.
        let elem_ty = self.infer_array_element_type(&elems[0], element_annotation.as_ref())?;
        if !matches!(
            elem_ty,
            Type::I64
                | Type::U64
                | Type::F64
                // SIMD-F32: `f32` was missing while
                // `elem_stride_bytes` already gave it a native
                // 4-byte stride — so `[f32; 3]` was refused by the
                // one list that had not been told about the type.
                | Type::F32
                | Type::Bool
                | Type::I8
                | Type::U8
                | Type::I16
                | Type::U16
                | Type::I32
                | Type::U32
                | Type::Struct(_)
                | Type::Tuple(_)
                // DATA-ORIENTED Phase 3: an enum element is the tag
                // plus every variant's payload, flattened the way
                // `collect_leaves` already flattens one.
                | Type::Enum(_)
        ) {
            return Err(format!(
                "compiler MVP only supports scalar / struct / tuple / enum array elements; \
                 got `{}`",
                crate::spelling::spell_type(self.module, self.interner, elem_ty)
            ));
        }
        let leaf_count = leaf_scalar_count(self.module, elem_ty);
        let storage = self.allocate_array_storage(elem_ty, elems.len(), soa);
        for (i, e) in elems.iter().enumerate() {
            self.store_array_element(&storage, elem_ty, i, leaf_count, e)?;
        }
        self.bindings.insert(
            name,
            Binding::Array {
                element_ty: elem_ty,
                length: elems.len(),
                storage,
            },
        );
        Ok(None)
    }

    /// Range-slice RHS helper (`val sub = arr[start..end]`).
    /// Constant bounds only — both endpoints must fold via
    /// `try_constant_index`. Allocates a fresh fixed-length
    /// array binding and copies each leaf scalar with an
    /// `ArrayLoad` + `ArrayStore` pair. DATA-ORIENTED: the
    /// destination layout is the annotation's when it names one,
    /// the source's otherwise — the per-leaf copy goes through
    /// leaf slots either way, so a re-layouting slice
    /// (`val sub: [P; 2] = soa_ps[1..3]`) is the same
    /// materialise-and-restore path any cross-layout copy is.
    fn lower_let_range_slice(
        &mut self,
        name: DefaultSymbol,
        arr_obj: ExprRef,
        info: frontend::ast::SliceInfo,
        dst_soa: Option<bool>,
    ) -> Result<Option<ValueId>, String> {
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
        let (element_ty, length, src_storage) = match self.bindings.get(&arr_sym).cloned() {
            Some(Binding::Array { element_ty, length, storage }) => {
                (element_ty, length, storage)
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
        let soa = dst_soa.unwrap_or(match &src_storage {
            ArrayStorage::Columns(_) => true,
            ArrayStorage::Interleaved(_) => false,
        });
        let dst_storage = self.allocate_array_storage(element_ty, new_len, soa);
        let src_units = match &src_storage {
            ArrayStorage::Interleaved(slot) => self.interleaved_units(*slot),
            ArrayStorage::Columns(_) => (1, Vec::new()),
        };
        let dst_units = match &dst_storage {
            ArrayStorage::Interleaved(slot) => self.interleaved_units(*slot),
            ArrayStorage::Columns(_) => (1, Vec::new()),
        };
        for i in 0..new_len {
            for j in 0..leaf_count {
                let leaf_ty = leaf_type_at(self.module, element_ty, j);
                // Source leaf position under the source layout, then
                // the destination position under the destination
                // layout — the pair is what makes a re-layouting
                // slice the same code as a preserving one.
                let (src_slot, src_idx) = match &src_storage {
                    ArrayStorage::Interleaved(slot) => {
                        (*slot, (start + i) * src_units.0 as usize + src_units.1[j] as usize)
                    }
                    ArrayStorage::Columns(cols) => (cols[j], start + i),
                };
                let (dst_slot, dst_idx) = match &dst_storage {
                    ArrayStorage::Interleaved(slot) => {
                        (*slot, i * dst_units.0 as usize + dst_units.1[j] as usize)
                    }
                    ArrayStorage::Columns(cols) => (cols[j], i),
                };
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
                storage: dst_storage,
            },
        );
        Ok(None)
    }

    /// Compound-element single-index slice RHS helper
    /// (`val p: Point = arr[i]`). Returns `Ok(Some(_))` if it
    /// handled the binding, `Ok(None)` if the caller should
    /// fall through to the next branch (e.g. scalar element
    /// type, or the slice doesn't resolve to an array binding).
    fn lower_let_slice_single_element(
        &mut self,
        name: DefaultSymbol,
        arr_obj: ExprRef,
        info: frontend::ast::SliceInfo,
    ) -> Result<Option<Option<ValueId>>, String> {
        let arr_expr = self
            .program
            .expression
            .get(&arr_obj)
            .ok_or_else(|| "array-access object missing".to_string())?;
        if let Expr::Identifier(arr_sym) = arr_expr
            && let Some(Binding::Array { element_ty, .. }) =
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
                            self.register_drop_for_struct_binding(struct_id, &fields);
                            self.bindings.insert(
                                name,
                                Binding::Struct { struct_id, fields },
                            );
                            return Ok(Some(None));
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
                            return Ok(Some(None));
                        }
                    }
                    // DATA-ORIENTED Phase 3: `val s: Shape = ss[i]`
                    // arrives with its tag and payload slots already
                    // filled, and binds like any other enum value —
                    // so `match s { ... }` is the ordinary path from
                    // here on.
                    Type::Enum(_) => {
                        self.pending_enum_value = None;
                        let _ = self.lower_slice_access(&arr_obj, &info)?;
                        if let Some(storage) = self.pending_enum_value.take() {
                            self.bindings.insert(name, Binding::Enum(storage));
                            return Ok(Some(None));
                        }
                    }
                    _ => {}
                }
            }
        Ok(None)
    }

    /// Tuple-literal RHS helper. Allocates one local per element
    /// and stores each element value through
    /// `store_value_into_tuple_element_shape`.
    /// RANGE-FOR: a range value is its two bounds, each in a local of
    /// the element type. Both are evaluated before the binding is
    /// visible, so `val r = r.start..n` over an older `r` reads the
    /// old one.
    fn lower_let_range(
        &mut self,
        name: DefaultSymbol,
        start: &ExprRef,
        end: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        let ty = self
            .value_scalar(start)
            .or_else(|| self.value_scalar(end))
            .ok_or_else(|| "compiler MVP could not infer the element type of a range".to_string())?;
        let s = self
            .lower_expr(start)?
            .ok_or_else(|| "range start produced no value".to_string())?;
        let e = self
            .lower_expr(end)?
            .ok_or_else(|| "range end produced no value".to_string())?;
        let func = self.module.function_mut(self.func_id);
        let (start_local, end_local) = (func.add_local(ty), func.add_local(ty));
        self.emit(InstKind::StoreLocal { dst: start_local, src: s }, None);
        self.emit(InstKind::StoreLocal { dst: end_local, src: e }, None);
        self.bindings.insert(
            name,
            Binding::Range { start: start_local, end: end_local, ty },
        );
        Ok(None)
    }

    fn lower_let_tuple_literal(
        &mut self,
        name: DefaultSymbol,
        elems: Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
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
        Ok(None)
    }
}
