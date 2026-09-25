//! Method-call lowering and generic-method instantiation.
//!
//! This file owns the impl-block methods that:
//!
//! - Resolve `obj.method(args)` to a target `FuncId` (Phase R), with
//!   automatic monomorphisation for generic methods (Phase R3) and
//!   method-only generic params (Phase X) inferred from arg types.
//! - Lower the call itself (`lower_method_call`), prepending the
//!   receiver's leaf scalars to the cranelift call's arguments.
//! - Provide signature-substitution helpers
//!   (`lower_method_param_type`, `peek_method_return_type`) used both
//!   by the instantiator and by `value_scalar`.
//! - Provide a peek-only target resolution (`resolve_method_target`)
//!   for paths (val rhs, print arg) that need to know the call shape
//!   before deciding whether to emit `CallStruct` / `CallTuple` /
//!   `CallEnum`.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::method_registry::PendingMethodInstance;
use super::templates::lower_param_or_return_type;
use super::types::lower_scalar;
use super::FunctionLower;
use crate::ir::{Const, FuncId, InstKind, Linkage, LocalId, StructId, Type, ValueId};

/// Map an IR `Type` for a primitive scalar receiver back to the
/// canonical-name symbol that `Stmt::ImplBlock` uses as its target
/// for `impl Trait for <PrimitiveType> { ... }`. Returns `None` for
/// non-primitive types (struct / enum / tuple / unit) and for
/// primitives whose canonical name has never been interned (no impl
/// targets that primitive in this program — caller short-circuits).
///
/// Used by `lower_method_call`'s Step D extension-trait path so
/// `i64.neg()` can be looked up in the same `method_func_ids`
/// table that struct methods use.
pub(super) fn primitive_target_sym_for_ir_type(
    ty: Type,
    interner: &DefaultStringInterner,
) -> Option<DefaultSymbol> {
    // NUM-W-ENUMERATION: no `_ =>` arm. The catch-all is precisely how
    // the narrow widths went missing here while registration already
    // knew them — an `impl <Trait> for u8` lowered and was then
    // unreachable, the call falling past this function to the
    // struct/enum binding path and failing with "the method receiver
    // must be a struct or enum binding", which names neither the width
    // nor the impl. Spelled out, a new IR type will not compile until
    // someone decides which side it belongs on.
    //
    // The names themselves come from `TypeDecl::PRIMITIVE_IMPL_TARGETS`
    // by way of the `TypeDecl` each IR type stands for.
    let decl = match ty {
        Type::Bool => TypeDecl::Bool,
        Type::I8 => TypeDecl::Int8,
        Type::U8 => TypeDecl::UInt8,
        Type::I16 => TypeDecl::Int16,
        Type::U16 => TypeDecl::UInt16,
        Type::I32 => TypeDecl::Int32,
        Type::U32 => TypeDecl::UInt32,
        Type::I64 => TypeDecl::Int64,
        Type::U64 => TypeDecl::UInt64,
        Type::F32 => TypeDecl::Float32,
        Type::F64 => TypeDecl::Float64,
        // `Type::Str` is a pointer-sized opaque handle in IR (Phase T).
        // Extension-trait dispatch (`s.hash()` from `core/std/hash.t`'s
        // `impl Hash for str`) routes through the same per-target
        // method registry as the numeric primitives.
        Type::Str => TypeDecl::String,
        // `ptr` lowers to `Type::U64`, so it is indistinguishable from
        // `u64` here and cannot be dispatched on. No stdlib extension
        // trait targets `ptr` today; giving it its own IR type is what
        // reaching one would take.
        //
        // The rest are not single SSA values and never reach a
        // primitive receiver.
        Type::Unit
        | Type::Vector(_)
        | Type::Struct(_)
        | Type::Tuple(_)
        | Type::Enum(_) => return None,
    };
    interner.get(decl.primitive_canonical_name()?)
}

/// CODE-SIZE-SELF-ABI: what a call site still owes its receiver after
/// the call is emitted.
///
/// A materialised receiver was copied into a slot before the call; the
/// callee wrote its mutations there, so the caller has to read them
/// back into its leaf locals. Forgetting that is a lost update, and it
/// is easy to forget because the reload happens *after* an emit that
/// may be several functions away from where the address was made.
///
/// So it is a guard value rather than a field on the lowerer.
/// `#[must_use]` catches ignoring it where it is produced; the `Drop`
/// below catches the other half, where it rides inside a
/// `CompoundMethodCall` that some call site drops without looking at
/// the field. Both failures are loud, which is the point: the quiet
/// version of this bug is a mutation that vanishes.
#[must_use = "a materialised receiver must be read back (`apply`) or explicitly dropped (`skip`)"]
pub(super) struct ReceiverReload {
    /// The slots owed a read-back: none when every pointer was
    /// forwarded (caller and callee look at the same bytes), and more
    /// than one when a call hands over the receiver *and* a `&T` /
    /// `&mut T` argument by address.
    slots: Vec<(ValueId, Vec<(LocalId, u64, Type)>)>,
}

impl ReceiverReload {
    /// The forwarded case: nothing owed.
    pub(super) fn none() -> Self {
        ReceiverReload { slots: Vec::new() }
    }

    pub(super) fn slot(addr: ValueId, layout: Vec<(LocalId, u64, Type)>) -> Self {
        ReceiverReload { slots: vec![(addr, layout)] }
    }

    /// Take on `other`'s obligations too, so one value carries every
    /// read-back a call owes (a `CompoundMethodCall` has room for one).
    pub(super) fn merge(&mut self, mut other: ReceiverReload) {
        self.slots.append(&mut other.slots);
    }

    /// Read the receiver back. Call immediately after the call.
    pub(super) fn apply(mut self, lower: &mut super::FunctionLower<'_>) {
        for (addr, layout) in std::mem::take(&mut self.slots) {
            lower.reload_receiver_slot(addr, &layout);
        }
    }

    /// Deliberately not read back -- only correct when the receiver is
    /// being destroyed, as in auto-drop glue.
    pub(super) fn skip(mut self) {
        self.slots.clear();
    }
}

impl Drop for ReceiverReload {
    fn drop(&mut self) {
        if !self.slots.is_empty() && !std::thread::panicking() {
            panic!(
                "internal error (CODE-SIZE-SELF-ABI): a materialised receiver was dropped \
                 without being read back; whichever call site emits the call must call \
                 `ReceiverReload::apply` (or `skip` when the value is being destroyed)"
            );
        }
    }
}

impl<'a> FunctionLower<'a> {
    /// `&self` cousin of `lower_method_param_type` — used by
    /// `value_scalar`'s MethodCall arm so val/var annotation
    /// inference can resolve generic method return types without
    /// triggering monomorphisation.
    ///
    /// Self-type-agnostic so enum-receiver method calls
    /// (`Option<T>::unwrap_or` etc.) get their return type resolved
    /// without forcing the caller to also know which side of the
    /// struct/enum split it's on.
    pub(super) fn peek_method_return_type_with_self(
        &self,
        ty: &TypeDecl,
        subst: &HashMap<DefaultSymbol, Type>,
        self_type: Type,
    ) -> Option<Type> {
        match ty {
            TypeDecl::Self_ => Some(self_type),
            TypeDecl::Identifier(sym) if self.interner.resolve(*sym) == Some("Self") => {
                Some(self_type)
            }
            TypeDecl::Generic(p) => subst.get(p).copied(),
            TypeDecl::Identifier(sym) => subst.get(sym).copied().or_else(|| lower_scalar(ty)),
            other => lower_scalar(other),
        }
    }

    /// Lower a method's declared parameter / return TypeDecl with
    /// `Self` and any `Generic(P)` references resolved against the
    /// active substitution. `self_type` is the IR type for `Self`
    /// (always the receiver's `Type::Struct(...)` in Phase R3).
    pub(super) fn lower_method_param_type(
        &mut self,
        ty: &TypeDecl,
        subst: &HashMap<DefaultSymbol, Type>,
        self_type: Type,
    ) -> Option<Type> {
        match ty {
            TypeDecl::Self_ => Some(self_type),
            TypeDecl::Identifier(sym) if self.interner.resolve(*sym) == Some("Self") => {
                Some(self_type)
            }
            TypeDecl::Generic(p) => subst.get(p).copied(),
            TypeDecl::Identifier(sym) => {
                if let Some(t) = subst.get(sym).copied() {
                    return Some(t);
                }
                lower_param_or_return_type(
                    ty,
                    self.struct_defs,
                    self.enum_defs,
                    self.module,
                    self.interner,
                )
            }
            // For struct / enum / tuple shapes that may contain
            // generic params, walk recursively and rebuild via the
            // boundary lowerer once everything is concrete. `Self`
            // rides along so it resolves at any depth
            // (`-> Option<Self>`, SELF-IN-TYPE-ARG).
            _ => self.lower_type_with_subst_self(ty, subst, Some(self_type)),
        }
    }

    /// Materialise (or fetch) the FuncId for a generic-method
    /// instance. Handles two flavours uniformly: impl-level generic
    /// params (covered by the receiver's `struct_def.type_args`,
    /// e.g. `impl<T> Cell<T> { fn get(self) -> T }`) and
    /// method-only generic params beyond the impl's count
    /// (`impl Box { fn pick<U>(self, a: U, b: U) -> U }`),
    /// inferred from the call site's argument types.
    pub(super) fn instantiate_generic_method_with_args(
        &mut self,
        target_sym: DefaultSymbol,
        method_sym: DefaultSymbol,
        template: &frontend::ast::MethodFunction,
        recv_struct_id: StructId,
        arg_refs: &[ExprRef],
    ) -> Result<FuncId, String> {
        let recv_type_args = self.module.struct_def(recv_struct_id).type_args.clone();
        self.instantiate_generic_method_with_self_type(
            target_sym,
            method_sym,
            template,
            Type::Struct(recv_struct_id),
            recv_type_args,
            arg_refs,
        )
    }

