//! Call expression lowering.
//!
//! Resolves the target `FuncId` for a call expression (lazily
//! instantiating generic functions on demand), infers generic
//! type arguments from the call site's argument expressions,
//! and emits the IR `Call` instruction.
//!
//! - `resolve_call_target`: looks up `fn_name` in the module
//!   index. Non-generic functions hit the index directly;
//!   generic functions go through `instantiate_generic_function`
//!   with type arguments inferred via
//!   `infer_generic_args_from_param`.
//! - `infer_generic_args_from_param`: walks one parameter's
//!   `TypeDecl` against the corresponding argument expression's
//!   inferred IR `Type`, binding each generic parameter once.
//! - `instantiate_generic_function`: monomorphises a generic
//!   function declaration: substitutes the concrete type args
//!   into the parameter / return types, mints a fresh `FuncId`
//!   under a `(name, type_args)` cache key, and queues the body
//!   for lowering.
//! - `lower_type_with_subst`: `TypeDecl` -> `Type` lowering
//!   that respects an in-flight `(generic_param, concrete_type)`
//!   substitution. Used by `instantiate_generic_function` and by
//!   the method-call instantiation path.
//! - `lower_call`: top-level `f(args)` lowering. Calls
//!   `lower_call_args`, then `resolve_call_target`, then emits
//!   the `Call` instruction with the resolved return type.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::bindings::{Binding, TupleElementShape};
use super::templates::{instantiate_enum, instantiate_struct};
use super::types::lower_scalar;
use super::{FunctionLower, PendingGenericInstance};
use crate::ir::{FuncId, InstKind, Linkage, LocalId, Type, ValueId};

