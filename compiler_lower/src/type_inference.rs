//! Structural type inference used by the lowering pass.
//!
//! `value_scalar` is a cheap, conservative inference that picks the
//! IR `Type` of an expression *without* materialising any IR
//! instructions. It runs on `&self` and exists primarily so val/var
//! sites can name the right local slot when the user omits the type
//! annotation. The full type-checker has already validated the
//! program; this pass just needs enough local information to
//! disambiguate (e.g.) signed-vs-unsigned division at codegen time.
//!
//! Lives in its own `impl` block on `super::FunctionLower<'a>` —
//! Rust permits multiple `impl`s of the same type across files of
//! the same module tree, which lets us split this big struct's
//! methods by topic without touching its public API.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, Operator, Stmt, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::bindings::{
    Binding, FieldBinding, FieldChainResult, FieldShape, TupleElementBinding,
    TupleElementShape,
};
use super::types::lower_scalar;
use super::FunctionLower;
use crate::ir::{Type, TupleId};

impl<'a> FunctionLower<'a> {
    /// Declared type of an arm body that is exactly one of the names
    /// its own pattern binds (`Enum::V(x) => x`).
    ///
    /// The enum is identified from the **scrutinee**, not from the
    /// pattern's enum name: generic enums are interned per
    /// instantiation, so `Option<i64>` and `Option<str>` share a base
    /// name but not their payload types. Picking by name would return
    /// the wrong one.
    ///
    /// `None` for any shape this cannot resolve purely — a method-call
    /// scrutinee needs `&mut self` to resolve its target, so those keep
    /// the previous behaviour rather than guessing.
    fn arm_payload_binding_type(
        &self,
        scrutinee: &ExprRef,
        arm: &frontend::ast::MatchArm,
    ) -> Option<Type> {
        use frontend::ast::Pattern;
        let Expr::Identifier(body_sym) = self.program.expression.get(&arm.body)? else {
            return None;
        };
        let Pattern::EnumVariant(_, variant_sym, sub_patterns) = &arm.pattern else {
            return None;
        };
        let slot = sub_patterns
            .iter()
            .position(|p| matches!(p, Pattern::Name(s) if *s == body_sym))?;
        let enum_id = self.scrutinee_enum_id(scrutinee)?;
        let variant = self
            .module
            .enum_def(enum_id)
            .variants
            .iter()
            .find(|v| v.name == *variant_sym)?;
        variant.payload_types.get(slot).copied()
    }

    /// The same peek for a name bound by a **struct or tuple**
    /// pattern (`match p { Painted { color: _, size } => size }`).
    /// PATTERN-COMPOUND-LOWER taught the lowering to bind those names
    /// but left the inference behind, so a `val` over such a match was
    /// rejected with "could not infer scalar type for val/var rhs"
    /// even though the arm bodies lower fine.
    ///
    /// The scrutinee's own binding already carries a shape per field /
    /// element, so the type is read off that — no binding is
    /// introduced, which keeps this `&self` like the rest of the
    /// inference.
    fn arm_compound_binding_type(
        &self,
        scrutinee: &ExprRef,
        arm: &frontend::ast::MatchArm,
    ) -> Option<Type> {
        use frontend::ast::Pattern;
        let Expr::Identifier(body_sym) = self.program.expression.get(&arm.body)? else {
            return None;
        };
        // `size` and `size @ 0i64` both name the field.
        let names_body = |p: &Pattern| match p {
            Pattern::Name(s) => *s == body_sym,
            Pattern::Binding(s, _) => *s == body_sym,
            _ => false,
        };
        match &arm.pattern {
            Pattern::Struct(_, field_patterns, _) => {
                let fields = self.scrutinee_struct_fields(scrutinee)?;
                let (field_sym, _) =
                    field_patterns.iter().find(|(_, p)| names_body(p))?;
                let field_name = self.interner.resolve(*field_sym)?;
                let fb = fields.iter().find(|f| f.name == field_name)?;
                Self::field_shape_type(&fb.shape)
            }
            Pattern::Tuple(sub_patterns) => {
                let elements = self.scrutinee_tuple_elements(scrutinee)?;
                let idx = sub_patterns.iter().position(names_body)?;
                let el = elements.iter().find(|e| e.index == idx)?;
                match &el.shape {
                    TupleElementShape::Scalar { ty, .. } => Some(*ty),
                    TupleElementShape::Struct { struct_id, .. } => {
                        Some(Type::Struct(*struct_id))
                    }
                    TupleElementShape::Tuple { .. } => None,
                }
            }
            _ => None,
        }
    }