    /// Receiver-type-agnostic form of
    /// `instantiate_generic_method_with_args`: takes the explicit
    /// `Self` cranelift `Type` and the receiver's pre-resolved
    /// `type_args` so it works for both `Type::Struct(id)` and
    /// `Type::Enum(id)` receivers. Used by the enum-method dispatch
    /// path that the (auto-loaded) `impl<T> Option<T>` etc. needs.
    pub(super) fn instantiate_generic_method_with_self_type(
        &mut self,
        target_sym: DefaultSymbol,
        method_sym: DefaultSymbol,
        template: &frontend::ast::MethodFunction,
        self_type: Type,
        recv_type_args: Vec<Type>,
        arg_refs: &[ExprRef],
    ) -> Result<FuncId, String> {
        let impl_param_count = recv_type_args.len();
        if template.generic_params.len() < impl_param_count {
            return Err(format!(
                "compiler MVP: generic method `{}::{}` has fewer generic params than receiver type_args",
                self.interner.resolve(target_sym).unwrap_or("?"),
                self.interner.resolve(method_sym).unwrap_or("?"),
            ));
        }
        let mut subst: HashMap<DefaultSymbol, Type> = HashMap::new();
        for (i, p) in template.generic_params.iter().enumerate() {
            if let Some(ty) = recv_type_args.get(i).copied() {
                subst.insert(*p, ty);
            }
        }
        let method_only_params: Vec<DefaultSymbol> = template
            .generic_params
            .iter()
            .skip(impl_param_count)
            .copied()
            .collect();
        if !method_only_params.is_empty() {
            // `template.parameter` includes `self` only when the
            // receiver is by-value (`self: Self`). `&self` /
            // `&mut self` receivers are kept OUT of the parameter
            // list by the frontend, so the call args map straight
            // onto the params in that case. Detect which case we
            // are in from the lengths: a by-value receiver leaves
            // one more param than there are call args (note that
            // `has_self_param` cannot tell the two apart — it is
            // false for `self: Self` receivers).
            let param_offset = if template.parameter.len() > arg_refs.len() {
                1
            } else {
                0
            };
            // Walk each pair, looking for
            // `Generic(P)` slots that match a method-only param.
            for (i, arg_ref) in arg_refs.iter().enumerate() {
                let param_idx = i + param_offset;
                let declared = match template.parameter.get(param_idx) {
                    Some((_, t)) => t.clone(),
                    None => continue,
                };
                // A function-typed parameter has to be matched
                // against the argument's *signature*: `map<U>(f: fn
                // (T) -> U)` mentions `U` only inside the closure's
                // return type, and the argument's IR type is a bare
                // U64 pointer that says nothing about it.
                self.bind_method_only_param_from_fn_arg(
                    &declared,
                    arg_ref,
                    &method_only_params,
                    &mut subst,
                );
                let arg_ty = match self.value_scalar(arg_ref) {
                    Some(t) => t,
                    None => continue,
                };
                self.bind_method_only_param(&declared, arg_ty, &method_only_params, &mut subst);
            }
            for p in &method_only_params {
                if !subst.contains_key(p) {
                    return Err(format!(
                        "compiler MVP: could not infer method-only generic param `{}` for `{}::{}`",
                        self.interner.resolve(*p).unwrap_or("?"),
                        self.interner.resolve(target_sym).unwrap_or("?"),
                        self.interner.resolve(method_sym).unwrap_or("?"),
                    ));
                }
            }
        }
        let inst_args: Vec<Type> = template
            .generic_params
            .iter()
            .filter_map(|p| subst.get(p).copied())
            .collect();
        if let Some(id) = self
            .method_instances
            .get(&(target_sym, method_sym, inst_args.clone()))
            .copied()
        {
            return Ok(id);
        }
        // `self_type` is supplied by the caller (Type::Struct(...) or
        // Type::Enum(...)) so this branch works for both struct and
        // enum receivers.
        let mut params: Vec<Type> = Vec::with_capacity(template.parameter.len() + 1);
        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `template.parameter`. Prepend
        // the self_type so the IR signature has the same shape the
        // body lowering will see (lower_method_body inserts a
        // matching synthetic `(self, Self)` entry into its
        // parameter list).
        if template.has_self_param
            && template.parameter.first().map(|(n, _)| {
                self.interner.resolve(*n) != Some("self")
            }).unwrap_or(true)
        {
            params.push(self_type);
        }
        for (pname, pty) in &template.parameter {
            let lowered = self
                .lower_method_param_type(pty, &subst, self_type)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower generic method param `{}: {}` after subst",
                        self.interner.resolve(*pname).unwrap_or("?"),
                        crate::spelling::spell_type_decl(self.interner, pty)
                    )
                })?;
            params.push(lowered);
        }
        let ret = match &template.return_type {
            Some(ty) => self
                .lower_method_param_type(ty, &subst, self_type)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower generic method return type `{}` after subst",
                        crate::spelling::spell_type_decl(self.interner, ty)
                    )
                })?,
            None => Type::Unit,
        };
        let target_str = self.interner.resolve(target_sym).unwrap_or("?");
        let method_str = self.interner.resolve(method_sym).unwrap_or("?");
        // DIAG-DEBUG-FMT-OK: a linker symbol, not a diagnostic. The
        // mangling wants a stable per-type token and nothing else
        // reads it.
        let arg_str = inst_args
            .iter()
            .map(|t| format!("{:?}", t))
            .collect::<Vec<_>>()
            .join("_");
        let export_name = format!("toy_{}__{}__{}", target_str, method_str, arg_str);
        let func_id = self
            .module
            .declare_function_anon(export_name, Linkage::Local, params, ret);
        // DEBUG-OBS D4: the mangled name has the monomorph's type
        // arguments in it; a backtrace wants what the user wrote.
        self.module
            .set_display_name(func_id, format!("{target_str}::{method_str}"));
        // REF-Stage-2 (ii-method): pre-populate the writeback shape
        // for this generic method instance so callers compiled
        // before the body see the correct trailing-return layout.
        // Same shape as the non-generic decl-time pre-populate in
        // `lower_program`.
        super::program::populate_method_writeback_types(
            self.module,
            func_id,
            template,
            // Generic instances target structs, where `&Self` is a
            // compound reference: it erases to leaves rather than
            // becoming an address, so there is nothing to resolve.
            None,
        );
        self.method_instances
            .insert((target_sym, method_sym, inst_args), func_id);
        // TEST-PERF: queued body-bearing instance — not plain work.
        self.scheduled.insert(func_id);
        // Capture the subst (including a synthetic `Self` entry when
        // the symbol is already interned) so the body lowering of
        // this monomorph can resolve val/var annotations that
        // reference generic params or `Self`. The interner is
        // borrowed immutably here; `Self` is virtually always
        // pre-interned because the parser sees it in any impl
        // block, so `get` is sufficient.
        let mut subst_vec: Vec<(DefaultSymbol, Type)> = subst.into_iter().collect();
        if let Some(self_sym) = self.interner.get("Self") {
            subst_vec.push((self_sym, self_type));
        }
        self.pending_method_work.push(PendingMethodInstance {
            func_id,
            target_sym,
            method_sym,
            subst: subst_vec,
        });
        Ok(func_id)
    }

    /// Walk `declared` against `arg_ty`, binding any `Generic(P)`
    /// (or defensive `Identifier(P)`) entries in `params` to the
    /// runtime type.
    /// Bind method-only generic params that appear inside a
    /// **function-typed** parameter, by unifying the declared
    /// signature against the argument's.
    ///
    /// `fn map<U>(self: Self, f: fn (T) -> U) -> Option<U>` is the
    /// motivating case: `U` occurs nowhere except the closure's return
    /// type, so `bind_method_only_param` — which compares the declared
    /// type against the argument's *IR* type — never sees it. A
    /// closure argument lowers to a U64 pointer, and a U64 carries no
    /// return type. The signature has to come from the AST (a closure
    /// literal's declared shape) or from a `FunctionPtr` binding.
    ///
    /// Without this, `o.map(fn(x: i64) -> bool { ... })` type checked
    /// and ran on the interpreter but failed to lower, so the stdlib's
    /// own `Option::map` / `Result::map` were interpreter-only.
    pub(super) fn bind_method_only_param_from_fn_arg(
        &self,
        declared: &TypeDecl,
        arg_ref: &ExprRef,
        params: &[DefaultSymbol],
        subst: &mut HashMap<DefaultSymbol, Type>,
    ) {
        let TypeDecl::Function(declared_params, declared_ret) = declared else {
            return;
        };
        let Some(arg_expr) = self.program.expression.get(arg_ref) else {
            return;
        };
        match arg_expr {
            Expr::Closure { params: closure_params, return_type, .. } => {
                for (declared_p, (_, actual_p)) in
                    declared_params.iter().zip(closure_params.iter())
                {
                    if let Some(ty) = crate::types::lower_scalar(actual_p) {
                        self.bind_method_only_param(declared_p, ty, params, subst);
                    }
                }
                if let Some(actual_ret) = return_type
                    && let Some(ty) = crate::types::lower_scalar(&actual_ret)
                {
                    self.bind_method_only_param(declared_ret, ty, params, subst);
                }
            }
            // A function value passed along (a HOF parameter forwarded
            // to another HOF): the binding already carries the lowered
            // signature.
            Expr::Identifier(sym) => {
                if let Some(Binding::FunctionPtr { param_tys, ret_ty, .. }) =
                    self.bindings.get(&sym)
                {
                    for (declared_p, actual) in declared_params.iter().zip(param_tys.iter()) {
                        self.bind_method_only_param(declared_p, *actual, params, subst);
                    }
                    self.bind_method_only_param(declared_ret, *ret_ty, params, subst);
                }
            }
            _ => {}
        }
    }

    pub(super) fn bind_method_only_param(
        &self,
        declared: &TypeDecl,
        arg_ty: Type,
        params: &[DefaultSymbol],
        subst: &mut HashMap<DefaultSymbol, Type>,
    ) {
        match declared {
            TypeDecl::Generic(p) | TypeDecl::Identifier(p) if params.contains(p) => {
                subst.entry(*p).or_insert(arg_ty);
            }
            // STDLIB-ITER-ADAPT: a generic struct / enum argument
            // (`other: VecIter<U>`) carries the method-only param
            // nested inside its type args. Match positionally against
            // the argument's resolved type args so `zip<U>(other:
            // VecIter<U>)` binds U from a `VecIter<u64>` argument.
            TypeDecl::Struct(_, decl_args) => {
                if let Type::Struct(id) = arg_ty {
                    let def = self.module.struct_def(id);
                    for (d, a) in decl_args.iter().zip(def.type_args.iter()) {
                        self.bind_method_only_param(d, *a, params, subst);
                    }
                }
            }
            TypeDecl::Enum(_, decl_args) => {
                if let Type::Enum(id) = arg_ty {
                    let def = self.module.enum_def(id);
                    for (d, a) in decl_args.iter().zip(def.type_args.iter()) {
                        self.bind_method_only_param(d, *a, params, subst);
                    }
                }
            }
            _ => {}
        }
    }

    /// Resolve `obj.method(args)` to a `(FuncId, receiver Binding)`
    /// pair without lowering the call itself. Used by paths (val
    /// rhs, print argument, future expression-position consumers)
    /// that need to know the target's signature before deciding
    /// what call shape to emit.
    pub(super) fn resolve_method_target(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<Option<(FuncId, Binding)>, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "method-call receiver missing".to_string())?;
        // A field-access receiver (`holder.inner.get()`) has no
        // binding of its own — its leaf locals live inside the
        // parent binding. `resolve_method_receiver_binding`
        // synthesises the struct binding from the field chain, so the
        // compound-method paths (match scrutinees, compound-returning
        // val rhs) can treat it exactly like a bare identifier
        // receiver, writeback included (the leaves are the parent's
        // locals, so `&mut self` mutations propagate). Any chain that
        // does not resolve to a struct stays "not this shape"
        // (`Ok(None)`), preserving the peek contract for the scalar
        // fall-through.
        let binding = match obj_expr {
            Expr::Identifier(s) => match self.bindings.get(&s).cloned() {
                Some(b) => b,
                None => return Ok(None),
            },
            Expr::FieldAccess(_, _) => match self.resolve_method_receiver_binding(obj) {
                Ok(b) => b,
                Err(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        // CONCRETE-IMPL Phase 2b: receiver's IR type args distinguish
        // multiple `impl Foo for Container<X>` impls; consult them
        // when looking up the matching FuncId. Templates likewise.
        // Phase 2c: the unified cross-registry dispatch resolves a
        // lone concrete spec only after the generic template is
        // exhausted, so `impl<T> C<T>` catches the receivers the
        // concrete impls don't exactly match.
        let (target_sym, recv_type_args): (DefaultSymbol, Vec<crate::ir::Type>) = match &binding {
            Binding::Struct { struct_id, .. } => {
                let def = self.module.struct_def(*struct_id);
                (def.base_name, def.type_args.clone())
            }
            Binding::Enum(storage) => {
                let def = self.module.enum_def(storage.enum_id);
                (def.base_name, def.type_args.clone())
            }
            _ => return Ok(None),
        };
        let resolved = super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            target_sym,
            method,
            &recv_type_args,
        );
        let id = match resolved {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => id,
            Some(super::method_registry::ResolvedMethodTarget::Template(template)) => {
                // Enum receivers go through the type-args-aware
                // instantiator, the same split the expression-position
                // dispatch above makes. Bailing out on them (which this
                // used to do) meant every caller that resolves a target
                // before choosing a call shape — `val` binding, `print`
                // argument, `match` scrutinee — saw a generic method on an
                // enum as "not a method", and fell through to a message
                // telling the user to bind it with `val` when they already
                // had. `Option::map` and `Result::map` were unreachable
                // from the compiler for exactly this reason.
                match &binding {
                    Binding::Struct { struct_id, .. } => self.instantiate_generic_method_with_args(
                        target_sym,
                        method,
                        &template,
                        *struct_id,
                        args,
                    )?,
                    Binding::Enum(storage) => {
                        let enum_id = storage.enum_id;
                        let recv_type_args = self.module.enum_def(enum_id).type_args.clone();
                        self.instantiate_generic_method_with_self_type(
                            target_sym,
                            method,
                            &template,
                            Type::Enum(enum_id),
                            recv_type_args,
                            args,
                        )?
                    }
                    _ => return Ok(None),
                }
            }
            None => return Ok(None),
        };
        Ok(Some((id, binding)))
    }

    /// CONCRETE-IMPL-Phase-2c: resolve `(target, method)` for a
    /// struct receiver to a FuncId, instantiating the generic
    /// template against `struct_id` when no concrete spec matches.
    /// `args` are the call arguments (used only to bind method-only
    /// generic params during instantiation). `Ok(None)` when neither
    /// registry has the method.
    pub(super) fn resolve_struct_method_func_id(
        &mut self,
        target_sym: DefaultSymbol,
        method_sym: DefaultSymbol,
        struct_id: crate::ir::StructId,
        args: &[ExprRef],
    ) -> Result<Option<FuncId>, String> {
        let type_args = self.module.struct_def(struct_id).type_args.clone();
        match super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            target_sym,
            method_sym,
            &type_args,
        ) {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => Ok(Some(id)),
            Some(super::method_registry::ResolvedMethodTarget::Template(t)) => Ok(Some(
                self.instantiate_generic_method_with_args(
                    target_sym,
                    method_sym,
                    &t,
                    struct_id,
                    args,
                )?,
            )),
            None => Ok(None),
        }
    }

    /// Lower an `obj.method(args)` expression. Phase R1 dispatch is
    /// purely static: we resolve the receiver's struct (or enum)
    /// symbol, look up the method via the registry built in
    /// `lower_program`, and emit a regular `Call` with the
    /// receiver's leaf scalars prepended to the call's arg values.
    /// Closures Phase 8 helper: try to lower `obj.method(args)`
    /// as a field-call when `method` names a field of fn type
    /// rather than a registered method. Returns:
    ///   - `Ok(Some(_))` when the field-call path applied.
    ///   - `Ok(None)` when this isn't a field-call (caller
    ///     should fall through to the regular method-dispatch
    ///     path).
    ///   - `Err(_)` for actual errors (signature mismatch).
    pub(super) fn try_lower_field_closure_call(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<Option<ValueId>>, String> {
        // Receiver must be a bare identifier bound to a struct
        // (other shapes — chained method calls, field access —
        // can be added later if needed).
        let sym = match self.program.expression.get(obj) {
            Some(Expr::Identifier(s)) => s,
            _ => return Ok(None),
        };
        let (struct_id, field_bindings) = match self.bindings.get(&sym) {
            Some(Binding::Struct { struct_id, fields }) => (*struct_id, fields.clone()),
            _ => return Ok(None),
        };
        // Struct definition must have a field whose name matches
        // the method symbol AND whose declared type is `Function`.
        let method_name = self.interner.resolve(method).unwrap_or("");
        if method_name.is_empty() {
            return Ok(None);
        }
        // Resolve the receiver's IR struct name → look up the
        // AST struct template to find the matching field's
        // declared type.
        let ir_struct_name = self.module.struct_def(struct_id).base_name;
        let template = match self.struct_defs.get(&ir_struct_name) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        let field_decl = template
            .fields
            .iter()
            .find(|(n, _)| n == method_name)
            .map(|(_, t)| t.clone());
        let (param_tys_decl, ret_ty_decl) = match field_decl {
            Some(TypeDecl::Function(p, r)) => (p, r),
            _ => return Ok(None),
        };
        // Resolve the field's IR shape — it must be a scalar
        // local of `Type::U64` (lower_scalar maps Function → U64).
        let field_local = field_bindings
            .iter()
            .find(|fb| fb.name == method_name)
            .and_then(|fb| match &fb.shape {
                super::bindings::FieldShape::Scalar { local, ty: Type::U64 } => {
                    Some(*local)
                }
                _ => None,
            });
        let field_local = match field_local {
            Some(l) => l,
            None => return Ok(None),
        };
        // Lower IR types from the AST signature so we can build
        // the CallIndirect signature.
        let mut ir_param_tys: Vec<Type> = Vec::with_capacity(param_tys_decl.len());
        for pt in &param_tys_decl {
            let lowered = self.lower_scalar_with_subst(pt).ok_or_else(|| {
                format!(
                    "compiler MVP: field-call closure parameter type `{}` is not a primitive scalar",
                    crate::spelling::spell_type_decl(self.interner, pt)
                )
            })?;
            ir_param_tys.push(lowered);
        }
        let ir_ret_ty = self
            .lower_scalar_with_subst(&ret_ty_decl)
            .ok_or_else(|| {
                format!(
                    "compiler MVP: field-call closure return type `{}` is not a primitive scalar",
                    crate::spelling::spell_type_decl(self.interner, &ret_ty_decl)
                )
            })?;
        if args.len() != ir_param_tys.len() {
            return Err(format!(
                "field-call `{}` expects {} arg(s), got {}",
                method_name,
                ir_param_tys.len(),
                args.len()
            ));
        }
        // Load the env_ptr from the field local, evaluate args,
        // and emit CallIndirect (env-aware Phase 6b ABI).
        let callee = self
            .emit(InstKind::LoadLocal(field_local), Some(Type::U64))
            .ok_or_else(|| "field-call: LoadLocal returned no value".to_string())?;
        let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
        for a in args {
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "field-call argument produced no value".to_string())?;
            arg_values.push(v);
        }
        let result_ty = if matches!(ir_ret_ty, Type::Unit) {
            None
        } else {
            Some(ir_ret_ty)
        };
        let inst = InstKind::CallIndirect {
            callee,
            args: arg_values,
            param_tys: ir_param_tys,
            ret_ty: ir_ret_ty,
        };
        Ok(Some(self.emit(inst, result_ty)))
    }

    pub(super) fn lower_method_call(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        // Closures Phase 8: field-call dispatch. When the
        // receiver is a struct-bound identifier and the
        // requested name matches a field whose declared type is
        // `fn (T1, T2) -> R`, the call is really
        // `(load_field)(args)` — load the closure value (an
        // env_ptr) from the field local, then dispatch through
        // the env-aware `CallIndirect` (Phase 6b ABI). The body
        // of `lower_method_call` looks for a registered method
        // first; this branch fires before so a same-named
        // method (rare but legal) doesn't shadow the field.
        if let Some(field_call) = self.try_lower_field_closure_call(obj, method, args)? {
            return Ok(field_call);
        }
        if let Some(result) = self.try_lower_str_concat_call(obj, method, args)? {
            return Ok(result);
        }
        if let Some(result) = self.try_lower_primitive_method_call(obj, method, args)? {
            return Ok(result);
        }

        let binding = self.resolve_method_receiver_binding(obj)?;

        // A5-P2: dyn-trait dispatch. When the receiver is bound as
        // `Binding::DynTraitObj`, look the method up by its index
        // in the trait's declaration order, load the fn pointer
        // from `vtable_ptr + idx*8`, and emit `CallIndirect`. The
        // call signature mirrors the trait method's declared
        // params/return, with the implicit `self` slot replaced by
        // `data_ptr` (passed but ignored for MVP-A empty structs).
        if let Binding::DynTraitObj { trait_sym, data_ptr_local, vtable_ptr_local } = &binding {
            return self.lower_dyn_method_call(
                *trait_sym,
                *data_ptr_local,
                *vtable_ptr_local,
                method,
                args,
            );
        }

        // CONCRETE-IMPL Phase 2b: extract receiver's IR type args
        // for spec-aware dispatch.
        let (target_sym, recv_type_args): (DefaultSymbol, Vec<crate::ir::Type>) = match &binding {
            Binding::Struct { struct_id, .. } => {
                let def = self.module.struct_def(*struct_id);
                (def.base_name, def.type_args.clone())
            }
            Binding::Enum(storage) => {
                let def = self.module.enum_def(storage.enum_id);
                (def.base_name, def.type_args.clone())
            }
            _ => {
                return Err(
                    "compiler MVP requires the method receiver to be a struct or enum binding".to_string()
                );
            }
        };
        // MEMORY-ACCESS M5: the stdlib `Ptr<T>`'s element access is the
        // read or write itself, not a call to it. Decided before the
        // target resolves, so the method is not instantiated either.
        if let Some(v) =
            self.lower_ptr_access_intrinsic(&binding, target_sym, method, &recv_type_args, args)?
        {
            return Ok(v);
        }
        // CONCRETE-IMPL Phase 2c: unified cross-registry dispatch —
        // exact concrete spec first, then the generic template
        // (instantiated against the receiver), then a lone concrete
        // spec. A receiver the concrete impls don't exactly match
        // belongs to the generic impl, not to whichever concrete spec
        // happens to be alone in its registry.
        let target = match super::method_registry::resolve_method_target(
            self.method_func_ids,
            self.generic_methods,
            target_sym,
            method,
            &recv_type_args,
        ) {
            Some(super::method_registry::ResolvedMethodTarget::Concrete(id)) => id,
            Some(super::method_registry::ResolvedMethodTarget::Template(template)) => {
                match &binding {
                    Binding::Struct { struct_id, .. } => self.instantiate_generic_method_with_args(
                        target_sym,
                        method,
                        &template,
                        *struct_id,
                        args,
                    )?,
                    Binding::Enum(storage) => {
                        // Enum receiver dispatch: pull the receiver's
                        // resolved `type_args` from `enum_def` and feed
                        // them to the type-args-aware monomorph
                        // instantiator. `Type::Enum(enum_id)` is the
                        // Self type for the impl body.
                        let enum_id = storage.enum_id;
                        let recv_type_args = self.module.enum_def(enum_id).type_args.clone();
                        self.instantiate_generic_method_with_self_type(
                            target_sym,
                            method,
                            &template,
                            Type::Enum(enum_id),
                            recv_type_args,
                            args,
                        )?
                    }
                    _ => {
                        return Err(format!(
                            "compiler MVP: generic method `{}::{}` requires a struct or enum receiver",
                            self.interner.resolve(target_sym).unwrap_or("?"),
                            self.interner.resolve(method).unwrap_or("?"),
                        ));
                    }
                }
            }
            None => {
                return Err(format!(
                    "no method `{}::{}` is defined",
                    self.interner.resolve(target_sym).unwrap_or("?"),
                    self.interner.resolve(method).unwrap_or("?"),
                ));
            }
        };
        let _ = self.method_registry; // referenced for documentation

        let ret_ty = self.module.function(target).return_type;
        if matches!(ret_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
            return Err(format!(
                "compiler MVP cannot use a compound-returning method (`{}::{}`) in expression position; bind the result with `val`",
                self.interner.resolve(target_sym).unwrap_or("?"),
                self.interner.resolve(method).unwrap_or("?"),
            ));
        }
        let (values, recv_reloads) = self.build_method_call_values(&binding, args, target)?;
        // Stage 1 of `&` references: if the method is `&mut self`,
        // emit `CallWithSelfWriteback` so the cranelift call's
        // trailing self-leaf return values are stored back into
        // the receiver binding's leaf locals — propagating the
        // mutation to the caller. Restricted to struct receivers
        // because writeback for enum receivers needs additional
        // tag/payload-slot plumbing (deferred). Falls through to
        // a regular `Call` for `self: Self` / `&self` methods.
        // CONCRETE-IMPL Phase 2b: pick the matching template spec by
        // receiver type args (same priority as `lookup_method_func`).
        let template_self_is_mut = super::method_registry::lookup_method_template(
            self.method_registry, target_sym, method, &[],
        )
            .map(|m| m.self_is_mut && m.has_self_param)
            .unwrap_or(false);
        // REF-Stage-2 (ii-method): the callee may declare writeback
        // returns from `&mut self` AND/OR compound `&mut T` arg
        // params. Pull the receiver-leaf dests when the method is
        // `&mut self`, then append any compound-`&mut T` arg dests.
        // The combined order must match the callee's
        // `self_writeback_types` (which is built body-time as
        // receiver leaves first, then args in declaration order).
        let needs_writeback = !self.module.function(target).self_writeback_types.is_empty();
        if needs_writeback {
            let mut self_dests: Vec<crate::ir::LocalId> = Vec::new();
            // CODE-SIZE-SELF-ABI: a pointer-passed receiver contributes
            // no writeback slots -- the callee wrote through the
            // pointer -- so its leaves must not appear here either.
            let recv_is_ptr = self.module.function(target).ptr_self().is_some();
            if template_self_is_mut && !recv_is_ptr {
                match &binding {
                    Binding::Struct { fields, .. } => {
                        for (l, _) in flatten_struct_locals(fields) {
                            self_dests.push(l);
                        }
                    }
                    Binding::Enum(storage) => {
                        Self::flatten_enum_dests_into(storage, &mut self_dests);
                    }
                    _ => {}
                }
            }
            self_dests.extend(self.collect_compound_writeback_dests_for(args, Some(target), 1)?);
            // Sanity: caller dest count must match callee writeback type count.
            let expected = self.module.function(target).self_writeback_types.len();
            if self_dests.len() == expected {
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
                        args: values,
                        ret_dest,
                        ret_ty: ret_ty_opt,
                        self_dests,
                    },
                    None,
                );
                for r in recv_reloads {
                    r.apply(self);
                }
                let result = match (ret_dest, ret_ty_opt) {
                    (Some(local), Some(ty)) => Some(
                        self.emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value"),
                    ),
                    _ => None,
                };
                return Ok(result);
            }
            // Fall through to plain Call when the dest count is
            // wrong — surfaces via the codegen mismatch error
            // (rare; means we missed an arg shape).
        }
        let inst = InstKind::Call { target, args: values };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        let result = self.emit(inst, result_ty);
        for r in recv_reloads {
            r.apply(self);
        }
        Ok(result)
    }

    /// STR-INTERP-AOT: built-in str methods that don't have a
    /// user-visible `impl` block. Today the type checker registers
    /// `concat` directly through `BuiltinMethod::StrConcat` (see
    /// `frontend/src/type_checker/builtin.rs`), so the Step D
    /// extension-trait lookup below would miss them. Intercept
    /// before that path and emit a direct call to the runtime
    /// helper. Restricted to str receivers; numeric BuiltinMethods
    /// (StrLen on str / I64Abs etc.) either go through
    /// `__builtin_str_len(s)` already or were migrated to extension-
    /// trait impls in the prelude (Step F).
    ///
    /// Returns `Some(result)` when the method was handled here,
    /// `None` to let the dispatcher fall through.
    fn try_lower_str_concat_call(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(Type::Str) = self.value_scalar(obj) else {
            return Ok(None);
        };
        let method_name = self.interner.resolve(method).unwrap_or("");
        if method_name != "concat" {
            return Ok(None);
        }
        if args.len() != 1 {
            return Err(format!(
                "str.concat takes 1 argument, got {}",
                args.len()
            ));
        }
        let recv_v = self
            .lower_expr(obj)?
            .ok_or_else(|| "str.concat receiver produced no value".to_string())?;
        let arg_v = self
            .lower_expr(&args[0])?
            .ok_or_else(|| "str.concat argument produced no value".to_string())?;
        Ok(Some(self.emit(
            InstKind::StrConcat { a: recv_v, b: arg_v },
            Some(Type::Str),
        )))
    }

    /// Step D + F: extension-trait dispatch on a primitive receiver.
    /// Run *before* the bare-identifier check so chained primitive
    /// method calls (`x.abs().abs()`) — whose receiver is itself a
    /// `MethodCall`, not an `Expr::Identifier` — also lower correctly.
    /// We can use `value_scalar` to discover the receiver's IR type
    /// without committing to lowering it twice; a hit then requires
    /// another `lower_expr` pass to actually emit the value (cheap
    /// because most receivers are simple).
    ///
    /// Returns `Some(result)` when the dispatch resolved, `None`
    /// otherwise.
    fn try_lower_primitive_method_call(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<Option<ValueId>>, String> {
        let Some(recv_ty) = self.value_scalar(obj) else {
            return Ok(None);
        };
        let Some(target_sym) = primitive_target_sym_for_ir_type(recv_ty, self.interner) else {
            return Ok(None);
        };
        // Primitive receiver: empty type args (no `impl Foo for u8<...>`).
        let Some(func_id) = super::method_registry::lookup_method_func(
            self.method_func_ids, target_sym, method, &[],
        ) else {
            return Ok(None);
        };
        let receiver_value = self
            .lower_expr(obj)?
            .ok_or_else(|| "primitive method receiver produced no value".to_string())?;
        let mut values: Vec<ValueId> = vec![receiver_value];
        for (arg_idx, a) in args.iter().enumerate() {
            // `T` -> `&T` auto-borrow, as at every other call. This
            // path did not have it, so `3u64.lt(5u64)` for
            // `fn lt(&self, other: &Self)` handed the *value* 5 to a
            // parameter the callee then read with `LoadRef` -- a read
            // of address 5, which answers 0 rather than crashing, so
            // the comparison quietly said false. The receiver always
            // occupies slot 0, hence `1 + arg_idx`.
            if let Some(ptr) = self.lower_scalar_ref_arg(
                a,
                self.module
                    .function(func_id)
                    .param_ref_pointee
                    .get(1 + arg_idx)
                    .copied()
                    .flatten(),
            )? {
                values.push(ptr);
                continue;
            }
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| {
                    "primitive method argument produced no value".to_string()
                })?;
            values.push(v);
        }
        let ret_ty = self.module.function(func_id).return_type;
        if matches!(ret_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
            return Err(format!(
                "compiler MVP cannot use a compound-returning method (`{}::{}`) in expression position; bind the result with `val`",
                self.interner.resolve(target_sym).unwrap_or("?"),
                self.interner.resolve(method).unwrap_or("?"),
            ));
        }
        let inst = InstKind::Call { target: func_id, args: values };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        Ok(Some(self.emit(inst, result_ty)))
    }

    /// Receiver shapes accepted by the compiler MVP method
    /// dispatcher:
    ///   - bare identifier (`a.foo()`): look up the binding directly.
    ///   - field access chain (`self.vec.size()`): resolve via
    ///     `resolve_field_chain` and synthesise a `Binding::Struct`
    ///     from the resulting `FieldChainResult` so the rest of the
    ///     dispatch / arg-loading code can run unchanged. This keeps
    ///     the `String` → `Vec` boundary clean — `String` methods
    ///     can call `self.vec.size()` instead of reading
    ///     `self.vec.len` directly.
    fn resolve_method_receiver_binding(&mut self, obj: &ExprRef) -> Result<Binding, String> {
        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "method-call receiver missing".to_string())?;
        match obj_expr {
            Expr::Identifier(sym) => self
                .bindings
                .get(&sym)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "undefined receiver `{}` for method call",
                        self.interner.resolve(sym).unwrap_or("?")
                    )
                }),
            Expr::FieldAccess(_, _) => {
                let chain = self.resolve_field_chain(obj)?;
                match chain {
                    super::bindings::FieldChainResult::Struct { struct_id, fields } => {
                        Ok(Binding::Struct { struct_id, fields })
                    }
                    _ => Err(format!(
                        "compiler MVP requires nested-field method receivers to resolve to a struct (got {})",
                        super::bindings::field_chain_result_name(&chain)
                    )),
                }
            }
            _ => Err(format!(
                "compiler MVP only supports method calls on a bare identifier or a field-access chain (got {})",
                crate::spelling::describe_expr(self.interner, &obj_expr)
            )),
        }
    }

    /// Build the call args: receiver leaf scalars first, then method
    /// args (per-arg expansion for struct/tuple/enum identifier args
    /// mirrors `lower_call_args`).
    /// CODE-SIZE-SELF-ABI: produce the address a pointer-passed
    /// receiver is handed as.
    ///
    /// Two cases, and the difference between them is the whole point
    /// of the change:
    ///
    /// * **Forwarding.** If these leaves *are* the current function's
    ///   own pointer-passed receiver, they already live in the
    ///   caller's memory; passing the pointer on costs one load. This
    ///   is what makes a chain of `self` methods stop re-pushing the
    ///   struct at every hop.
    /// * **Materialising.** Otherwise the receiver lives in leaf
    ///   locals and needs somewhere addressable: a per-call slot,
    ///   written once before the call. `finish_receiver_slot` reads
    ///   the leaves back afterwards, which is what replaces the
    ///   writeback returns this receiver no longer has.
    pub(super) fn receiver_address(
        &mut self,
        leaves: &[(LocalId, Type)],
    ) -> Result<(ValueId, ReceiverReload), String> {
        // Any of this function's own pointer-passed parameters -- the
        // receiver or a `&T` / `&mut T` -- is already an address into
        // the caller's storage, so handing it on costs one load. This
        // is what stops a chain of calls rebuilding the struct at
        // every hop.
        let forwarded = self
            .module
            .function(self.func_id)
            .ptr_params
            .iter()
            .find(|ps| {
                ps.leaves.len() == leaves.len()
                    && ps
                        .leaves
                        .iter()
                        .zip(leaves.iter())
                        .all(|((a, _, _), (b, _))| a == b)
            })
            .cloned();
        // CODE-SIZE-SELF-ABI S3b: a binding that already lives in a
        // slot hands over that slot's address. This is the root of the
        // chain -- without it every call from the function that *owns*
        // the struct copies it out and reads it back.
        let leaf_ids: Vec<LocalId> = leaves.iter().map(|(l, _)| *l).collect();
        if let Some(r) = self
            .module
            .function(self.func_id)
            .resident_for(&leaf_ids)
            .cloned()
        {
            let addr = self
                .emit(
                    InstKind::DynCoerceSlotAddr { slot_idx: r.slot_idx },
                    Some(Type::U64),
                )
                .expect("DynCoerceSlotAddr returns a value");
            return Ok((addr, ReceiverReload::none()));
        }
        if let Some(ps) = forwarded
            && let Some(ptr_local) = ps.ptr_local
        {
            let v = self
                .emit(InstKind::LoadLocal(ptr_local), Some(Type::U64))
                .expect("LoadLocal returns a value");
            // Forwarded: caller and callee share the bytes, so there
            // is nothing to read back.
            return Ok((v, ReceiverReload::none()));
        }
        let (addr, layout) = self.materialise_receiver_slot(leaves)?;
        Ok((addr, ReceiverReload::slot(addr, layout)))
    }

    /// CODE-SIZE-SELF-ABI: a compound *temporary* -- a literal or a
    /// call's result, already lowered to its leaf values -- for a
    /// parameter the callee takes by address. The leaves go into a
    /// fresh slot laid out as `ty`, and its address is the argument.
    /// Nothing is read back: no binding holds the temporary, so a
    /// write through `&mut` has nowhere to be seen.
    pub(super) fn temporary_address(&mut self, values: &[ValueId], ty: Type) -> Result<ValueId, String> {
        let layout = crate::program::struct_leaf_layout(self.module, ty).ok_or_else(|| {
            format!(
                "CODE-SIZE-SELF-ABI: a temporary of type {} has no leaf layout",
                crate::spelling::spell_type(self.module, self.interner, ty)
            )
        })?;
        if layout.len() != values.len() {
            return Err(format!(
                "internal error (CODE-SIZE-SELF-ABI): a temporary has {} leaf value(s) but its \
                 type lays out {}",
                values.len(),
                layout.len()
            ));
        }
        let size = layout
            .iter()
            .map(|(off, t)| off + crate::program::scalar_byte_size(*t).unwrap_or(8))
            .max()
            .unwrap_or(1);
        let slot_idx = {
            let func = self.module.function_mut(self.func_id);
            let idx = func.dyn_coerce_slots.len() as u32;
            func.dyn_coerce_slots.push(size.max(1) as u32);
            idx
        };
        let addr = self
            .emit(InstKind::DynCoerceSlotAddr { slot_idx }, Some(Type::U64))
            .expect("DynCoerceSlotAddr returns a value");
        for ((off, leaf_ty), value) in layout.iter().zip(values) {
            let off_v = self
                .emit(InstKind::Const(Const::U64(*off)), Some(Type::U64))
                .expect("Const returns a value");
            self.emit(
                InstKind::PtrWrite { ptr: addr, offset: off_v, value: *value, value_ty: *leaf_ty },
                None,
            );
        }
        Ok(addr)
    }

    /// The address a pointer parameter at `slot` of `target` takes for
    /// a temporary's `values`, or the values themselves when that
    /// parameter travels as its leaves.
    pub(super) fn temporary_arg(
        &mut self,
        target: FuncId,
        slot: usize,
        values: Vec<ValueId>,
    ) -> Result<Vec<ValueId>, String> {
        if self.module.function(target).ptr_param(slot).is_none() {
            return Ok(values);
        }
        let ty = self.module.function(target).params[slot];
        Ok(vec![self.temporary_address(&values, ty)?])
    }

    /// MEMORY-ACCESS M5: `p.get(i)` / `p.set(i, v)` (and `p[i]` /
    /// `p[i] = v`) on the stdlib `Ptr<T>`, lowered to the `PtrRead` /
    /// `PtrWrite` its body performs instead of a call to it.
    ///
    /// The compiled lanes have no inliner, so a collection written over
    /// `Ptr<T>` paid a call per element access -- measured at twice the
    /// time of the raw builtin in an element loop. That cost was what
    /// kept `Vec` / `String` / `Dict` on raw `ptr` and their methods
    /// `unsafe`. The arithmetic is the body's, `i * sizeof::<T>()`
    /// bytes from `addr`, so the answer cannot differ from the call's.
    ///
    /// Only the stdlib's `Ptr` qualifies (the method's template was
    /// written in `std.ptr`), only for a scalar `T` (a compound one
    /// expands per leaf, which needs a destination binding -- it keeps
    /// the call), and only when the receiver is the one-leaf struct the
    /// stdlib declares. `Ok(None)` means "not this intrinsic".
    pub(super) fn lower_ptr_access_intrinsic(
        &mut self,
        binding: &Binding,
        target_sym: DefaultSymbol,
        method: DefaultSymbol,
        recv_type_args: &[Type],
        args: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        #[derive(Clone, Copy)]
        enum Access {
            Get,
            Set,
        }
        if self.interner.resolve(target_sym) != Some("Ptr") {
            return Ok(None);
        }
        let access = match self.interner.resolve(method) {
            Some("get" | "__getitem__") => Access::Get,
            Some("set" | "__setitem__") => Access::Set,
            _ => return Ok(None),
        };
        let from_std_ptr = self
            .generic_methods
            .get(&(target_sym, method))
            .is_some_and(|specs| {
                !specs.is_empty()
                    && specs.iter().all(|spec| {
                        spec.method.module_path.as_deref().is_some_and(|path| {
                            let names: Vec<&str> =
                                path.iter().filter_map(|s| self.interner.resolve(*s)).collect();
                            names == ["std", "ptr"]
                        })
                    })
            });
        if !from_std_ptr {
            return Ok(None);
        }
        let Binding::Struct { fields, .. } = binding else {
            return Ok(None);
        };
        let leaves = flatten_struct_locals(fields);
        let [(addr_local, Type::U64)] = leaves.as_slice() else {
            return Ok(None);
        };
        // `Ptr<T>` reads and writes `T`: the receiver's one type argument.
        let [elem_ty] = recv_type_args else {
            return Ok(None);
        };
        let elem_ty = *elem_ty;
        let Some(size) = elem_ty.scalar_byte_size() else {
            return Ok(None);
        };
        let expected_args = match access {
            Access::Get => 1,
            Access::Set => 2,
        };
        if args.len() != expected_args {
            return Ok(None);
        }
        let addr = self
            .emit(InstKind::LoadLocal(*addr_local), Some(Type::U64))
            .expect("LoadLocal returns a value");
        let index = self
            .lower_expr(&args[0])?
            .ok_or_else(|| "Ptr index produced no value".to_string())?;
        let size_v = self
            .emit(InstKind::Const(Const::U64(size)), Some(Type::U64))
            .expect("Const returns a value");
        let offset = self
            .emit(
                InstKind::BinOp { op: crate::ir::BinOp::Mul, lhs: index, rhs: size_v },
                Some(Type::U64),
            )
            .expect("imul returns a value");
        Ok(Some(match access {
            Access::Get => self.emit(
                InstKind::PtrRead { ptr: addr, offset, elem_ty },
                Some(elem_ty),
            ),
            Access::Set => {
                let value = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "Ptr value produced no value".to_string())?;
                self.emit(
                    InstKind::PtrWrite { ptr: addr, offset, value, value_ty: elem_ty },
                    None,
                );
                None
            }
        }))
    }

    /// Write the receiver's leaves into a fresh per-call slot and
    /// answer its address plus the layout needed to read them back.
    fn materialise_receiver_slot(
        &mut self,
        leaves: &[(LocalId, Type)],
    ) -> Result<(ValueId, Vec<(LocalId, u64, Type)>), String> {
        let mut layout: Vec<(LocalId, u64, Type)> = Vec::with_capacity(leaves.len());
        let mut offset: u64 = 0;
        for (local, ty) in leaves {
            let size = crate::program::scalar_byte_size(*ty).ok_or_else(|| {
                format!(
                    "CODE-SIZE-SELF-ABI: receiver leaf of type {} has no byte size",
                    crate::spelling::spell_type(self.module, self.interner, *ty)
                )
            })?;
            layout.push((*local, offset, *ty));
            offset += size;
        }
        let slot_idx = {
            let func = self.module.function_mut(self.func_id);
            let idx = func.dyn_coerce_slots.len() as u32;
            func.dyn_coerce_slots.push(offset.max(1) as u32);
            idx
        };
        let addr = self
            .emit(InstKind::DynCoerceSlotAddr { slot_idx }, Some(Type::U64))
            .expect("DynCoerceSlotAddr returns a value");
        for (local, off, ty) in &layout {
            let val = self
                .emit(InstKind::LoadLocal(*local), Some(*ty))
                .expect("LoadLocal returns a value");
            let off_v = self
                .emit(InstKind::Const(Const::U64(*off)), Some(Type::U64))
                .expect("Const returns a value");
            self.emit(
                InstKind::PtrWrite {
                    ptr: addr,
                    offset: off_v,
                    value: val,
                    value_ty: *ty,
                },
                None,
            );
        }
        Ok((addr, layout))
    }

    /// Read a materialised receiver's leaves back out of its slot,
    /// after the call that may have mutated them.
    pub(super) fn reload_receiver_slot(
        &mut self,
        addr: ValueId,
        layout: &[(LocalId, u64, Type)],
    ) {
        for (local, off, ty) in layout {
            let off_v = self
                .emit(InstKind::Const(Const::U64(*off)), Some(Type::U64))
                .expect("Const returns a value");
            let v = self
                .emit(
                    InstKind::PtrRead { ptr: addr, offset: off_v, elem_ty: *ty },
                    Some(*ty),
                )
                .expect("PtrRead returns a value");
            self.emit(InstKind::StoreLocal { dst: *local, src: v }, None);
        }
    }

    fn build_method_call_values(
        &mut self,
        binding: &Binding,
        args: &Vec<ExprRef>,
        target: crate::ir::FuncId,
    ) -> Result<(Vec<ValueId>, Vec<ReceiverReload>), String> {
        let mut values: Vec<ValueId> = Vec::new();
        let mut reload = ReceiverReload::none();
        let mut arg_reloads: Vec<ReceiverReload> = Vec::new();
        // The callee's declared parameter types, receiver leaves
        // included — a compound literal argument follows the one at
        // its own slot to pick the right monomorphisation. The
        // offset is worked out below, once the receiver's leaves are
        // on `values`.
        let param_tys: Vec<Type> = self.module.function(target).params.clone();
        match binding {
            Binding::Struct { fields, .. } => {
                let leaves = flatten_struct_locals(fields);
                if self.module.function(target).ptr_self().is_some() {
                    // CODE-SIZE-SELF-ABI: the callee wants one address.
                    let (addr, r) = self.receiver_address(&leaves)?;
                    reload = r;
                    values.push(addr);
                } else {
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                }
            }
            Binding::Enum(storage) => {
                let storage = storage.clone();
                let vs = self.load_enum_locals(&storage);
                values.extend(vs);
            }
            _ => unreachable!("receiver shape already validated"),
        }
        for (arg_idx, a) in args.iter().enumerate() {
            // REF-Stage-2: a scalar borrow travels as an address —
            // the same four shapes a function call's arguments take
            // (`borrow_arg_address`). Compound borrows fall through
            // to the identifier expansion below.
            if let Some(v) = self.borrow_arg_address(a)? {
                values.push(v);
                continue;
            }
            // Peel any explicit borrow so compound borrows
            // (`&p` / `&mut p` of a struct/tuple/enum binding) flow
            // through the identifier-expansion path below.
            let arg_expr_ref = match self.program.expression.get(a) {
                Some(Expr::Unary(frontend::ast::UnaryOp::Borrow | frontend::ast::UnaryOp::BorrowMut, inner)) => {
                    inner
                }
                _ => *a,
            };
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(&arg_expr_ref) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
                    // CODE-SIZE-SELF-ABI: the receiver sits at param 0,
                    // so this argument fills slot `1 + arg_idx`.
                    if self
                        .module
                        .function(target)
                        .ptr_param(1 + arg_idx)
                        .is_some()
                    {
                        let (addr, r) = self.receiver_address(&leaves)?;
                        arg_reloads.push(r);
                        values.push(addr);
                        continue;
                    }
                    for (local, ty) in leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    let vs = self.load_enum_locals(&storage);
                    values.extend(vs);
                    continue;
                }
            }
            // REF-Stage-2 (iv): `T` -> `&T` auto-borrow, the same as
            // at a free-function call. This was missing here
            // entirely, so `a.plus(y)` for `fn plus(&self, other: &i64)`
            // passed the value into a pointer slot — 20 instead of 42
            // once the callee dereferenced it, or a segfault when the
            // value did not happen to name mapped memory. The slot is
            // `1 + arg_idx` for the same reason the compound-literal
            // lookup below uses it: the receiver always occupies the
            // first parameter.
            if let Some(ptr) = self.lower_scalar_ref_arg(
                &arg_expr_ref,
                self.module
                    .function(target)
                    .param_ref_pointee
                    .get(1 + arg_idx)
                    .copied()
                    .flatten(),
            )? {
                values.push(ptr);
                continue;
            }
            // CALL-ARG-COMPOUND-LITERAL: `g.shifted(Point { .. })`.
            // `params` carries one entry per declared parameter, and
            // the receiver is always the first of them (prepended for
            // an implicit `&self`, written out for `self: Self`), so
            // this argument's slot is `1 + arg_idx`. A slot that does
            // not match the literal is ignored rather than trusted —
            // see `lower_compound_literal_arg`.
            let param_ty = param_tys.get(1 + arg_idx).copied();
            if let Some(leaves) = self.lower_compound_literal_arg(param_ty, &arg_expr_ref)? {
                let leaves = self.temporary_arg(target, 1 + arg_idx, leaves)?;
                values.extend(leaves);
                continue;
            }
            let v = self
                .lower_expr(&arg_expr_ref)?
                .ok_or_else(|| "method argument produced no value".to_string())?;
            values.push(v);
        }
        arg_reloads.insert(0, reload);
        Ok((values, arg_reloads))
    }

    /// A5-P2-MVP-A: vtable-dispatched method call on a `&dyn Trait`
    /// receiver. Loads the function pointer at slot `method_idx * 8`
    /// of the vtable held in `vtable_ptr_local`, then emits a
    /// `CallIndirect` against the trait method's declared signature.
    /// MVP-A restricts impls to empty structs, so the underlying
    /// method has zero `self` leaves — the indirect call does NOT
    /// pass `data_ptr` as an extra argument (the vtable entries
    /// point directly at the impl methods, not at thunks). MVP-B
    /// will add thunk generation and pass `data_ptr` so non-empty
    /// receivers' field state survives the dispatch.
    pub(super) fn lower_dyn_method_call(
        &mut self,
        trait_sym: DefaultSymbol,
        data_ptr_local: LocalId,
        vtable_ptr_local: LocalId,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        // Resolve the method's index in trait declaration order.
        let method_idx = self
            .module
            .trait_method_order
            .get(&trait_sym)
            .and_then(|order| order.iter().position(|m| *m == method))
            .ok_or_else(|| {
                format!(
                    "A5-P2: trait `{}` has no method `{}` registered in order map",
                    crate::spelling::name(self.interner, trait_sym),
                    crate::spelling::name(self.interner, method)
                )
            })?;

        // Walk the AST to recover the trait method's declared
        // signature. The dispatched thunk's IR signature is
        // `(U64 data_ptr, ...user_arg_tys) -> ret_ty`; the user-arg
        // list comes from the trait declaration (skipping the
        // implicit `self`), return is mapped through
        // `lower_param_or_return_type`. The dispatch site has to
        // prepend `data_ptr` to both the param_tys and the args
        // arrays so the cranelift signature lines up with the
        // thunk's pre-declared one (see
        // `compiler/src/lower/program.rs` thunk pre-declare loop).
        let mut method_param_decls: Option<Vec<frontend::type_decl::TypeDecl>> = None;
        let mut method_ret_decl: Option<frontend::type_decl::TypeDecl> = None;
        for i in 0..self.program.statement.len() {
            let stmt_ref = frontend::ast::StmtRef(i as u32);
            if let Some(frontend::ast::Stmt::TraitDecl { name, methods, .. }) =
                self.program.statement.get(&stmt_ref)
                && name == trait_sym {
                    for sig in &methods {
                        if sig.name == method {
                            method_param_decls = Some(
                                sig.parameter
                                    .iter()
                                    .map(|(_, t)| t.clone())
                                    .collect(),
                            );
                            method_ret_decl = Some(
                                sig.return_type
                                    .clone()
                                    .unwrap_or(frontend::type_decl::TypeDecl::Unit),
                            );
                            break;
                        }
                    }
                    break;
                }
        }
        let method_param_decls = method_param_decls.ok_or_else(|| {
            format!(
                "A5-P2: TraitDecl for `{}` does not contain method `{}`",
                crate::spelling::name(self.interner, trait_sym),
                crate::spelling::name(self.interner, method)
            )
        })?;
        let method_ret_decl =
            method_ret_decl.unwrap_or(frontend::type_decl::TypeDecl::Unit);

        // Lower trait method param/return types to IR Types. The
        // first slot of the dispatched thunk signature is always
        // `data_ptr: U64`; user args follow. Skip the trait
        // declaration's `self: Self` entry — it's absorbed into
        // the thunk's data_ptr + leaf-read path. (MVP-B uniform ABI.)
        let mut ir_param_tys: Vec<crate::ir::Type> = Vec::with_capacity(args.len() + 1);
        ir_param_tys.push(crate::ir::Type::U64); // data_ptr
        for (i, pty) in method_param_decls.iter().enumerate() {
            // Skip a leading `self`-typed entry: the trait signature
            // always lists `self: Self` as its first parameter, and
            // the type-checker has already verified the call uses
            // the trait's own receiver. The thunk handles the
            // leaf-read; dispatch just forwards `data_ptr`.
            if i == 0 && matches!(pty, frontend::type_decl::TypeDecl::Self_) {
                continue;
            }
            let lowered = super::templates::lower_param_or_return_type(
                pty,
                self.struct_defs,
                self.enum_defs,
                self.module,
                self.interner,
            )
            .ok_or_else(|| {
                format!(
                    "A5-P2: cannot lower trait method param type `{}`",
                    crate::spelling::spell_type_decl(self.interner, pty)
                )
            })?;
            ir_param_tys.push(lowered);
        }
        let ir_ret_ty = super::templates::lower_param_or_return_type(
            &method_ret_decl,
            self.struct_defs,
            self.enum_defs,
            self.module,
            self.interner,
        )
        .ok_or_else(|| {
            format!(
                "A5-P2: cannot lower trait method return type `{}`",
                crate::spelling::spell_type_decl(self.interner, &method_ret_decl)
            )
        })?;

        // Load vtable_ptr from the binding's local.
        let vtable_ptr_val = self
            .emit(InstKind::LoadLocal(vtable_ptr_local), Some(crate::ir::Type::U64))
            .expect("LoadLocal returns a value");
        // Compute offset = method_idx * 8 bytes.
        let offset_val = self
            .emit(
                InstKind::Const(Const::U64((method_idx * 8) as u64)),
                Some(crate::ir::Type::U64),
            )
            .expect("Const returns a value");
        // Load fn_ptr = *(vtable_ptr + offset) as U64.
        let fn_ptr_val = self
            .emit(
                InstKind::PtrRead {
                    ptr: vtable_ptr_val,
                    offset: offset_val,
                    elem_ty: crate::ir::Type::U64,
                },
                Some(crate::ir::Type::U64),
            )
            .expect("PtrRead returns a value");

        // Lower the user-written args + prepend `data_ptr` so the
        // thunk receives `(data_ptr, ...user_args)`. MVP-B doesn't
        // support compound argument types through dyn dispatch yet;
        // scalar args go through the regular value path.
        let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len() + 1);
        let data_ptr_val = self
            .emit(InstKind::LoadLocal(data_ptr_local), Some(crate::ir::Type::U64))
            .expect("LoadLocal(data_ptr) returns a value");
        arg_values.push(data_ptr_val);
        for a in args {
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "dyn method arg produced no value".to_string())?;
            arg_values.push(v);
        }

        // A5-P2-MVP-D: when the trait method returns a struct, fan
        // the multi-result indirect call into pre-allocated field
        // locals via `CallIndirectFnStruct`. The caller observes the
        // result through `pending_struct_value` — the same channel
        // the regular `CallStruct` path uses — so `val p = m.build()`
        // and similar bindings just work via the existing
        // `lower_let_call_struct` consumer.
        if let crate::ir::Type::Struct(struct_id) = ir_ret_ty {
            let field_bindings = self.allocate_struct_fields(struct_id);
            let dests: Vec<crate::ir::LocalId> =
                super::bindings::flatten_struct_locals(&field_bindings)
                    .into_iter()
                    .map(|(local, _ty)| local)
                    .collect();
            self.emit(
                InstKind::CallIndirectFnStruct {
                    callee: fn_ptr_val,
                    args: arg_values,
                    param_tys: ir_param_tys,
                    ret_struct_id: struct_id,
                    dests,
                },
                None,
            );
            self.pending_struct_value = Some(field_bindings);
            return Ok(None);
        }
        // A5-P2-MVP-E: tuple return — `CallIndirectFnTuple` +
        // `pending_tuple_value`. Mirrors the struct arm above; the
        // tuple's element layout drives both dest allocation and
        // the cranelift signature shape codegen emits.
        if let crate::ir::Type::Tuple(tuple_id) = ir_ret_ty {
            let element_bindings = self.allocate_tuple_elements(tuple_id)?;
            let dests: Vec<crate::ir::LocalId> =
                super::bindings::flatten_tuple_element_locals(&element_bindings)
                    .into_iter()
                    .map(|(local, _ty)| local)
                    .collect();
            self.emit(
                InstKind::CallIndirectFnTuple {
                    callee: fn_ptr_val,
                    args: arg_values,
                    param_tys: ir_param_tys,
                    ret_tuple_id: tuple_id,
                    dests,
                },
                None,
            );
            self.pending_tuple_value = Some(element_bindings);
            return Ok(None);
        }
        // A5-P2-MVP-E: enum return — `CallIndirectFnEnum` +
        // `pending_enum_value`. `flatten_enum_dests` already
        // produces the canonical `[tag, variant payloads ...]`
        // order the codegen multi-result walk expects.
        if let crate::ir::Type::Enum(enum_id) = ir_ret_ty {
            let storage = self.allocate_enum_storage(enum_id);
            let dests = Self::flatten_enum_dests(&storage);
            self.emit(
                InstKind::CallIndirectFnEnum {
                    callee: fn_ptr_val,
                    args: arg_values,
                    param_tys: ir_param_tys,
                    ret_enum_id: enum_id,
                    dests,
                },
                None,
            );
            self.pending_enum_value = Some(storage);
            return Ok(None);
        }
        let result_ty = if matches!(ir_ret_ty, crate::ir::Type::Unit) {
            None
        } else {
            Some(ir_ret_ty)
        };
        Ok(self.emit(
            InstKind::CallIndirectFn {
                callee: fn_ptr_val,
                args: arg_values,
                param_tys: ir_param_tys,
                ret_ty: ir_ret_ty,
            },
            result_ty,
        ))
    }

    /// Everything a compound-returning method call needs *except* the
    /// destination: the target, its return type, the flattened
    /// argument values, and the writeback slots the callee appends to
    /// its return shape.
    ///
    /// Split out because the destination differs by caller — a `val`
    /// rhs allocates a fresh binding, a struct field or enum payload
    /// already has leaf locals waiting — while the resolution, the
    /// receiver-leaf flatten and the writeback bookkeeping are the
    /// same work either way.
    ///
    /// `Ok(None)` means "not this shape" (the receiver isn't a struct
    /// or enum binding, or the method returns a scalar), and the
    /// caller should keep looking; nothing has been emitted.
    pub(super) fn prepare_compound_method_call(
        &mut self,
        recv: &ExprRef,
        method_sym: DefaultSymbol,
        method_args: &[ExprRef],
    ) -> Result<Option<CompoundMethodCall>, String> {
        let Some((target, recv_binding)) =
            self.resolve_method_target(recv, method_sym, method_args)?
        else {
            return Ok(None);
        };
        let ret = self.module.function(target).return_type;
        if !matches!(ret, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
            return Ok(None);
        }
        // Call args: receiver leaf scalars first, then the method
        // arguments (each lowered individually so identifier-arg
        // expansion for struct / tuple / enum stays intact).
        let mut args: Vec<ValueId> = Vec::new();
        let mut reload = ReceiverReload::none();
        match &recv_binding {
            Binding::Struct { fields, .. } => {
                let leaves = flatten_struct_locals(fields);
                if self.module.function(target).ptr_self().is_some() {
                    // CODE-SIZE-SELF-ABI: same as the scalar-returning
                    // sibling -- one address instead of every leaf.
                    let (addr, r) = self.receiver_address(&leaves)?;
                    reload = r;
                    args.push(addr);
                } else {
                    for (local, ty) in leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns");
                        args.push(v);
                    }
                }
            }
            Binding::Enum(storage) => {
                let storage = storage.clone();
                let vs = self.load_enum_locals(&storage);
                args.extend(vs);
            }
            _ => unreachable!("resolve_method_target only returns struct/enum receivers"),
        }
        for (arg_idx, a) in method_args.iter().enumerate() {
            // Mirror `lower_method_call`'s identifier-arg flatten path
            // so a struct / tuple / enum argument (auto-borrowed or
            // not) decomposes into leaf locals before landing in the
            // cranelift call ABI. Without this, a `concat(other:
            // &Vec<u8>)` / similar signature would bail with "method
            // argument produced no value" — `lower_expr` on a struct
            // identifier intentionally returns `Ok(None)` (the value
            // is held in the binding's leaf locals, not in the IR
            // value graph).
            let arg_expr_ref = match self.program.expression.get(a) {
                Some(Expr::Unary(
                    frontend::ast::UnaryOp::Borrow | frontend::ast::UnaryOp::BorrowMut,
                    inner,
                )) => inner,
                _ => *a,
            };
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(&arg_expr_ref) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
                    // CODE-SIZE-SELF-ABI: the same check the
                    // scalar-returning sibling makes -- a parameter the
                    // callee takes by address gets one. Missing here,
                    // `val v = s.split(&sep)` spread `sep` into leaves
                    // the callee's signature no longer had room for.
                    if self.module.function(target).ptr_param(1 + arg_idx).is_some() {
                        let (addr, r) = self.receiver_address(&leaves)?;
                        reload.merge(r);
                        args.push(addr);
                        continue;
                    }
                    for (local, ty) in leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        args.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        args.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    let vs = self.load_enum_locals(&storage);
                    args.extend(vs);
                    continue;
                }
            }
            // CALL-ARG-COMPOUND-LITERAL / ENUM-VARIANT-ARG: a compound
            // written straight into an argument of a *compound-returning*
            // method (`val b = z.apply(Op::Add(3u64))`). The scalar-returning
            // sibling in `build_method_call_values` already had this;
            // here the literal reached `lower_expr` and produced no
            // value. The receiver occupies the first declared param,
            // so this argument's slot is `1 + arg_idx`.
            // `T` -> `&T` auto-borrow, as on the scalar-returning
            // sibling. Compound-returning methods reached this loop
            // without it.
            if let Some(ptr) = self.lower_scalar_ref_arg(
                &arg_expr_ref,
                self.module
                    .function(target)
                    .param_ref_pointee
                    .get(1 + arg_idx)
                    .copied()
                    .flatten(),
            )? {
                args.push(ptr);
                continue;
            }
            let param_ty = self
                .module
                .function(target)
                .params
                .get(1 + arg_idx)
                .copied();
            if let Some(leaves) = self.lower_compound_literal_arg(param_ty, &arg_expr_ref)? {
                let leaves = self.temporary_arg(target, 1 + arg_idx, leaves)?;
                args.extend(leaves);
                continue;
            }
            let v = self
                .lower_expr(&arg_expr_ref)?
                .ok_or_else(|| "method argument produced no value".to_string())?;
            args.push(v);
        }
        // ITER-PROTOCOL-AOT: when the callee is `&mut self` (or has
        // compound `&mut T` arg writeback declared), its cranelift
        // signature carries extra return values for the writeback
        // leaves. Order matches `populate_method_writeback_types`:
        // receiver leaves first, then args in declaration order.
        let mut writeback_dests: Vec<LocalId> = Vec::new();
        if !self.module.function(target).self_writeback_types.is_empty() {
            // CODE-SIZE-SELF-ABI: a pointer-passed receiver has no
            // writeback slots to fill.
            let recv_is_ptr = self.module.function(target).ptr_self().is_some();
            match &recv_binding {
                Binding::Struct { fields, .. } if !recv_is_ptr => writeback_dests.extend(
                    flatten_struct_locals(fields).into_iter().map(|(l, _)| l),
                ),
                Binding::Struct { .. } => {}
                Binding::Enum(storage) => {
                    Self::flatten_enum_dests_into(storage, &mut writeback_dests)
                }
                _ => {}
            }
            writeback_dests
                .extend(self.collect_compound_writeback_dests_for(method_args, Some(target), 1)?);
        }
        Ok(Some(CompoundMethodCall {
            target,
            ret,
            args,
            writeback_dests,
            reload,
        }))
    }
}