impl<'a> FunctionLower<'a> {
    /// Find (or instantiate) a `FuncId` for `fn_name`. Non-generic
    /// functions hit `module.function_index` directly. Generic
    /// functions are instantiated lazily: we infer the concrete type
    /// arguments from the call's argument expressions, mint a fresh
    /// `FuncId`, and queue the body for lowering.
    pub(super) fn resolve_call_target(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<FuncId, String> {
        // Closures Phase 5a: `lift_closure_binding` registers
        // `name -> FuncId` when it sees a `val name = fn(...)`
        // literal, so a subsequent `name(args)` lands here as a
        // regular direct `Call` to the lifted body.
        //
        // Lexical scoping: this map is consulted *before* the global
        // function table so a closure binding shadows a top-level
        // function of the same name, matching the type checker's
        // `visit_call` and the interpreter's `evaluate_function_call`.
        // Resolving the global first would call the wrong body (and
        // with the wrong arity, since a capturing closure carries an
        // implicit env parameter).
        if let Some(link) = self.closure_bindings.get(&fn_name).copied() {
            return Ok(link.func_id);
        }
        // Generic template first: a generic function is instantiated
        // *per concrete type-argument list*, and each instantiation is
        // registered in the module's function index under the bare
        // template name. Looking the bare name up first would find the
        // first instantiation regardless of the call's actual argument
        // types — `id(1u64)` then `id("x")` would call the u64 body
        // with a str handle, returning garbage on every backend (found
        // via the if-condition type-checking work: `same<str>` resolved
        // to the `same<u64>` FuncId).
        if let Some(template) = self.generic_funcs.get(&fn_name).cloned() {
            // Infer type-argument bindings by walking each parameter
            // declaration alongside the call's actual argument
            // expression. A `T` slot in the parameter type means
            // "take the IR Type of the matching arg"; concrete slots
            // are skipped (the type-checker has already verified
            // they line up).
            let arg_exprs: Vec<ExprRef> = match self
                .program
                .expression
                .get(args_ref)
            {
                Some(Expr::ExprList(items)) => items,
                _ => {
                    return Err(
                        "call arguments must be an ExprList".to_string(),
                    );
                }
            };
            if template.parameter.len() != arg_exprs.len() {
                return Err(format!(
                    "generic function `{}` expects {} argument(s), got {}",
                    self.interner.resolve(fn_name).unwrap_or("?"),
                    template.parameter.len(),
                    arg_exprs.len(),
                ));
            }
            let mut inferred: HashMap<DefaultSymbol, Type> = HashMap::new();
            for ((_pname, ptype), arg) in template.parameter.iter().zip(arg_exprs.iter())
            {
                self.infer_generic_args_from_param(
                    ptype,
                    arg,
                    &template.generic_params,
                    &mut inferred,
                );
            }
            let type_args: Option<Vec<Type>> = template
                .generic_params
                .iter()
                .map(|p| inferred.get(p).copied())
                .collect();
            let type_args = type_args.ok_or_else(|| {
                format!(
                    "cannot infer type arguments for generic function `{}` from call \
                     arguments; expected each `T` parameter to map to a known scalar / \
                     struct / enum type",
                    self.interner.resolve(fn_name).unwrap_or("?"),
                )
            })?;
            return self.instantiate_generic_function(fn_name, &template, type_args);
        }
        // Bare call: try the user-authored `(None, fn_name)` slot
        // first, then any unique `(Some(_), fn_name)` integrated
        // module's `pub fn`. See `Module::lookup_function` for the
        // ambiguity rule.
        if let Some(id) = self.module.lookup_function(None, fn_name) {
            return Ok(id);
        }
        Err(format!(
            "call to unknown function `{}` (only same-program functions are supported)",
            self.interner.resolve(fn_name).unwrap_or("?")
        ))
    }

    /// Walk one parameter declaration / call-site argument pair and
    /// record any generic-parameter bindings the pairing implies.
    /// Handles scalar generic params (`fn id<T>(x: T)` where `x`'s
    /// arg has a concrete scalar type), enum identifier args
    /// (`fn f<T>(o: Option<T>)` where the arg is an Option binding),
    /// and struct identifier args. A generic param nested inside a
    /// struct / enum / tuple type argument (`fn peek<T>(c: Cell<T>)`)
    /// is recovered by matching the declared type args positionally
    /// against the argument's concrete type args
    /// (AOT-GENERIC-THROUGH-STRUCT — the same zip the generic-method
    /// path's `bind_method_only_param` performs). Other shapes are
    /// silently skipped (`infer` returns None overall).
    pub(super) fn infer_generic_args_from_param(
        &self,
        ptype: &TypeDecl,
        arg: &ExprRef,
        generic_params: &[DefaultSymbol],
        inferred: &mut HashMap<DefaultSymbol, Type>,
    ) {
        match ptype {
            TypeDecl::Generic(g) | TypeDecl::Identifier(g)
                if generic_params.contains(g) =>
            {
                if let Some(ty) = self.value_scalar(arg) {
                    inferred.entry(*g).or_insert(ty);
                    return;
                }
                // Non-scalar: try identifier → struct/enum binding.
                if let Some(Expr::Identifier(sym)) = self.program.expression.get(arg)
                    && let Some(binding) = self.bindings.get(&sym) {
                        match binding {
                            Binding::Struct { struct_id, .. } => {
                                inferred.entry(*g).or_insert(Type::Struct(*struct_id));
                            }
                            Binding::Enum(s) => {
                                inferred.entry(*g).or_insert(Type::Enum(s.enum_id));
                            }
                            _ => {}
                        }
                    }
            }
            // AOT-GENERIC-THROUGH-STRUCT: a named-type parameter whose
            // type args contain a generic param (`Cell<T>`). The
            // argument's concrete instance carries its own type args
            // (`Cell<u64>` → `[U64]`), so zip the declared args
            // against them positionally and recurse into each pair.
            // The parser writes `Struct(N, args)` for `N<args>`
            // regardless of struct vs enum, so both arms accept both
            // concrete shapes and verify the base name before zipping.
            TypeDecl::Struct(name, decl_args) | TypeDecl::Enum(name, decl_args) => {
                match self.value_scalar(arg) {
                    Some(Type::Struct(id)) => {
                        let def = self.module.struct_def(id);
                        if def.base_name == *name {
                            for (d, a) in decl_args.iter().zip(def.type_args.iter()) {
                                self.bind_method_only_param(d, *a, generic_params, inferred);
                            }
                        }
                    }
                    Some(Type::Enum(id)) => {
                        let def = self.module.enum_def(id);
                        if def.base_name == *name {
                            for (d, a) in decl_args.iter().zip(def.type_args.iter()) {
                                self.bind_method_only_param(d, *a, generic_params, inferred);
                            }
                        }
                    }
                    _ => {}
                }
            }
            TypeDecl::Tuple(elems) => {
                if let Some(Type::Tuple(id)) = self.value_scalar(arg) {
                    let def = &self.module.tuple_defs[id.0 as usize];
                    for (d, a) in elems.iter().zip(def.iter()) {
                        self.bind_method_only_param(d, *a, generic_params, inferred);
                    }
                    return;
                }
                // A tuple *binding* carries only per-element shapes,
                // not an interned tuple id (`value_scalar` returns
                // None for it), so walk the shapes directly.
                if let Some(Expr::Identifier(sym)) = self.program.expression.get(arg)
                    && let Some(Binding::Tuple { elements }) = self.bindings.get(&sym) {
                        for (d, el) in elems.iter().zip(elements.iter()) {
                            let ty = match &el.shape {
                                TupleElementShape::Scalar { ty, .. } => *ty,
                                TupleElementShape::Struct { struct_id, .. } => {
                                    Type::Struct(*struct_id)
                                }
                                TupleElementShape::Tuple { tuple_id, .. } => {
                                    Type::Tuple(*tuple_id)
                                }
                            };
                            self.bind_method_only_param(d, ty, generic_params, inferred);
                        }
                    }
            }
            _ => {}
        }
    }

    /// Mint a fresh `FuncId` for `(template_name, type_args)`, declare
    /// the monomorphised signature on the module, and queue the body
    /// for lowering. Returns the cached id on subsequent hits.
    pub(super) fn instantiate_generic_function(
        &mut self,
        template_name: DefaultSymbol,
        template: &frontend::ast::Function,
        type_args: Vec<Type>,
    ) -> Result<FuncId, String> {
        if let Some(id) = self
            .generic_instances
            .get(&(template_name, type_args.clone()))
            .copied()
        {
            return Ok(id);
        }
        let subst: HashMap<DefaultSymbol, Type> = template
            .generic_params
            .iter()
            .copied()
            .zip(type_args.iter().copied())
            .collect();
        // Lower the param / return signatures with the active subst.
        let mut params: Vec<Type> = Vec::with_capacity(template.parameter.len());
        for (pname, ptype) in &template.parameter {
            let lowered = self.lower_type_with_subst(ptype, &subst).ok_or_else(|| {
                format!(
                    "generic function `{}`: cannot lower parameter `{}: {:?}` after \
                     substitution",
                    self.interner.resolve(template_name).unwrap_or("?"),
                    self.interner.resolve(*pname).unwrap_or("?"),
                    ptype,
                )
            })?;
            params.push(lowered);
        }
        let ret = match &template.return_type {
            Some(t) => self.lower_type_with_subst(t, &subst).ok_or_else(|| {
                format!(
                    "generic function `{}`: cannot lower return type `{:?}` after \
                     substitution",
                    self.interner.resolve(template_name).unwrap_or("?"),
                    t,
                )
            })?,
            None => Type::Unit,
        };
        // Mangle the export name with the type-arg list so each
        // instance gets a distinct linker symbol. Format mirrors what
        // print uses for header display: `toy_name__<T1, T2>`.
        let raw_name = self.interner.resolve(template_name).unwrap_or("anon");
        let arg_str = type_args
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let export_name = format!("toy_{raw_name}__{arg_str}");
        // `declare_function_anon`, not `declare_function`: the
        // instance must not occupy the bare template name in the
        // function index. With `declare_function`, the *first*
        // instantiation registered itself under `same`, so a later
        // `same<str>` call resolved to the `same<u64>` body — a
        // wrong-answer divergence the type checker could not catch
        // (generic equality was previously rejected outright, which
        // is how `same<T>` stayed untested until the if-condition
        // type-checking work made `a == b` on generics legal).
        let func_id = self
            .module
            .declare_function_anon(export_name, Linkage::Local, params, ret);
        // REF-Stage-2 (iv): record `&T` / `&mut T` parameters so call
        // sites hand them an address rather than a value.
        let param_ref_pointee: Vec<Option<crate::ir::Type>> = template
            .parameter
            .iter()
            .map(|(_, t)| crate::templates::param_ref_pointee_ty(t))
            .collect();
        self.module.function_mut(func_id).param_ref_pointee = param_ref_pointee;
        self.generic_instances
            .insert((template_name, type_args), func_id);
        // TEST-PERF: this body-bearing instance is now queued; the
        // reachability scan must not re-enqueue it as plain work.
        self.scheduled.insert(func_id);
        self.pending_generic_work.push(PendingGenericInstance {
            func_id,
            template_name,
            // POINTER P1: the monomorph subst rides with the queue
            // entry so the body can resolve a written generic
            // parameter (`__builtin_sizeof::<T>()`), the same way
            // `PendingMethodInstance` carries it for methods.
            subst: subst.iter().map(|(k, v)| (*k, *v)).collect(),
        });
        Ok(func_id)
    }

    /// Lower a `TypeDecl` with the active type-parameter substitution
    /// applied. Mirrors `lower_param_or_return_type` but for the
    /// already-resolved-once-per-instance generic function path.
    pub(super) fn lower_type_with_subst(
        &mut self,
        t: &TypeDecl,
        subst: &HashMap<DefaultSymbol, Type>,
    ) -> Option<Type> {
        if let Some(s) = lower_scalar(t) {
            return Some(s);
        }
        match t {
            TypeDecl::Generic(g) => subst.get(g).copied(),
            TypeDecl::Identifier(name) => {
                if let Some(ty) = subst.get(name).copied() {
                    return Some(ty);
                }
                if self.struct_defs.contains_key(name) {
                    instantiate_struct(
                        self.module,
                        self.struct_defs,
                        self.enum_defs,
                        *name,
                        Vec::new(),
                        self.interner,
                    )
                    .ok()
                    .map(Type::Struct)
                } else if self.enum_defs.contains_key(name) {
                    instantiate_enum(
                        self.module,
                        self.enum_defs,
                        self.struct_defs,
                        *name,
                        Vec::new(),
                        self.interner,
                    )
                    .ok()
                    .map(Type::Enum)
                } else {
                    None
                }
            }
            TypeDecl::Struct(name, args) if self.struct_defs.contains_key(name) => {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_with_subst(a, subst)?);
                }
                instantiate_struct(
                    self.module,
                    self.struct_defs,
                    self.enum_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Struct)
            }
            TypeDecl::Enum(name, args) | TypeDecl::Struct(name, args)
                if self.enum_defs.contains_key(name) =>
            {
                let mut concrete: Vec<Type> = Vec::with_capacity(args.len());
                for a in args {
                    concrete.push(self.lower_type_with_subst(a, subst)?);
                }
                instantiate_enum(
                    self.module,
                    self.enum_defs,
                    self.struct_defs,
                    *name,
                    concrete,
                    self.interner,
                )
                .ok()
                .map(Type::Enum)
            }
            // STDLIB-ITER: a tuple type argument (`Option<(K, V)>`)
            // needs its elements substituted before interning.
            TypeDecl::Tuple(elems) => {
                let mut concrete: Vec<Type> = Vec::with_capacity(elems.len());
                for e in elems {
                    concrete.push(self.lower_type_with_subst(e, subst)?);
                }
                Some(Type::Tuple(super::types::intern_tuple(self.module, concrete)))
            }
            _ => None,
        }
    }

    pub(super) fn lower_call(
        &mut self,
        fn_name: DefaultSymbol,
        args_ref: &ExprRef,
    ) -> Result<Option<ValueId>, String> {
        // Closures Phase 5b: indirect-call dispatch through a
        // `Binding::FunctionPtr` value. Hits when the bare-name
        // resolves to a function-typed local (HOF parameter or
        // re-bound closure). LoadLocal yields the U64 fn-pointer,
        // each arg goes through the regular expression lowering,
        // and `InstKind::CallIndirect` carries the signature so
        // codegen can `import_signature` + `call_indirect`.
        if let Some(super::bindings::Binding::FunctionPtr { local, param_tys, ret_ty }) =
            self.bindings.get(&fn_name).cloned()
        {
            let callee = self
                .emit(InstKind::LoadLocal(local), Some(Type::U64))
                .ok_or_else(|| "FunctionPtr load: LoadLocal returned no value".to_string())?;
            let arg_values = self.lower_call_args(args_ref)?;
            let _ = &param_tys; // borrowed for the InstKind below
            let result_ty = if matches!(ret_ty, Type::Unit) {
                None
            } else {
                Some(ret_ty)
            };
            // DEBUG-OBS: the frame is named after the binding, which
            // is what the user wrote and what the tree-walker prints.
            self.pending_frame_name = Some(fn_name);
            return Ok(self.emit(
                InstKind::CallIndirect {
                    callee,
                    args: arg_values,
                    param_tys,
                    ret_ty,
                },
                result_ty,
            ));
        }
        // Phase 6 capturing closure direct call: inject env_ptr as
        // the implicit first argument before the user-visible args.
        // `closure_bindings` carries the env_ptr ValueId from
        // `lift_closure_binding`'s MakeClosure emission.
        let capturing_env = self
            .closure_bindings
            .get(&fn_name)
            .and_then(|link| link.env_ptr);
        let target = self.resolve_call_target(fn_name, args_ref)?;
        let ret_ty = self.module.function(target).return_type;
        // Struct-returning calls in expression position aren't
        // supported; the user must bind the result with `val x = ...`.
        if matches!(ret_ty, Type::Struct(_)) {
            return Err(format!(
                "compiler MVP cannot use a struct-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Tuple(_)) {
            return Err(format!(
                "compiler MVP cannot use a tuple-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        if matches!(ret_ty, Type::Enum(_)) {
            return Err(format!(
                "compiler MVP cannot use an enum-returning call (`{}`) in expression position; bind the result with `val`",
                self.interner.resolve(fn_name).unwrap_or("?")
            ));
        }
        let mut arg_values = self.lower_call_args_with_target(args_ref, Some(target))?;
        // Phase 6: capturing closure direct call — prepend the
        // env_ptr in front of the user-visible args so the
        // callee's signature `(env: U64, ...user_params)` is
        // matched exactly.
        if let Some(env_ptr) = capturing_env {
            arg_values.insert(0, env_ptr);
        }
        // REF-Stage-2 (ii): if the callee declares writeback
        // returns (`&mut <compound>` parameters contributed leaf
        // types to `self_writeback_types`), gather the caller-
        // side leaf locals from the matching `&mut <var>` args
        // and emit `CallWithSelfWriteback` so the call's trailing
        // returns flow back into the caller's bindings.
        let writeback_dests = if !self.module.function(target).self_writeback_types.is_empty() {
            self.collect_compound_writeback_dests(args_ref)?
        } else {
            Vec::new()
        };
        if !writeback_dests.is_empty() {
            // Sanity: caller dest count must match callee writeback type count.
            let expected_writeback = self.module.function(target).self_writeback_types.len();
            if writeback_dests.len() != expected_writeback {
                return Err(format!(
                    "internal error: call to `{}` has {} writeback dests but callee declared {} writeback returns",
                    self.interner.resolve(fn_name).unwrap_or("?"),
                    writeback_dests.len(),
                    expected_writeback,
                ));
            }
            let ret_ty_opt = if ret_ty.produces_value() {
                Some(ret_ty)
            } else {
                None
            };
            let ret_dest = ret_ty_opt.map(|ty| {
                self.module.function_mut(self.func_id).add_local(ty)
            });
            self.emit(
                InstKind::CallWithSelfWriteback {
                    target,
                    args: arg_values,
                    ret_dest,
                    ret_ty: ret_ty_opt,
                    self_dests: writeback_dests,
                },
                None,
            );
            // A5-P2-MVP-C: drain `&mut dyn Trait` slot read-backs.
            // The CallWithSelfWriteback path mixes regular `&mut T`
            // compound writebacks (return-tuple) with dyn writebacks
            // (stack-slot read-back); both are independent and only
            // the dyn side uses the pending queue.
            self.drain_dyn_mut_writebacks()?;
            // Surface the user-return value (loaded from the
            // ret_dest local) so the caller's expression-position
            // consumer sees a normal ValueId.
            return match (ret_dest, ret_ty_opt) {
                (Some(local), Some(ty)) => {
                    Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                }
                _ => Ok(None),
            };
        }
        let inst = InstKind::Call {
            target,
            args: arg_values,
        };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        let call_value = self.emit(inst, result_ty);
        // A5-P2-MVP-C: drain `&mut dyn Trait` slot read-backs after
        // the regular `Call`. See the
        // `CallWithSelfWriteback` branch above for the parallel
        // path that drains in the same way.
        self.drain_dyn_mut_writebacks()?;
        Ok(call_value)
    }

    /// REF-Stage-2 (ii): walk a call's argument list, collect the
    /// caller-side leaf locals for every `&mut <compound-var>`
    /// argument. The order matches `Function.self_writeback_types`
    /// (each compound `&mut T` parameter contributed its leaves
    /// in declaration order during the callee's `lower_body`).
    /// Returns an empty Vec when no writeback args are present.
    pub(super) fn collect_compound_writeback_dests(
        &self,
        args_ref: &ExprRef,
    ) -> Result<Vec<LocalId>, String> {
        let items = match self.program.expression.get(args_ref) {
            Some(frontend::ast::Expr::ExprList(items)) => items,
            _ => return Ok(Vec::new()),
        };
        self.collect_compound_writeback_dests_slice(&items)
    }

    /// Slice-based variant for `MethodCall` (which carries args as
    /// a `Vec<ExprRef>` instead of an `ExprList` reference).
    pub(super) fn collect_compound_writeback_dests_slice(
        &self,
        items: &[ExprRef],
    ) -> Result<Vec<LocalId>, String> {
        let mut dests: Vec<LocalId> = Vec::new();
        for a in items {
            let inner = match self.program.expression.get(a) {
                Some(frontend::ast::Expr::Unary(frontend::ast::UnaryOp::BorrowMut, inner)) => {
                    inner
                }
                _ => continue,
            };
            let sym = match self.program.expression.get(&inner) {
                Some(frontend::ast::Expr::Identifier(s)) => s,
                _ => continue,
            };
            match self.bindings.get(&sym) {
                Some(super::bindings::Binding::Struct { fields, .. }) => {
                    for (l, _) in super::bindings::flatten_struct_locals(fields) {
                        dests.push(l);
                    }
                }
                Some(super::bindings::Binding::Tuple { elements }) => {
                    for (l, _) in super::bindings::flatten_tuple_element_locals(elements) {
                        dests.push(l);
                    }
                }
                Some(super::bindings::Binding::Enum(storage)) => {
                    Self::flatten_enum_dests_into(storage, &mut dests);
                }
                _ => {} // Scalar bindings handled by AddressOf path; not a writeback dest.
            }
        }
        Ok(dests)
    }
}