    /// Type a `FieldShape` names, for the inference peeks. `None` for
    /// a tuple field, which carries no id to name — the same answer
    /// the `FieldAccess` arm gives.
    fn field_shape_type(shape: &FieldShape) -> Option<Type> {
        match shape {
            FieldShape::Scalar { ty, .. } => Some(*ty),
            FieldShape::Struct { struct_id, .. } => Some(Type::Struct(*struct_id)),
            FieldShape::Enum(storage) => Some(Type::Enum(storage.enum_id)),
            FieldShape::Tuple { .. } => None,
        }
    }

    /// Field list of a struct-valued match scrutinee — a bare
    /// identifier or a field-access chain, the two shapes
    /// `classify_match_scrutinee` accepts without lowering anything.
    fn scrutinee_struct_fields(&self, scrutinee: &ExprRef) -> Option<Vec<FieldBinding>> {
        match self.program.expression.get(scrutinee)? {
            Expr::Identifier(sym) => match self.bindings.get(&sym)? {
                Binding::Struct { fields, .. } => Some(fields.clone()),
                _ => None,
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(scrutinee).ok()? {
                FieldChainResult::Struct { fields, .. } => Some(fields),
                _ => None,
            },
            _ => None,
        }
    }

    /// Tuple counterpart of `scrutinee_struct_fields`.
    fn scrutinee_tuple_elements(
        &self,
        scrutinee: &ExprRef,
    ) -> Option<Vec<TupleElementBinding>> {
        match self.program.expression.get(scrutinee)? {
            Expr::Identifier(sym) => match self.bindings.get(&sym)? {
                Binding::Tuple { elements } => Some(elements.clone()),
                _ => None,
            },
            Expr::FieldAccess(_, _) => match self.resolve_field_chain(scrutinee).ok()? {
                FieldChainResult::Tuple { elements } => Some(elements),
                _ => None,
            },
            _ => None,
        }
    }

    /// The interned enum a match scrutinee produces, when that can be
    /// determined without lowering anything.
    fn scrutinee_enum_id(&self, scrutinee: &ExprRef) -> Option<crate::ir::EnumId> {
        match self.program.expression.get(scrutinee)? {
            Expr::Identifier(sym) => match self.bindings.get(&sym)? {
                Binding::Enum(storage) => Some(storage.enum_id),
                _ => None,
            },
            // Direct table lookup, not `resolve_call_target`: that one
            // takes `&mut self` and would queue a monomorphisation from
            // what is supposed to be a peek.
            Expr::Call(fn_name, _) => {
                let target = self.module.lookup_function(None, fn_name)?;
                match self.module.function(target).return_type {
                    Type::Enum(id) => Some(id),
                    _ => None,
                }
            }
            // A method-call scrutinee resolves through the same
            // receiver-binding + method-registry peek `value_scalar`'s
            // `MethodCall` arm uses. It needs no `&mut self` and the
            // recursion is bounded: every step descends into a
            // subexpression of the scrutinee.
            Expr::MethodCall(..) => match self.value_scalar(scrutinee)? {
                Type::Enum(id) => Some(id),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn value_scalar(&self, expr_ref: &ExprRef) -> Option<Type> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::Int64(_) => Some(Type::I64),
            Expr::UInt64(_) => Some(Type::U64),
            // NUM-W-AOT: narrow integer literal types.
            Expr::Int8(_) => Some(Type::I8),
            Expr::UInt8(_) => Some(Type::U8),
            Expr::Int16(_) => Some(Type::I16),
            Expr::UInt16(_) => Some(Type::U16),
            Expr::Int32(_) => Some(Type::I32),
            Expr::UInt32(_) | Expr::CharLiteral(_) => Some(Type::U32),
            Expr::Float64(_) => Some(Type::F64),
            // SIMD-F32: single-precision literal.
            Expr::Float32(_) => Some(Type::F32),
            Expr::String(_) => Some(Type::Str),
            Expr::True | Expr::False => Some(Type::Bool),
            // #121 Phase B-min: a `with allocator = ... { body }`
            // expression takes its value from the body, so peek the
            // body for type inference. This lets `val x = with ... { e }`
            // bind to the right scalar type.
            Expr::With(_, body) => self.value_scalar(&body),
            Expr::Cast(_, target_ty) => lower_scalar(&target_ty),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                // Compound bindings — surface the IR type so callers
                // like `__builtin_sizeof(value)` (compute_byte_size
                // walks the def) can see the shape. The
                // identifier-arg flatten paths in `method_call.rs`
                // / `let_lowering.rs` skip this read entirely (they
                // pull leaf locals straight out of the binding), so
                // returning a Some here doesn't accidentally route
                // through the IR value graph.
                Some(Binding::Struct { struct_id, .. }) => Some(Type::Struct(*struct_id)),
                Some(Binding::Tuple { elements }) => {
                    // Tuple bindings only carry per-element shapes,
                    // not the interned tuple_id. Fall back to None
                    // so callers needing a concrete shape error out
                    // explicitly.
                    let _ = elements;
                    None
                }
                Some(Binding::Enum(storage)) => Some(Type::Enum(storage.enum_id)),
                // Closure / function-pointer values are U64-sized
                // pointers in the IR (the env address). This lets
                // `val g = f` infer the correct slot type when `f`
                // is a closure binding.
                Some(Binding::FunctionPtr { .. }) => Some(Type::U64),
                // A borrow reads as its pointee: `&mut n` and a
                // shared closure capture (CLOSURE-CAPTURE E3) both
                // hold a pointer in the IR, but every read of the
                // name yields the value behind it.
                Some(Binding::RefScalar { pointee_ty, .. }) => Some(*pointee_ty),
                Some(_) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            Expr::FieldAccess(obj, field) => {
                if let Some((_, ty)) = self.range_bound(&obj, field) {
                    return Some(ty);
                }
                // DATA-ORIENTED: a chain rooted at an array element
                // (`ps[i].y`) names a leaf scalar — the same
                // resolution the load lowering emits, so report the
                // leaf's type instead of failing on the SliceAccess
                // root `resolve_field_chain` cannot walk.
                if let Ok(Some(leaf)) = self.resolve_array_element_leaf(expr_ref) {
                    return Some(leaf.leaf_ty);
                }
                let inner = self.resolve_field_chain(&obj).ok()?;
                let fields = match inner {
                    FieldChainResult::Struct { fields, .. } => fields,
                    FieldChainResult::Scalar { .. }
                    | FieldChainResult::Tuple { .. }
                    | FieldChainResult::Enum(_) => return None,
                };
                let field_str = self.interner.resolve(field)?;
                fields
                    .iter()
                    .find(|f| f.name == field_str)
                    .and_then(|f| match &f.shape {
                        FieldShape::Scalar { ty, .. } => Some(*ty),
                        // Struct-typed field — surface the shape for
                        // the same reason the `Identifier` arm above
                        // does (`__builtin_sizeof`, the interpolation
                        // formatter). Tuple fields stay `None`: a
                        // `FieldShape::Tuple` carries no `tuple_id`
                        // to name, exactly as tuple bindings don't.
                        FieldShape::Struct { struct_id, .. } => Some(Type::Struct(*struct_id)),
                        FieldShape::Tuple { .. } => None,
                        // JIT-enum-1: same reasoning as the struct
                        // arm — the enum id names the type, which is
                        // what `__builtin_sizeof` and the formatter
                        // need.
                        FieldShape::Enum(storage) => Some(Type::Enum(storage.enum_id)),
                    })
            }
            Expr::TupleAccess(tuple, index) => {
                // DATA-ORIENTED: same as the FieldAccess arm above —
                // `ts[i].0` names a leaf of an array element.
                if let Ok(Some(leaf)) = self.resolve_array_element_leaf(expr_ref) {
                    return Some(leaf.leaf_ty);
                }
                let elements = self.resolve_tuple_chain_elements(&tuple).ok()?;
                elements
                    .iter()
                    .find(|e| e.index == index)
                    .map(|e| match &e.shape {
                        TupleElementShape::Scalar { ty, .. } => *ty,
                        TupleElementShape::Struct { struct_id, .. } => {
                            Type::Struct(*struct_id)
                        }
                        TupleElementShape::Tuple { tuple_id, .. } => {
                            Type::Tuple(*tuple_id)
                        }
                    })
            }
            Expr::TupleLiteral(elems) => {
                // We can't intern a fresh tuple shape here (this method
                // is `&self`), so fall back to looking up the existing
                // shape if it's already in the IR module's table.
                let mut element_tys: Vec<Type> = Vec::with_capacity(elems.len());
                for e in &elems {
                    element_tys.push(self.value_scalar(e)?);
                }
                self.module
                    .tuple_defs
                    .iter()
                    .position(|t| *t == element_tys)
                    .map(|i| Type::Tuple(TupleId(i as u32)))
            }
            Expr::Binary(op, lhs, _rhs) => match op {
                Operator::EQ
                | Operator::NE
                | Operator::LT
                | Operator::LE
                | Operator::GT
                | Operator::GE => match self.value_scalar(&lhs) {
                    // SIMD: a lane-wise comparison produces a mask,
                    // not one `bool` — the lane count is the number of
                    // answers.
                    Some(Type::Vector(v)) => Some(Type::Vector(v.mask())),
                    _ => Some(Type::Bool),
                },
                Operator::LogicalAnd | Operator::LogicalOr => Some(Type::Bool),
                _ => self.value_scalar(&lhs),
            },
            Expr::Unary(op, operand) => match op {
                UnaryOp::LogicalNot => Some(Type::Bool),
                _ => self.value_scalar(&operand),
            },
            Expr::Block(stmts) => {
                if let Some(last) = stmts.last()
                    && let Some(Stmt::Expression(e)) = self.program.statement.get(last) {
                        return self.value_scalar(&e);
                    }
                None
            }
            Expr::IfElifElse(_, then_body, _, _) => self.value_scalar(&then_body),
            Expr::Match(scrutinee, arms) => {
                // An arm body that stands on its own: a literal, a call,
                // an expression over bindings from an enclosing scope.
                if let Some(ty) = arms.iter().find_map(|a| self.value_scalar(&a.body)) {
                    return Some(ty);
                }
                // Otherwise every body depends on what its *own* pattern
                // binds — `match e { A(v) => v, B(e) => e }`. This method
                // is `&self`, so it cannot introduce the binding and
                // recurse the way `arm_body_type` does at lowering time;
                // it reads the payload's declared type off the enum
                // instead. Without this the whole match infers nothing
                // and a `val` over it is rejected, even though lowering
                // would have handled it.
                arms.iter()
                    .find_map(|a| self.arm_payload_binding_type(&scrutinee, a))
                    .or_else(|| {
                        arms.iter()
                            .find_map(|a| self.arm_compound_binding_type(&scrutinee, a))
                    })
            }
            Expr::Call(fn_name, args_ref) => {
                // Phase 6b: a FunctionPtr binding (HOF parameter
                // or closure-returning call result) carries its
                // own return type; resolve through the binding
                // map first so `val r = f(x)` infers correctly
                // even when `f` isn't a top-level function.
                if let Some(super::bindings::Binding::FunctionPtr { ret_ty, .. }) =
                    self.bindings.get(&fn_name)
                {
                    return Some(*ret_ty);
                }
                // A closure binding shadows a top-level function of the
                // same name, so it is consulted first -- same order as
                // `resolve_call_target`, which decides the actual callee.
                self.closure_bindings
                    .get(&fn_name)
                    .map(|link| link.func_id)
                    .or_else(|| self.module.lookup_function(None, fn_name))
                    .map(|id| self.module.function(id).return_type)
                    // A generic template is *not* in the function index
                    // (its instances are registered under the mangled
                    // name only), so the lookup above finds nothing.
                    // Substitute the inferred type args into the
                    // declared return type instead — the instance was
                    // already created while lowering the call itself,
                    // so the inference is guaranteed to succeed when
                    // the program type-checked.
                    .or_else(|| {
                        let template = self.generic_funcs.get(&fn_name)?;
                        let arg_exprs = match self.program.expression.get(&args_ref) {
                            Some(Expr::ExprList(items)) => items,
                            _ => return None,
                        };
                        let mut inferred: std::collections::HashMap<
                            DefaultSymbol,
                            Type,
                        > = std::collections::HashMap::new();
                        for ((_pname, ptype), arg) in
                            template.parameter.iter().zip(arg_exprs.iter())
                        {
                            self.infer_generic_args_from_param(
                                ptype,
                                arg,
                                &template.generic_params,
                                &mut inferred,
                            );
                        }
                        // STDLIB-TRAIT-BASE B5: the same last-resort
                        // evidence `resolve_call_target` uses. Without
                        // it the arm below cannot answer for a
                        // parameter that appears only in the return
                        // type, and the binding is reported as
                        // "could not infer scalar type" after the
                        // instance has already been created.
                        if let Some(hint) = self.pending_return_hint
                            && let Some(ret) = template.return_type.as_ref()
                            && template
                                .generic_params
                                .iter()
                                .any(|p| !inferred.contains_key(p))
                        {
                            self.bind_method_only_param(
                                ret,
                                hint,
                                &template.generic_params,
                                &mut inferred,
                            );
                        }
                        let type_args: Option<Vec<Type>> = template
                            .generic_params
                            .iter()
                            .map(|p| inferred.get(p).copied())
                            .collect();
                        let subst: std::collections::HashMap<DefaultSymbol, Type> =
                            template
                                .generic_params
                                .iter()
                                .copied()
                                .zip(type_args?)
                                .collect();
                        let ret = template.return_type.as_ref()?;
                        match ret {
                            // A return type written `T` reaches here as
                            // `Generic` or as `Identifier`, depending on
                            // whether the checker resolved it -- both
                            // name the same parameter.
                            TypeDecl::Generic(g) | TypeDecl::Identifier(g)
                                if subst.contains_key(g) =>
                            {
                                subst.get(g).copied()
                            }
                            other => lower_scalar(other),
                        }
                    })
            }
            Expr::AssociatedFunctionCall(struct_name, fn_name, _) => {
                // Module-qualified call: prefer
                // `(Some(struct_name), fn_name)` so cross-module
                // collisions resolve unambiguously, then fall back to
                // the bare lookup. Real associated method calls
                // aren't supported in expression position so the
                // None return at the bottom is the correct fallback.
                //
                // STDLIB-TRAIT-BASE B5: the qualifier is substituted
                // first, and the method registry consulted last, for
                // the same reason `lower_expr_associated_call` does
                // both -- `T::default()` in a monomorphised body names
                // a type parameter, and for a primitive `T` the impl
                // lives under the canonical name symbol.
                let struct_name = self
                    .concrete_type_param_name(struct_name)
                    .unwrap_or(struct_name);
                let written = self.written_qualifier_at(Some(expr_ref), struct_name);
                self.module
                    .lookup_function(Some(&written), fn_name)
                    .or_else(|| self.module.lookup_function(None, fn_name))
                    .or_else(|| {
                        crate::method_registry::lookup_method_func(
                            self.method_func_ids,
                            struct_name,
                            fn_name,
                            &[],
                        )
                    })
                    .map(|id| self.module.function(id).return_type)
            }
            Expr::BuiltinCall(func, args) => match func {
                // SIMD: the result of `__simd_splat` / `__simd_load`
                // is the type stamped onto the call by
                // `type_checker::simd::stamp_simd_result_types`;
                // everything else reads it off an argument.
                frontend::ast::BuiltinFunction::Simd(op) => {
                    self.simd_result_type(&op, &args)
                }
                frontend::ast::BuiltinFunction::Abs => {
                    // Polymorphic: forwards the operand's type.
                    args.first().and_then(|a| self.value_scalar(a))
                }
                frontend::ast::BuiltinFunction::Min
                | frontend::ast::BuiltinFunction::Max => {
                    args.first().and_then(|a| self.value_scalar(a))
                }
                // #121 Phase A: heap_alloc / heap_realloc return a
                // pointer-sized value. Pointer is U64 in the IR
                // (matches the `ptr` keyword's lowering).
                frontend::ast::BuiltinFunction::HeapAlloc
                | frontend::ast::BuiltinFunction::HeapRealloc => Some(Type::U64),
                // DICT-AOT-NEW Phase C: __builtin_sizeof returns u64.
                frontend::ast::BuiltinFunction::SizeOf => Some(Type::U64),
                // POINTER P1: the type-argument form answers the same
                // question, also as u64.
                frontend::ast::BuiltinFunction::SizeOfType(_) => Some(Type::U64),
                // MEMORY-ACCESS M1: the read's type is written at the
                // call, so this is the one pointer builtin whose
                // result type needs no surrounding annotation.
                // A compound `T` is not a scalar and answers `None`
                // here; that read goes through the per-leaf path in
                // `let_lowering.rs` and never asks this.
                frontend::ast::BuiltinFunction::PtrReadTyped(ty) => {
                    self.lower_scalar_with_subst(&ty)
                }
                // #121 Phase B-min: allocator handles are u64
                // sentinel values.
                frontend::ast::BuiltinFunction::DefaultAllocator
                | frontend::ast::BuiltinFunction::CurrentAllocator => Some(Type::U64),
                frontend::ast::BuiltinFunction::PtrIsNull => Some(Type::Bool),
                // MEMORY-ACCESS M3: the range questions.
                frontend::ast::BuiltinFunction::MemEq => Some(Type::Bool),
                frontend::ast::BuiltinFunction::MemFind
                | frontend::ast::BuiltinFunction::MemFindSeq => Some(Type::U64),
                frontend::ast::BuiltinFunction::PtrEq => Some(Type::Bool),
                frontend::ast::BuiltinFunction::NullPtr => Some(Type::U64),
                // `__builtin_ptr_offset(base, offset) -> ptr` is a
                // pointer-sized (u64) interior address.
                frontend::ast::BuiltinFunction::PtrOffset => Some(Type::U64),
                frontend::ast::BuiltinFunction::MemStat(_) => Some(Type::U64),
                // `__builtin_record_allocator_layout(...) -> unit`.
                frontend::ast::BuiltinFunction::RecordAllocatorLayout => Some(Type::Unit),
                // `__builtin_str_to_ptr(s) -> ptr` returns a u64-sized
                // pointer value.
                frontend::ast::BuiltinFunction::StrToPtr => Some(Type::U64),
                frontend::ast::BuiltinFunction::StrFromBytes => Some(Type::Str),
                // `__builtin_str_len(s) -> u64`.
                frontend::ast::BuiltinFunction::StrLen => Some(Type::U64),
                // `__builtin_to_string(value) -> str` (powers
                // string-interpolation desugaring; STR-INTERP-AOT).
                frontend::ast::BuiltinFunction::ToString => Some(Type::Str),
                // `__builtin_format(value, spec) -> str` — the same,
                // under a parse-time format spec (STR-INTERP-FMT).
                frontend::ast::BuiltinFunction::Format => Some(Type::Str),
                // SizeOf handled above already; no other builtins
                // currently route through value_scalar.
                // f64 math (sqrt/pow/sin/cos/tan/log/log2/exp
                // /floor/ceil) used to be `BuiltinFunction` arms.
                // Phase 4 moved them onto `extern fn`, so type
                // inference for those calls flows through the
                // regular `Expr::Call` path instead.
                _ => None,
            },
            Expr::BuiltinMethodCall(_receiver, _method, _args) => {
                // NOTE: `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}`
                // arms used to live here. Step F removed them;
                // numeric value-method type inference now flows
                // through the regular `MethodCall` arm against the
                // prelude's extension-trait impls.
                None
            }
            Expr::SliceAccess(obj, info) => {
                if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                    return None;
                }
                let obj_expr = self.program.expression.get(&obj)?;
                let arr_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                match self.bindings.get(&arr_sym)? {
                    Binding::Array { element_ty, .. } => Some(*element_ty),
                    // POINTER P2: `p[i]` on a struct / enum binding
                    // lowers as a `__getitem__` call, so its scalar
                    // type is that call's return type — needed by
                    // `as` casts and any other inference consumer of
                    // the indexed expression.
                    Binding::Struct { struct_id, .. } => {
                        let def = self.module.struct_def(*struct_id);
                        let getitem = self.interner.get("__getitem__")?;
                        self.method_call_return_type(
                            def.base_name,
                            Some((Type::Struct(*struct_id), def.type_args.clone())),
                            getitem,
                            &[],
                        )
                    }
                    Binding::Enum(storage) => {
                        let def = self.module.enum_def(storage.enum_id);
                        let getitem = self.interner.get("__getitem__")?;
                        self.method_call_return_type(
                            def.base_name,
                            Some((Type::Enum(storage.enum_id), def.type_args.clone())),
                            getitem,
                            &[],
                        )
                    }
                    _ => None,
                }
            }
            Expr::MethodCall(obj, method, args) => {
                // Numeric method calls (`x.abs()` for i64,
                // `x.sqrt()` for f64) reach the AST as `MethodCall`.
                // Peek through them so cast / let inference works on
                // call sites like `x.abs() as u64` without needing an
                // intermediate `val: i64` annotation.
                if args.is_empty()
                    && let Some(name) = self.interner.resolve(method)
                        && let Some(recv_ty) = self.value_scalar(&obj) {
                            match (name, recv_ty) {
                                ("abs", Type::I64) => return Some(Type::I64),
                                ("abs", Type::F64) => return Some(Type::F64),
                                ("sqrt", Type::F64) => return Some(Type::F64),
                                _ => {}
                            }
                        }
                // STR-INTERP-AOT: `s.concat(t)` returns str even
                // when the receiver is a string literal or another
                // `.concat()` chain — peek through any number of
                // levels so `value_scalar` of the desugared
                // interpolation chain works in print / let-rhs
                // contexts. Needed because the chain receiver is
                // typically `Expr::String` (the leading literal),
                // not an `Identifier`, which the binding-based
                // path below doesn't handle.
                if let Some(name) = self.interner.resolve(method)
                    && name == "concat" && args.len() == 1
                        && let Some(recv_ty) = self.value_scalar(&obj)
                            && matches!(recv_ty, Type::Str) {
                                return Some(Type::Str);
                            }
                let obj_expr = self.program.expression.get(&obj)?;
                // A compound-typed field / element receiver
                // (`x.name.to_str()`) resolves through the same leaf
                // tree a field read walks. Without this, `println(v)`
                // on a `Display` field could not be typed: the
                // checker rewrites the argument to `v.to_str()`, and
                // a `None` here reports it as "print accepts only
                // scalar values" — about a call the user never wrote.
                if matches!(obj_expr, Expr::FieldAccess(_, _) | Expr::TupleAccess(_, _)) {
                    let FieldChainResult::Struct { struct_id, .. } =
                        self.resolve_field_chain(&obj).ok()?
                    else {
                        return None;
                    };
                    let def = self.module.struct_def(struct_id);
                    return self.method_call_return_type(
                        def.base_name,
                        Some((Type::Struct(struct_id), def.type_args.clone())),
                        method,
                        &args,
                    );
                }
                // A primitive receiver that is not a name — a literal
                // (`21u8.twice()`), an arithmetic result, a cast. The
                // primitive dispatch path lowers the receiver as an
                // ordinary expression, so it needs no binding; without
                // this arm the type was simply unknown here and a cast
                // over the call reported "could not infer source scalar
                // type for `as` cast", about a receiver the user can
                // see the type of.
                let Expr::Identifier(recv_sym) = obj_expr else {
                    let ty = self.value_scalar(&obj)?;
                    let target_sym =
                        super::method_call::primitive_target_sym_for_ir_type(ty, self.interner)?;
                    return self.method_call_return_type(target_sym, None, method, &args);
                };
                // Track receiver self-type and per-receiver type
                // args separately so the generic-method peek path
                // below can handle struct AND enum receivers
                // uniformly.
                let (target_sym, recv_self): (DefaultSymbol, Option<(Type, Vec<Type>)>) =
                    match self.bindings.get(&recv_sym)? {
                        Binding::Struct { struct_id, .. } => {
                            let def = self.module.struct_def(*struct_id);
                            (
                                def.base_name,
                                Some((Type::Struct(*struct_id), def.type_args.clone())),
                            )
                        }
                        Binding::Enum(storage) => {
                            let def = self.module.enum_def(storage.enum_id);
                            (
                                def.base_name,
                                Some((Type::Enum(storage.enum_id), def.type_args.clone())),
                            )
                        }
                        // Step D: extension-trait dispatch — primitive
                        // receiver. Map the binding's IR type back to
                        // the canonical-name symbol; the rest of the
                        // lookup falls through into the same
                        // `method_func_ids` branch struct receivers use.
                        //
                        // This used to spell the mapping out again. It
                        // was the fourth copy of one table, and each
                        // copy that missed a width disabled the feature
                        // for it silently -- `f32` was absent from
                        // every one of them, so `impl <Trait> for f32`
                        // parsed and then vanished (NUM-W-ENUMERATION).
                        Binding::Scalar { ty, .. } => {
                            let sym = super::method_call::primitive_target_sym_for_ir_type(
                                *ty,
                                self.interner,
                            )?;
                            (sym, None)
                        }
                        _ => return None,
                    };
                self.method_call_return_type(target_sym, recv_self, method, &args)
            }
            _ => None,
        }
    }

    /// Return type of `<receiver>.method(args)` once the receiver has
    /// been resolved to its impl target symbol and (for nominal
    /// receivers) its self type + type args. Split out of
    /// `value_scalar`'s `MethodCall` arm so identifier receivers and
    /// compound field / element receivers share one lookup.
    fn method_call_return_type(
        &self,
        target_sym: DefaultSymbol,
        recv_self: Option<(Type, Vec<Type>)>,
        method: DefaultSymbol,
        args: &[ExprRef],
    ) -> Option<Type> {
        // CONCRETE-IMPL Phase 2c: unified dispatch — exact concrete
        // spec first, then the generic template peek, then a lone
        // concrete spec. Mirrors `resolve_method_target` so the peek
        // agrees with the call-time dispatch (a receiver the concrete
        // impls don't exactly match belongs to the generic impl).
        let recv_args_for_lookup: Vec<Type> = recv_self
            .as_ref()
            .map(|(_, args)| args.clone())
            .unwrap_or_default();
        match super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            target_sym,
            method,
            &recv_args_for_lookup,
        ) {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => {
                Some(self.module.function(id).return_type)
            }
            Some(super::method_registry::ResolvedMethodTarget::Template(template)) => {
                if let Some((self_ty, recv_type_args)) = recv_self
                    && template.generic_params.len() >= recv_type_args.len() {
                        let mut subst: HashMap<DefaultSymbol, Type> = HashMap::new();
                        for (i, p) in template.generic_params.iter().enumerate() {
                            if let Some(t) = recv_type_args.get(i).copied() {
                                subst.insert(*p, t);
                            }
                        }
                        let method_only_params: Vec<DefaultSymbol> = template
                            .generic_params
                            .iter()
                            .skip(recv_type_args.len())
                            .copied()
                            .collect();
                        if !method_only_params.is_empty() {
                            for (i, arg_ref) in args.iter().enumerate() {
                                let param_idx = i + 1;
                                if let Some((_, decl)) = template.parameter.get(param_idx)
                                    && let Some(arg_ty) = self.value_scalar(arg_ref)
                                        && let TypeDecl::Generic(p) | TypeDecl::Identifier(p) = decl
                                            && method_only_params.contains(p) {
                                                subst.entry(*p).or_insert(arg_ty);
                                            }
                            }
                        }
                        if let Some(ret) = &template.return_type {
                            return self.peek_method_return_type_with_self(ret, &subst, self_ty);
                        }
                        return Some(Type::Unit);
                    }
                    None
                }
                None => None,
        }
    }
}