/// A compound-returning method call resolved down to "emit this into
/// the slots you pick". Built by
/// [`FunctionLower::prepare_compound_method_call`].
pub(super) struct CompoundMethodCall {
    pub target: FuncId,
    /// The callee's return type — decides `CallStruct` / `CallTuple` /
    /// `CallEnum`.
    pub ret: Type,
    /// Receiver leaves followed by the method's arguments.
    pub args: Vec<ValueId>,
    /// Slots that receive `&mut` writeback, appended *after* the
    /// caller's own destination locals.
    pub writeback_dests: Vec<LocalId>,
    /// CODE-SIZE-SELF-ABI: the caller emits the call, so the caller
    /// owes the receiver its read-back.
    pub reload: ReceiverReload,
}

#[cfg(test)]
mod primitive_target_tests {
    use super::*;
    use crate::types::lower_scalar;

    /// NUM-W-ENUMERATION: every primitive the IR can represent as a
    /// scalar is dispatchable as a method receiver.
    ///
    /// This is the projection that failed before: registration knew the
    /// narrow widths and dispatch did not, so `impl <Trait> for u8`
    /// lowered and was then unreachable. Deriving the names from
    /// `TypeDecl::PRIMITIVE_IMPL_TARGETS` does not by itself stop a
    /// width being dropped in the `Type` match above, so that is what
    /// this asserts.
    #[test]
    fn every_ir_representable_primitive_dispatches() {
        let mut interner = DefaultStringInterner::new();
        for (_, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            interner.get_or_intern(*name);
        }

        for (decl, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            let Some(ir_ty) = lower_scalar(decl) else {
                continue;
            };
            let got = primitive_target_sym_for_ir_type(ir_ty, &interner);
            if *name == "ptr" {
                // `ptr` lowers to `Type::U64`, indistinguishable from
                // `u64`, so it resolves to `u64` rather than to itself.
                // Documented in the function; asserted so the day `ptr`
                // gains its own IR type, this test says so.
                assert_eq!(got, interner.get("u64"), "ptr no longer aliases u64 in IR");
                continue;
            }
            // DIAG-DEBUG-FMT-OK: a test assertion, and the IR type is
            // exactly what the failure needs to name.
            assert_eq!(
                got,
                interner.get(*name),
                "`{name}` lowers to {ir_ty:?} but does not dispatch as a receiver"
            );
        }
    }

    /// A program with no extension trait on a primitive never interned
    /// its name, and dispatch short-circuits rather than inventing one.
    #[test]
    fn an_uninterned_primitive_name_resolves_to_nothing() {
        let interner = DefaultStringInterner::new();
        assert_eq!(primitive_target_sym_for_ir_type(Type::U8, &interner), None);
    }

    /// Compound IR types have no primitive receiver path.
    #[test]
    fn a_compound_ir_type_is_not_a_primitive_receiver() {
        let mut interner = DefaultStringInterner::new();
        for (_, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
            interner.get_or_intern(*name);
        }
        assert_eq!(primitive_target_sym_for_ir_type(Type::Unit, &interner), None);
        assert_eq!(
            primitive_target_sym_for_ir_type(Type::Struct(crate::ir::StructId(0)), &interner),
            None
        );
    }
}
