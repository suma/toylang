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
use crate::ir::{FuncId, InstKind, Linkage, StructId, Type, ValueId};

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
fn primitive_target_sym_for_ir_type(
    ty: Type,
    interner: &DefaultStringInterner,
) -> Option<DefaultSymbol> {
    let name = match ty {
        Type::Bool => "bool",
        Type::I64 => "i64",
        Type::U64 => "u64",
        Type::F64 => "f64",
        // `Type::Str` is a pointer-sized opaque handle in IR
        // (Phase T). Extension-trait dispatch (`s.hash()` from
        // `core/std/hash.t`'s `impl Hash for str`) routes through
        // the same per-target method registry as the numeric
        // primitives above — `lower_program` uses the matching
        // `"str" => TypeDecl::String` entry in
        // `primitive_type_decl_for_target_sym`.
        Type::Str => "str",
        // `ptr` has no extension trait in stdlib yet; wire when
        // exercised.
        _ => return None,
    };
    interner.get(name)
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
            // boundary lowerer once everything is concrete.
            _ => self.lower_type_with_subst(ty, subst),
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
            // Method param[0] is `self`; call args[i] corresponds to
            // method param[i+1]. Walk each pair, looking for
            // `Generic(P)` slots that match a method-only param.
            for (i, arg_ref) in arg_refs.iter().enumerate() {
                let param_idx = i + 1;
                let declared = match template.parameter.get(param_idx) {
                    Some((_, t)) => t.clone(),
                    None => continue,
                };
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
        let mut params: Vec<Type> = Vec::with_capacity(template.parameter.len());
        for (pname, pty) in &template.parameter {
            let lowered = self
                .lower_method_param_type(pty, &subst, self_type)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower generic method param `{}: {:?}` after subst",
                        self.interner.resolve(*pname).unwrap_or("?"),
                        pty
                    )
                })?;
            params.push(lowered);
        }
        let ret = match &template.return_type {
            Some(ty) => self
                .lower_method_param_type(ty, &subst, self_type)
                .ok_or_else(|| {
                    format!(
                        "compiler MVP cannot lower generic method return type `{:?}` after subst",
                        ty
                    )
                })?,
            None => Type::Unit,
        };
        let target_str = self.interner.resolve(target_sym).unwrap_or("?");
        let method_str = self.interner.resolve(method_sym).unwrap_or("?");
        let arg_str = inst_args
            .iter()
            .map(|t| format!("{:?}", t))
            .collect::<Vec<_>>()
            .join("_");
        let export_name = format!("toy_{}__{}__{}", target_str, method_str, arg_str);
        let func_id = self
            .module
            .declare_function_anon(export_name, Linkage::Local, params, ret);
        self.method_instances
            .insert((target_sym, method_sym, inst_args), func_id);
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
        let recv_sym = match obj_expr {
            Expr::Identifier(s) => s,
            _ => return Ok(None),
        };
        let binding = match self.bindings.get(&recv_sym).cloned() {
            Some(b) => b,
            None => return Ok(None),
        };
        let target_sym = match &binding {
            Binding::Struct { struct_id, .. } => self.module.struct_def(*struct_id).base_name,
            Binding::Enum(storage) => self.module.enum_def(storage.enum_id).base_name,
            _ => return Ok(None),
        };
        if let Some(id) = self.method_func_ids.get(&(target_sym, method)).copied() {
            return Ok(Some((id, binding)));
        }
        if let Some(template) = self.generic_methods.get(&(target_sym, method)).cloned() {
            let recv_struct_id = match &binding {
                Binding::Struct { struct_id, .. } => *struct_id,
                _ => return Ok(None),
            };
            let id = self.instantiate_generic_method_with_args(
                target_sym,
                method,
                &template,
                recv_struct_id,
                args,
            )?;
            return Ok(Some((id, binding)));
        }
        Ok(None)
    }

    /// Lower an `obj.method(args)` expression. Phase R1 dispatch is
    /// purely static: we resolve the receiver's struct (or enum)
    /// symbol, look up the method via the registry built in
    /// `lower_program`, and emit a regular `Call` with the
    /// receiver's leaf scalars prepended to the call's arg values.
    pub(super) fn lower_method_call(
        &mut self,
        obj: &ExprRef,
        method: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        // Step D + F: extension-trait dispatch on a primitive
        // receiver. Run *before* the bare-identifier check so
        // chained primitive method calls (`x.abs().abs()`) — whose
        // receiver is itself a `MethodCall`, not an
        // `Expr::Identifier` — also lower correctly. We can use
        // `value_scalar` to discover the receiver's IR type
        // without committing to lowering it twice; a hit then
        // requires another `lower_expr` pass to actually emit the
        // value (cheap because most receivers are simple).
        if let Some(recv_ty) = self.value_scalar(obj) {
            if let Some(target_sym) =
                primitive_target_sym_for_ir_type(recv_ty, self.interner)
            {
                if let Some(func_id) = self.method_func_ids.get(&(target_sym, method)).copied()
                {
                    let receiver_value = self
                        .lower_expr(obj)?
                        .ok_or_else(|| "primitive method receiver produced no value".to_string())?;
                    let mut values: Vec<ValueId> = vec![receiver_value];
                    for a in args {
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
                    return Ok(self.emit(inst, result_ty));
                }
            }
        }

        let obj_expr = self
            .program
            .expression
            .get(obj)
            .ok_or_else(|| "method-call receiver missing".to_string())?;
        let recv_sym = match obj_expr {
            Expr::Identifier(sym) => sym,
            _ => {
                return Err(format!(
                    "compiler MVP only supports method calls on a bare identifier (got {:?})",
                    obj_expr
                ));
            }
        };
        let binding = self
            .bindings
            .get(&recv_sym)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "undefined receiver `{}` for method call",
                    self.interner.resolve(recv_sym).unwrap_or("?")
                )
            })?;

        let target_sym = match &binding {
            Binding::Struct { struct_id, .. } => self.module.struct_def(*struct_id).base_name,
            Binding::Enum(storage) => self.module.enum_def(storage.enum_id).base_name,
            _ => {
                return Err(format!(
                    "compiler MVP requires the method receiver `{}` to be a struct or enum binding",
                    self.interner.resolve(recv_sym).unwrap_or("?")
                ));
            }
        };
        let target = if let Some(id) = self.method_func_ids.get(&(target_sym, method)).copied()
        {
            id
        } else if let Some(template) = self.generic_methods.get(&(target_sym, method)).cloned()
        {
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
        } else {
            return Err(format!(
                "no method `{}::{}` is defined",
                self.interner.resolve(target_sym).unwrap_or("?"),
                self.interner.resolve(method).unwrap_or("?"),
            ));
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
        // Build the call args: receiver leaf scalars first, then
        // method args (per-arg expansion for struct/tuple/enum
        // identifier args mirrors `lower_call_args`).
        let mut values: Vec<ValueId> = Vec::new();
        match &binding {
            Binding::Struct { fields, .. } => {
                let leaves = flatten_struct_locals(fields);
                for (local, ty) in &leaves {
                    let v = self
                        .emit(InstKind::LoadLocal(*local), Some(*ty))
                        .expect("LoadLocal returns a value");
                    values.push(v);
                }
            }
            Binding::Enum(storage) => {
                let storage = storage.clone();
                let vs = self.load_enum_locals(&storage);
                values.extend(vs);
            }
            _ => unreachable!("receiver shape already validated"),
        }
        for a in args {
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(a) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    for (local, ty) in flatten_struct_locals(&fields) {
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
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "method argument produced no value".to_string())?;
            values.push(v);
        }
        let inst = InstKind::Call { target, args: values };
        let result_ty = if ret_ty.produces_value() {
            Some(ret_ty)
        } else {
            None
        };
        Ok(self.emit(inst, result_ty))
    }
}
