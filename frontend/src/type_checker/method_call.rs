//! Method-call type checking — instance methods (`x.foo()`) and associated
//! functions (`Struct::new()`).
//!
//! Pulled out of `expression.rs` to keep that file focused on plain
//! expression visitors. Methods here remain inherent on
//! `TypeCheckerVisitor`; `visitor_impl.rs` still routes the trait method
//! calls into them via thin wrappers.

use string_interner::{DefaultSymbol, Symbol};
use std::collections::HashMap;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};
use crate::type_checker::generics::GenericTypeChecking;
use crate::type_checker::method::MethodProcessing;

impl<'a> TypeCheckerVisitor<'a> {
    /// Walk a declared `TypeDecl` against the actual `arg_ty`, populating
    /// `out` with `Generic(P) -> ConcreteType` mappings whenever a generic
    /// param `P` (one of `params`) appears in `declared`. Recurses through
    /// `Struct(_, args)` / `Enum(_, args)` / `Tuple(_)` so nested generic
    /// positions resolve too. Skips conflicting bindings — the caller is
    /// trusted to only feed compatible (declared, arg) pairs.
    ///
    /// Uses raw symbol *values* (`u32`) for matching so that parser-level
    /// `string_interner` inconsistencies (where `resolve()` returns the
    /// wrong string for a symbol) are harmless. The `params` set holds the
    /// numeric `to_usize()` values of the generic-param symbols taken from
    /// `method_func.generic_params`.
    fn collect_substitution(
        &self,
        declared: &TypeDecl,
        arg_ty: &TypeDecl,
        param_values: &std::collections::HashSet<u32>,
        out: &mut HashMap<DefaultSymbol, TypeDecl>,
    ) {
        match declared {
            TypeDecl::Generic(p) | TypeDecl::Identifier(p)
                if param_values.contains(&(p.to_usize() as u32)) => {
                    out.entry(*p).or_insert_with(|| arg_ty.clone());
                }
            TypeDecl::Struct(_, decl_args) | TypeDecl::Enum(_, decl_args) => {
                let arg_args = match arg_ty {
                    TypeDecl::Struct(_, a) | TypeDecl::Enum(_, a) => a.clone(),
                    _ => return,
                };
                for (d, a) in decl_args.iter().zip(arg_args.iter()) {
                    self.collect_substitution(d, a, param_values, out);
                }
            }
            TypeDecl::Tuple(decl_elems) => {
                if let TypeDecl::Tuple(arg_elems) = arg_ty {
                    for (d, a) in decl_elems.iter().zip(arg_elems.iter()) {
                        self.collect_substitution(d, a, param_values, out);
                    }
                }
            }
            TypeDecl::Function(decl_params, decl_ret) => {
                if let TypeDecl::Function(arg_params, arg_ret) = arg_ty {
                    for (d, a) in decl_params.iter().zip(arg_params.iter()) {
                        self.collect_substitution(d, a, param_values, out);
                    }
                    self.collect_substitution(decl_ret, arg_ret, param_values, out);
                }
            }
            _ => {}
        }
    }

    /// Type check method calls - implementation used by type_checker.rs
    pub fn visit_method_call_impl(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let method_name = self.resolve_symbol_name(*method);
        
        
        let obj_type = self.visit_expr(obj)?;
        
        
        // Check if obj is a variable and get its name for type mapping lookup
        let _var_name = if let Some(obj_expr) = self.core.expr_pool.get(obj) {
            match obj_expr {
                Expr::Identifier(name) => Some(name),
                _ => None
            }
        } else {
            None
        };
        
        // The obj_type should already contain the concrete type parameters
        // No need to look up mappings, just use the type as-is.
        // Refinement: when the parser's `Struct(name, args)` annotation
        // names a type that the type-checker has registered as an
        // *enum*, refine it to `Enum(name, args)` so the enum-method
        // dispatch arm in `visit_method_call_on_type` can match. The
        // parser can't tell enums from structs at parse time, so this
        // post-decl refinement is needed for `val o: Option<u64>` to
        // route through the enum path.
        // REF-Stage-2: peel `&T` before structural refinement so a
        // method call on a `&String` receiver dispatches through
        // `String`'s impl table the same as a `String` receiver
        // would. Auto-deref is implicit for method receivers.
        let obj_type_deref = obj_type.deref_ref().clone();
        let resolved_obj_type = match &obj_type_deref {
            TypeDecl::Struct(name, args)
                if self.context.enum_definitions.contains_key(name) =>
            {
                TypeDecl::Enum(*name, args.clone())
            }
            TypeDecl::Identifier(name)
                if self.context.enum_definitions.contains_key(name) =>
            {
                TypeDecl::Enum(*name, vec![])
            }
            // Symmetric refinement for structs: a `val s: String`
            // annotation parses as `Identifier(String)` because the
            // parser doesn't know whether the bare name is a struct
            // or an alias. If `String` is a registered struct, lift
            // it to `Struct(String, [])` so the struct-method
            // dispatch arm at line ~248 matches.
            TypeDecl::Identifier(name)
                if self.context.struct_definitions.contains_key(name) =>
            {
                TypeDecl::Struct(*name, vec![])
            }
            _ => obj_type_deref.clone(),
        };
        
        // Type check arguments.
        //
        // NUMBER-HINT: a method's declared parameter types are the
        // hint for suffix-less literal arguments, the same way a free
        // function's are (`expression.rs::check_call_arguments`).
        // Without this a literal argument defaulted to `u64` however
        // the parameter was declared, so `s.push_char('a')` — a `u32`
        // parameter — reached the compiled backends as an i64 and the
        // cranelift verifier rejected the call.
        let declared_params = self.declared_method_param_types(&resolved_obj_type, method, args.len());
        let saved_hint = self.type_inference.type_hint.clone();
        let mut arg_types = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let expected = declared_params.as_ref().and_then(|p| p.get(i).cloned().flatten());
            self.type_inference.type_hint = expected.clone().or_else(|| saved_hint.clone());
            let arg_ty = match self.visit_expr(arg) {
                Ok(t) => t,
                Err(e) => {
                    self.type_inference.type_hint = saved_hint;
                    return Err(e);
                }
            };
            let arg_ty = match expected {
                Some(expected) => match self.coerce_number_expr(arg, &arg_ty, &expected) {
                    Ok(t) => t,
                    Err(e) => {
                        self.type_inference.type_hint = saved_hint;
                        return Err(e);
                    }
                },
                None => arg_ty,
            };
            arg_types.push(arg_ty);
        }
        self.type_inference.type_hint = saved_hint;
        
        // Check for builtin methods
        let method_str = self.resolve_symbol_name(*method);
        // STDLIB-TEXT §3: these used to be accepted on `str` and work
        // on the tree-walker only. Rather than let them fail as "no
        // such method", say why `str` cannot have them and where the
        // owned version lives.
        if matches!(resolved_obj_type, TypeDecl::String)
            && let Some(owned) = str_needs_owned_buffer(&method_str)
        {
            return Err(self.error_with_location(
                TypeCheckError::generic_error(&format!(
                    "`str` has no method `{method_str}`: it borrows its bytes, so it has no buffer to write a new string into. `String::from_str(s).{owned}` produces an owned String"
                )),
                obj,
            ));
        }
        let builtin_method = self.builtin_methods.get(&(resolved_obj_type.clone(), method_str.to_string())).cloned();
        if let Some(builtin_method) = builtin_method {
            // visit_builtin_method_call expects ExprRef, not TypeDecl
            return self.visit_builtin_method_call(obj, &builtin_method, args);
        }
        
        // Check struct methods
        // Note: Current struct definition does not include methods
        // Method support will be added in a future refactoring
        
        // Check other type methods
        let result = self.visit_method_call_on_type(&resolved_obj_type, method, args, &arg_types);
        
        // A `Generic(P)` result is only a bug when `P` is not a type
        // parameter that is actually in scope.
        //
        // Two scopes count. A method inside a generic impl returns an
        // impl-level param (`self.f(v)` on `MapIter<T, U>` returns
        // `U` while `next`'s body is checked against the template) --
        // and so does a **generic free function**: `fn dup<T: Clone>(v:
        // &T) -> T { v.clone() }` resolves `Self` to `T`, which is
        // exactly right and was being rejected because only the impl
        // scope was consulted (STDLIB-TRAIT-BASE B1).
        //
        // That single omission is why `Clone`, `Default` and every
        // arithmetic trait were unwritable in a generic context, and
        // why `Ord` was the one that worked: `lt` returns `bool`, so
        // it never produced a `Generic` to reject.
        if let Ok(TypeDecl::Generic(sym)) = &result {
            let in_impl_scope = self
                .context
                .current_impl_generic_params
                .as_ref()
                .map(|p| p.contains(sym))
                .unwrap_or(false);
            // Bounds alone are not the scope: `fn shuffle<T>(v: &mut
            // Vec<T>)` declares `T` and constrains it with nothing, so
            // it never reaches the bounds map. Both lists have to be
            // consulted or an unbounded parameter reads as undeclared.
            let in_fn_scope = self.context.current_fn_generic_bounds.contains_key(sym)
                || self.context.current_fn_generic_params.contains(sym);
            if !in_impl_scope && !in_fn_scope {
                let sym_str = self.resolve_symbol_name(*sym);
                return Err(TypeCheckError::generic_error(&format!(
                    "method '{}' on `{}` returns the type parameter `{}`, which is not                      bound here — name it in the enclosing function's or impl's parameter list",
                    method_name, self.type_name_for_error(&resolved_obj_type), sym_str
                )));
            }
        }
        
        result
    }

    /// From/Into: rewrite `expr.into()` (the MethodCall node at
    /// `call_ref`, whose receiver is `obj_ref`) to
    /// `Target::from(expr)` when the expected type `Target` implements
    /// `From<source>`. Returns `true` when the rewrite happened.
    ///
    /// The target type is taken from the type hint — the surrounding
    /// annotation (`val s: String = x.into()`) or argument-expected
    /// type. Without a hint the call has no target and is left for the
    /// ordinary method dispatch to reject (Rust requires the same
    /// annotation: `let x = s.into();` is ambiguous).
    ///
    /// Only the `From` side is ever written as an impl
    /// (`core/std/convert.t` explains why); `Into` is the blanket
    /// derivation here, so `struct_implements_trait(target, From)`
    /// plus a matching `From<source>` entry is the whole check.
    pub(super) fn rewrite_into_call(
        &mut self,
        call_ref: ExprRef,
        obj_ref: ExprRef,
    ) -> bool {
        // Expected target type from the surrounding context.
        let target = match self.type_inference.get_type_hint() {
            Some(ty) => ty,
            None => return false,
        };
        // The parser emits `Struct(String, [])` for a bare struct name
        // in an annotation, but `Identifier(String)` for a named alias;
        // normalise both to a concrete carrier symbol.
        let target_sym = match &target {
            TypeDecl::Struct(sym, _) => *sym,
            TypeDecl::Identifier(sym) => *sym,
            _ => return false,
        };
        // Source type: the receiver's own type, with `&T` peeled.
        let Ok(source_ty) = self.visit_expr(&obj_ref) else {
            return false;
        };
        let source_ty = source_ty.deref_ref().clone();
        // The target must implement `From<source>` with matching args
        // (`From<str>` for `str -> String`, not `From<u64>`).
        if !self.type_implements_from(&target, &source_ty) {
            return false;
        }
        let from_method = match self.from_method_symbol() {
            Some(sym) => sym,
            None => return false,
        };
        // Rewrite `expr.into()` -> `Target::from(expr)` in place.
        // ERROR_MODEL E1: with several `From` impls on the target, the
        // source type names which one.
        let from_method = self.resolve_trait_overload(target_sym, from_method, &source_ty);
        self.core.expr_pool.update(
            &call_ref,
            Expr::AssociatedFunctionCall(target_sym, from_method, vec![obj_ref]),
        );
        true
    }

    /// Helper method to handle method calls on a specific type
    /// The declared parameter types of `obj_type::method`, aligned to
    /// the *call's* argument list.
    ///
    /// Only concrete types are reported: a `Generic(P)` / `Self_`
    /// parameter is left as `None` for that position, since binding it
    /// is the job of the substitution collection further down and a
    /// literal must not be narrowed to a type parameter's name.
    /// `None` overall when the receiver is not a struct / enum whose
    /// method table can be consulted here (builtin methods, trait
    /// objects, closures in fields) — those keep the previous
    /// hint-free behaviour.
    fn declared_method_param_types(
        &self,
        obj_type: &TypeDecl,
        method: &DefaultSymbol,
        arg_count: usize,
    ) -> Option<Vec<Option<TypeDecl>>> {
        let (name, type_args) = match obj_type {
            TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) => (*name, args.clone()),
            _ => return None,
        };
        let method_func = self
            .context
            .get_struct_method(name, *method, &type_args)
            .or_else(|| self.context.get_struct_method(name, *method, &[]))?;
        // CHAR-LITERAL-GENERIC-ARG: a receiver with concrete type
        // arguments has already decided what its impl-level parameters
        // are, so `fn set(&mut self, i: u64, v: T)` on a `Span<u8>`
        // declares a `u8` here — not a `T` with nothing to say.
        //
        // Without the substitution the slot looked generic, the hint
        // was dropped, and a char literal argument stayed the `u32` it
        // is held as. `s.set(0u64, 'A')` then reached cranelift as an
        // i32 against an i8 parameter: `arg 3 (v48) has type i32,
        // expected i8` — a verifier crash rather than a diagnostic.
        //
        // A parameter that is still generic after this is one the
        // *method* introduced (`fn map<U>`), and that one really does
        // name nothing; it keeps being filtered out below.
        let mut subst: std::collections::HashMap<DefaultSymbol, TypeDecl> =
            std::collections::HashMap::new();
        if !type_args.is_empty() {
            let decl_params = self
                .context
                .get_struct_generic_params(name)
                .cloned()
                .or_else(|| self.context.enum_generic_params.get(&name).cloned());
            if let Some(decl_params) = decl_params {
                for (decl, concrete) in decl_params.iter().zip(type_args.iter()) {
                    subst.insert(*decl, concrete.clone());
                }
            }
        }
        // A by-value `self: Self` receiver occupies parameter slot 0;
        // `&self` / `&mut self` receivers are kept out of the list.
        let offset = if method_func.parameter.len() > arg_count { 1 } else { 0 };
        Some(
            (0..arg_count)
                .map(|i| {
                    method_func
                        .parameter
                        .get(i + offset)
                        .map(|(_, ty)| {
                            if subst.is_empty() {
                                ty.clone()
                            } else {
                                ty.substitute_generics(&subst)
                            }
                        })
                        .filter(|ty| !matches!(ty, TypeDecl::Generic(_) | TypeDecl::Self_))
                })
                .collect(),
        )
    }

    pub fn visit_method_call_on_type(&mut self, obj_type: &TypeDecl, method: &DefaultSymbol, args: &Vec<ExprRef>, _arg_types: &[TypeDecl]) -> Result<TypeDecl, TypeCheckError> {
        let method_name = self.resolve_symbol_name(*method);

        // Step B of extension-trait support: dispatch primitive
        // receivers through the user-registered `struct_methods`
        // table first. The impl-block type-checker registered each
        // `impl Trait for <PrimitiveType>` method under the canonical
        // primitive symbol (`"i64"`, `"f64"`, …); a method call on a
        // primitive value can therefore look the body up by symbol
        // before falling back to the legacy hardcoded `BuiltinMethod`
        // arms below. `Self` in the return type resolves back to the
        // receiver's primitive `TypeDecl`.
        if let Some(target_sym) = self.primitive_target_symbol_from_type(obj_type)
            && let Some(method_func) =
                self.context.get_struct_method(target_sym, *method, &[]).cloned()
            {
                // Visit the args so each one is type-checked even
                // when the callee's parameters are concrete (no
                // generics to bind on primitives in this iteration).
                for arg_ref in args {
                    let _ = self.visit_expr(arg_ref)?;
                }
                let return_type = method_func
                    .return_type
                    .clone()
                    .unwrap_or(TypeDecl::Unit);
                // SELF-IN-TYPE-ARG: `Self` also appears inside a type
                // argument (`-> Option<Self>`, the shape a checked
                // arithmetic trait wants), which a top-level match
                // leaves unresolved for the caller to trip over.
                let resolved = return_type.substitute_self(obj_type);
                return Ok(resolved);
            }

        // A5 trait-object dispatch: a `dyn Trait` receiver (typically
        // reached after the auto-deref of `&dyn Trait`) resolves the
        // method through the trait's signature table. `Self` in the
        // return type collapses back to the trait-object type so
        // chained calls keep the dynamic shape. Interpreter does the
        // actual dispatch through the regular `method_registry`
        // (every Object is a typed `Rc<RefCell<...>>`); the trait
        // signature lookup here is only the static type contract.
        if let TypeDecl::Dyn(trait_sym) = obj_type
            && let Some(sig) = self.context.get_trait_method(*trait_sym, *method).cloned()
        {
            let ret = sig.return_type.clone().unwrap_or(TypeDecl::Unit);
            let resolved = ret.substitute_self(obj_type);
            return Ok(resolved);
        }

        // Method call on a trait-bounded generic parameter, e.g. inside
        // `fn f<T: MyTrait>(x: T) { x.foo() }`. Resolve `foo` through the
        // trait's method signature table; `Self` in the return type is
        // mapped back to the generic parameter so the caller sees the
        // appropriate concrete type after monomorphization.
        // A2 multi-bound: `<T: A + B>` stores `TraitIntersection([A, B])`.
        // Try each trait in declaration order and take the first whose
        // signature table contains `method` (overlap is allowed but rare;
        // first-hit semantics keep the lookup deterministic).
        // TRAIT-BOUND: a generic-trait bound (`I: Iter<i64>`) parses as
        // `Struct(iter_sym, [i64])`; the trait's own generic params are
        // substituted with those args in the method's return type
        // (`fn next(&mut self) -> Option<T>` resolves to `Option<i64>`).
        //
        // STDLIB-ORD: the receiver may also surface as the bare
        // `Identifier(t_sym)` form — a local annotated `val key: T`
        // inside a bounded impl (`impl<T: Ord> Vec<T>`) resolves `T`
        // to `Identifier` rather than the canonical `Generic`. Both
        // shapes are treated as the bounded generic when `t_sym` is a
        // bound in scope; a bare struct name is never in
        // `current_fn_generic_bounds`, so the guard is precise.
        let generic_sym = match obj_type {
            TypeDecl::Generic(sym) => Some(*sym),
            TypeDecl::Identifier(sym)
                if self.context.current_fn_generic_bounds.contains_key(sym) =>
            {
                Some(*sym)
            }
            _ => None,
        };
        if let Some(t_sym) = generic_sym {
            let trait_bounds: Vec<(DefaultSymbol, Vec<TypeDecl>)> =
                match self.context.current_fn_generic_bounds.get(&t_sym).cloned() {
                    Some(TypeDecl::Identifier(trait_sym)) => vec![(trait_sym, Vec::new())],
                    Some(TypeDecl::Struct(trait_sym, args))
                    | Some(TypeDecl::Enum(trait_sym, args)) => vec![(trait_sym, args)],
                    Some(TypeDecl::TraitIntersection(syms)) => {
                        syms.into_iter().map(|s| (s, Vec::new())).collect()
                    }
                    _ => Vec::new(),
                };
            for (trait_sym, trait_args) in &trait_bounds {
                if let Some(sig) = self.context.get_trait_method(*trait_sym, *method).cloned() {
                    // Substitute the trait's generic params with the
                    // bound's type args (`Iter<i64>`: T -> i64) so the
                    // resolved return type is concrete.
                    let trait_generic_params = self
                        .context
                        .trait_generic_params
                        .get(trait_sym)
                        .cloned()
                        .unwrap_or_default();
                    let mut subst: HashMap<DefaultSymbol, TypeDecl> = HashMap::new();
                    for (p, a) in trait_generic_params.iter().zip(trait_args.iter()) {
                        subst.insert(*p, a.clone());
                    }
                    let ret = sig.return_type.clone().unwrap_or(TypeDecl::Unit);
                    let ret = ret.substitute_generics(&subst);
                    let resolved = match ret {
                        TypeDecl::Self_ => TypeDecl::Generic(t_sym),
                        other => other,
                    };
                    return Ok(resolved);
                }
            }
        }

        // Method call on a generic enum receiver
        // (`val o: Option<i64> = ...; o.unwrap_or(default)`).
        // `impl<T> Option<T> { fn unwrap_or(self: Self, default: T) -> T }`
        // registers under the enum's name symbol, so the same
        // `(target_symbol, method_name)` lookup the struct path uses
        // works here. T is bound from the enum's type_params and
        // substituted into the return type.
        if let TypeDecl::Enum(enum_name, type_params) = obj_type {
            let _method_str = self.core.string_interner.resolve(*method).unwrap_or("?").to_string();
            if let Some(method_func) =
                self.context.get_struct_method(*enum_name, *method, type_params).cloned()
            {
                let generic_params = self
                    .context
                    .enum_generic_params
                    .get(enum_name)
                    .cloned()
                    .unwrap_or_default();
                let mut substitutions: HashMap<DefaultSymbol, TypeDecl> =
                    HashMap::new();
                for (i, generic_param) in generic_params.iter().enumerate() {
                    if let Some(concrete_type) = type_params.get(i) {
                        substitutions.insert(*generic_param, concrete_type.clone());
                    }
                }
                // Method-only generic params: bind from arg types
                // (skip self at index 0).

                if !method_func.generic_params.is_empty() {
                    let param_values: std::collections::HashSet<u32> =
                        method_func.generic_params.iter()
                            .map(|p| p.to_usize() as u32)
                            .collect();
                    for (i, arg_ref) in args.iter().enumerate() {
                        let param_idx = i + 1;
                        if let Some((_, declared_ty)) =
                            method_func.parameter.get(param_idx)
                        {
                            let arg_ty = self.visit_expr(arg_ref)?;
                            self.collect_substitution(
                                declared_ty,
                                &arg_ty,
                                &param_values,
                                &mut substitutions,
                            );
                        }
                    }
                }
                // Same impl-bound enforcement as the generic-struct
                // path below (`impl<T: Ord> MyEnum<T>` must reject a
                // receiver whose payload type has no `Ord` impl).
                let mut bound_subs = substitutions.clone();
                bound_subs.extend(self.impl_param_substitutions(
                    *enum_name,
                    *method,
                    type_params,
                ));
                let method_name_str = self.resolve_symbol_name(*method);
                self.check_generic_bounds(
                    &method_func.generic_params,
                    &method_func.generic_bounds,
                    &bound_subs,
                    "Method",
                    &method_name_str,
                )?;
                // COLLECTIONS C0(a): see the note in `generics.rs` —
                // a `==` inside the method's body is answered by the
                // type arguments this call resolved to.
                self.note_generic_instantiation(crate::type_checker::context::EqInstantiation {
                    owner: crate::type_checker::context::EqOwner::Method(*enum_name, *method),
                    substitutions: bound_subs.iter().map(|(k, v)| (*k, v.clone())).collect(),
                    owner_kind: "Method",
                    owner_name: method_name_str.clone(),
                    location: args.first().and_then(|a| self.get_expr_location(a)),
                });
                let method_return_type = method_func
                    .return_type
                    .clone()
                    .unwrap_or(TypeDecl::Unit);
                let resolved = match method_return_type {
                    TypeDecl::Self_ => {
                        TypeDecl::Enum(*enum_name, type_params.clone())
                    }
                    TypeDecl::Generic(p) => substitutions
                        .get(&p)
                        .cloned()
                        .unwrap_or(TypeDecl::Generic(p)),
                    ref other => other.substitute_generics(&substitutions),
                };
                // SELF-IN-TYPE-ARG: a `Self` nested in a type argument
                // survives the arms above; the `Self_` arm has already
                // produced a concrete type, so this is a no-op there.
                let resolved =
                    resolved.substitute_self(&TypeDecl::Enum(*enum_name, type_params.clone()));
                return Ok(resolved);
            }
        }

        // Check if this is a user-defined method for a struct
        if let TypeDecl::Struct(struct_name, type_params) = obj_type {
            // Check if this is a generic struct with type parameters
            
            if !type_params.is_empty() {
                // Handle generic struct method call
                let method_func_opt = self.context.get_struct_method(*struct_name, *method, type_params).cloned();
                
                
                if let Some(method_func) = method_func_opt {
                    // Create substitution map from generic parameters to concrete types
                    let generic_params = self.context.get_struct_generic_params(*struct_name).cloned();
                    
                    
                    let generic_params = generic_params.unwrap_or_default();
                    let mut substitutions = HashMap::new();
                    for (i, generic_param) in generic_params.iter().enumerate() {
                        if let Some(concrete_type) = type_params.get(i) {
                            substitutions.insert(*generic_param, concrete_type.clone());
                        }
                    }

                    // STDLIB-ITER-ADAPT: bind method-only generic params
                    // (`fn map<U>(&self, f: fn (T) -> U) -> MapIter<T, U>`)
                    // from the call argument types, same as the enum and
                    // non-generic-struct paths below. Without this the
                    // return type keeps `Generic(U)` unresolved and the
                    // caller sees a generic-typed value.
                    // Note: `&self` / `&mut self` receivers are not part
                    // of `method_func.parameter`, so the argument index
                    // maps straight onto the parameter index (unlike the
                    // enum path where `self: Self` occupies slot 0).
                    if !method_func.generic_params.is_empty() {
                        let param_values: std::collections::HashSet<u32> =
                            method_func.generic_params.iter()
                                .map(|p| p.to_usize() as u32)
                                .collect();
                        for (i, arg_ref) in args.iter().enumerate() {
                            if let Some((_, declared_ty)) =
                                method_func.parameter.get(i)
                            {
                                let arg_ty = self.visit_expr(arg_ref)?;
                                self.collect_substitution(
                                    declared_ty,
                                    &arg_ty,
                                    &param_values,
                                    &mut substitutions,
                                );
                            }
                        }
                    }

                    // STDLIB-ORD: enforce the bounds the winning impl
                    // block declared (`impl<T: Ord> Vec<T>` rejects a
                    // `Vec<NonOrd>` receiver here). The parser merges
                    // impl-level bounds into every method it contains,
                    // so `method_func.generic_bounds` already carries
                    // them; only the parameter *names* can differ from
                    // the struct's, which `impl_param_substitutions`
                    // reconciles. Without this the call type-checks and
                    // dispatch fails much later — at run time in the
                    // interpreter, at AOT-compile time natively.
                    let mut bound_subs = substitutions.clone();
                    bound_subs.extend(self.impl_param_substitutions(
                        *struct_name,
                        *method,
                        type_params,
                    ));
                    let method_name_str = self.resolve_symbol_name(*method);
                    self.check_generic_bounds(
                        &method_func.generic_params,
                        &method_func.generic_bounds,
                        &bound_subs,
                        "Method",
                        &method_name_str,
                    )?;
                    // COLLECTIONS C0(a): as above — record what this
                    // call instantiated the method with.
                    self.note_generic_instantiation(crate::type_checker::context::EqInstantiation {
                        owner: crate::type_checker::context::EqOwner::Method(*struct_name, *method),
                        substitutions: bound_subs.iter().map(|(k, v)| (*k, v.clone())).collect(),
                        owner_kind: "Method",
                        owner_name: method_name_str.clone(),
                        location: args.first().and_then(|a| self.get_expr_location(a)),
                    });

                    // Apply substitutions to method return type
                    let method_return_type = method_func.return_type.as_ref().unwrap_or(&TypeDecl::Unit);
                    
                    let resolved_return_type = match method_return_type {
                        TypeDecl::Self_ => TypeDecl::Struct(*struct_name, type_params.clone()),
                        TypeDecl::Generic(param) => {
                            
                            substitutions.get(param).cloned().unwrap_or(TypeDecl::Generic(*param))
                        },
                        other => other.substitute_generics(&substitutions)
                    };
                    // SELF-IN-TYPE-ARG (see the enum path above).
                    let resolved_return_type = resolved_return_type
                        .substitute_self(&TypeDecl::Struct(*struct_name, type_params.clone()));
                    
                    
                    return Ok(resolved_return_type);
                }
                // No matching method on this generic struct: fall
                // through to the field-call / array / builtin arms
                // below instead of failing outright, so a field of
                // fn type (`self.f(v)` on `MapIter`) dispatches
                // through the Closure-Phase-8 fallback.
            } else {
                // Handle non-generic struct method call. Method-only
                // generic params (`fn pick<U>(...)`) need substitution
                // from the actual argument types — pull the method
                // function and infer.
                if let Some(method_func) =
                    self.context.get_struct_method(*struct_name, *method, &[]).cloned()
                {
                    let method_return_type = method_func
                        .return_type
                        .clone()
                        .unwrap_or(TypeDecl::Unit);
                    let mut substitutions: HashMap<DefaultSymbol, TypeDecl> =
                        HashMap::new();
                    if !method_func.generic_params.is_empty() {
                        let param_values: std::collections::HashSet<u32> =
                            method_func.generic_params.iter()
                                .map(|p| p.to_usize() as u32)
                                .collect();
                        // Visit each call argument and bind any
                        // matching `Generic(P)` slot in the method's
                        // declared params to the runtime arg type.
                        // `self` occupies a parameter slot only when
                        // the receiver is by-value (`self: Self`);
                        // `&self` / `&mut self` receivers are kept
                        // out of `method_func.parameter` (same
                        // convention as the generic-struct path).
                        let param_offset =
                            if method_func.parameter.len() > args.len() { 1 } else { 0 };
                        for (i, arg_ref) in args.iter().enumerate() {
                            let param_idx = i + param_offset;
                            if let Some((_, declared_ty)) =
                                method_func.parameter.get(param_idx)
                            {
                                let arg_ty = self.visit_expr(arg_ref)?;
                                self.collect_substitution(
                                    declared_ty,
                                    &arg_ty,
                                    &param_values,
                                    &mut substitutions,
                                );
                            }
                        }
                    }
                    let resolved = match method_return_type {
                        TypeDecl::Self_ => TypeDecl::Struct(*struct_name, vec![]),
                        TypeDecl::Identifier(name)
                            if self.context.struct_definitions.contains_key(&name) =>
                        {
                            TypeDecl::Struct(name, vec![])
                        }
                        // SELF-IN-TYPE-ARG (see the enum path above).
                        ref other if !matches!(other, TypeDecl::Generic(_)) => other
                            .substitute_self(&TypeDecl::Struct(*struct_name, vec![]))
                            .substitute_generics(&substitutions),
                        TypeDecl::Generic(p) => {
                            substitutions.get(&p).cloned().unwrap_or(TypeDecl::Generic(p))
                        }
                        other => other.substitute_generics(&substitutions),
                    };
                    return Ok(resolved);
                }
            }
        }

        // Check array methods
        if let TypeDecl::Array(..) = obj_type
            && method_name == "len" {
                // Array len() returns u64
                return Ok(TypeDecl::UInt64);
            }

        // Check builtin methods
        if let Some(builtin_method) = self.builtin_methods.get(&(obj_type.clone(), method_name.to_string())).cloned() {
            // For builtin methods, we need to create a temporary expression ref for the object
            // This is a bit of a hack but necessary for the current API
            let dummy_obj_ref = ExprRef(0); // Use dummy ref for now
            return self.visit_builtin_method_call(&dummy_obj_ref, &builtin_method, args);
        }

        // Closures Phase 8: fallback to field-call dispatch.
        // When `obj.method(args)` doesn't resolve to any
        // method, look for a field whose name matches and
        // whose declared type is `fn (T1, T2) -> R`. If found,
        // type-check the call against the function type's
        // signature and return its return type. The runtime
        // (interpreter / AOT) reads the field as a closure
        // value and dispatches indirectly — same path it uses
        // when a function-typed local is called.
        if let TypeDecl::Struct(struct_name, type_params) = obj_type
            && let Some(fields) = self.context.get_struct_fields(*struct_name).cloned() {
                let field = fields.iter().find(|f| f.name == method_name);
                if let Some(field) = field {
                    // STDLIB-ITER-ADAPT: a field of fn type inside a
                    // generic struct (`MapIter<T, U>`'s `f: fn (T) -> U`)
                    // mentions the struct's generic params. Substitute
                    // them from the receiver's concrete type args so the
                    // field's function signature — and its return type —
                    // resolve to the impl-level params instead of the
                    // declaration-level symbols.
                    let mut subst: HashMap<DefaultSymbol, TypeDecl> = HashMap::new();
                    if let Some(decl_params) = self.context.get_struct_generic_params(*struct_name) {
                        for (decl, concrete) in decl_params.iter().zip(type_params.iter()) {
                            subst.insert(*decl, concrete.clone());
                        }
                    }
                    let field_ty = field.type_decl.substitute_generics(&subst);
                    if let TypeDecl::Function(param_tys, ret_ty) = field_ty {
                        // Argument count + per-position
                        // compatibility — same shape as
                        // `visit_indirect_call`'s checks.
                        if args.len() != param_tys.len() {
                            return Err(TypeCheckError::generic_error(&format!(
                                "field '{}' on struct '{}' has fn type taking {} args, got {}",
                                method_name,
                                self.resolve_symbol_name(*struct_name),
                                param_tys.len(),
                                args.len()
                            )));
                        }
                        let original_hint = self.type_inference.type_hint.clone();
                        for (idx, (arg_ref, expected)) in args.iter().zip(param_tys.iter()).enumerate() {
                            self.type_inference.type_hint = Some(expected.clone());
                            let arg_ty = self.visit_expr(arg_ref)?;
                            if !self.is_arg_compatible_dyn_aware(&arg_ty, expected)
                                && arg_ty != TypeDecl::Unknown
                            {
                                self.type_inference.type_hint = original_hint;
                                return Err(TypeCheckError::generic_error(&format!(
                                    "Type error: expected {}, found {}. field '{}' on struct '{}' arg {} type mismatch",
                                    self.type_name_for_error(expected),
                                    self.type_name_for_error(&arg_ty),
                                    method_name,
                                    self.resolve_symbol_name(*struct_name),
                                    idx + 1
                                )));
                            }
                        }
                        self.type_inference.type_hint = original_hint;
                        return Ok((*ret_ty).clone());
                    }
                }
            }

        // The universal `is_null()` is deliberately unsupported — the
        // `null` literal has no working semantics in any backend (see
        // `docs/language.md`). Rather than a bare "method not found",
        // point the reader at the supported spellings.
        let reason = if method_name == "is_null" {
            "method not found; the universal is_null() is unsupported — \
             test raw pointers with `__builtin_ptr_is_null(p)` and absent \
             values with `Option<T>::is_none()`"
        } else {
            "method not found"
        };
        Err(TypeCheckError::method_error(&method_name, obj_type.clone(), reason))
    }

    /// Type check associated function calls - implementation
    /// Dispatch a `module::func(args)` qualified call. The qualifier
    /// has already been confirmed to match an imported module alias;
    /// the function lives in the (flat) main function table because
    /// module integration appends imported `pub fn` items there.
    /// Type-check the args against the callee parameter list (mirrors
    /// the non-generic branch of `visit_call`) and return the
    /// callee's declared return type.
    fn dispatch_module_function_call(
        &mut self,
        function_name: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<TypeDecl, TypeCheckError> {
        self.dispatch_module_function_call_with_qualifier(None, function_name, args)
    }

    /// Same as `dispatch_module_function_call` but takes the module
    /// qualifier the call site wrote (`["math"]` for
    /// `math::add(args)`), matched against the tail of each
    /// candidate's module path (MODULE-SYSTEM P2). Two modules whose
    /// last segment collides therefore report as ambiguous instead of
    /// resolving to whichever one was registered last.
    pub(super) fn dispatch_module_function_call_with_qualifier(
        &mut self,
        qualifier: Option<&[DefaultSymbol]>,
        function_name: DefaultSymbol,
        args: &Vec<ExprRef>,
    ) -> Result<TypeDecl, TypeCheckError> {
        let fun = match self.context.lookup_fn_detailed(qualifier, function_name) {
            crate::type_checker::context::FnLookup::Found(f) => f,
            crate::type_checker::context::FnLookup::Missing => {
                return Err(TypeCheckError::not_found(
                    "Function",
                    &self.resolve_symbol_name(function_name),
                ));
            }
            crate::type_checker::context::FnLookup::Ambiguous(paths) => {
                return Err(self.ambiguous_module_function_error(
                    qualifier,
                    function_name,
                    &paths,
                ));
            }
        };
        // Honour visibility (matches the bare-call path).
        self.check_function_access(&fun)?;
        // Generic module functions: synthesize an `ExprList` for the
        // args and reuse the regular generic-call path so the
        // existing inference / monomorphisation logic runs.
        if !fun.generic_params.is_empty() {
            let args_ref = self
                .core
                .expr_pool
                .add(Expr::ExprList(args.clone()));
            // `visit_generic_call` pops a scope on every exit -- the
            // bare-call path in `expression.rs` pushes one before
            // dispatching, and this path has to as well. Without it the
            // pop takes the *caller's* scope, so `random::shuffle(&mut
            // v)` left every later mention of `v` reading as
            // `[E0003] Identifier 'v' not found`.
            self.push_context();
            return self.visit_generic_call(function_name, &args_ref, &fun);
        }
        let params: Vec<_> = fun
            .parameter
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect();
        if args.len() != params.len() {
            return Err(TypeCheckError::generic_error(&format!(
                "module function '{}' expects {} argument(s), found {}",
                self.resolve_symbol_name(function_name),
                params.len(),
                args.len()
            )));
        }
        for (arg_expr, expected_ty) in args.iter().zip(params.iter()) {
            let actual_ty = self.visit_expr(arg_expr)?;
            if !self.is_arg_compatible_dyn_aware(&actual_ty, expected_ty) && !matches!(actual_ty, TypeDecl::Unknown) {
                return Err(TypeCheckError::type_mismatch(
                    expected_ty.clone(),
                    actual_ty,
                ).with_context(&format!(
                    "argument of module function '{}'",
                    self.resolve_symbol_name(function_name)
                )));
            }
        }
        let return_ty = fun.return_type.clone().unwrap_or(TypeDecl::Unit);
        Ok(return_ty)
    }

    /// STDLIB-TRAIT-BASE B5: resolve `T::assoc(args)` against the
    /// trait `T` is bound to, or `None` when `struct_name` is not a
    /// type parameter in scope (which is every ordinary
    /// `Type::assoc()` call).
    fn resolve_type_param_associated_call(
        &mut self,
        struct_name: DefaultSymbol,
        function_name: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<Option<TypeDecl>, TypeCheckError> {
        let Some(bound) = self.context.current_fn_generic_bounds.get(&struct_name).cloned() else {
            return Ok(None);
        };
        let trait_bounds: Vec<(DefaultSymbol, Vec<TypeDecl>)> = match bound {
            TypeDecl::Identifier(t) => vec![(t, Vec::new())],
            TypeDecl::Struct(t, a) | TypeDecl::Enum(t, a) => vec![(t, a)],
            TypeDecl::TraitIntersection(ts) => ts.into_iter().map(|t| (t, Vec::new())).collect(),
            _ => return Ok(None),
        };
        for (trait_sym, trait_args) in &trait_bounds {
            let Some(sig) = self.context.get_trait_method(*trait_sym, function_name).cloned() else {
                continue;
            };
            // Substitute the trait's own generic params with the
            // bound's arguments, exactly as the method path does.
            let trait_generic_params = self
                .context
                .trait_generic_params
                .get(trait_sym)
                .cloned()
                .unwrap_or_default();
            let mut subst: HashMap<DefaultSymbol, TypeDecl> = HashMap::new();
            for (p, a) in trait_generic_params.iter().zip(trait_args.iter()) {
                subst.insert(*p, a.clone());
            }
            // Check the arguments against the trait's declaration.
            // `self` is skipped: an associated call names no receiver.
            let params: Vec<TypeDecl> = sig
                .parameter
                .iter()
                .skip(usize::from(sig.has_self_param))
                .map(|(_, ty)| ty.substitute_generics(&subst))
                .collect();
            if args.len() != params.len() {
                return Err(TypeCheckError::generic_error(&format!(
                    "`{}::{}` expects {} argument(s), found {}",
                    self.resolve_symbol_name(struct_name),
                    self.resolve_symbol_name(function_name),
                    params.len(),
                    args.len(),
                )));
            }
            for (arg, expected) in args.iter().zip(params.iter()) {
                let actual = self.visit_expr(arg)?;
                let expected_here = match expected {
                    TypeDecl::Self_ => TypeDecl::Generic(struct_name),
                    other => other.clone(),
                };
                if !self.is_arg_compatible_dyn_aware(&actual, &expected_here)
                    && !matches!(actual, TypeDecl::Unknown)
                {
                    return Err(TypeCheckError::type_mismatch(expected_here, actual).with_context(
                        &format!(
                            "argument of `{}::{}`",
                            self.resolve_symbol_name(struct_name),
                            self.resolve_symbol_name(function_name),
                        ),
                    ));
                }
            }
            let ret = sig
                .return_type
                .clone()
                .unwrap_or(TypeDecl::Unit)
                .substitute_generics(&subst);
            return Ok(Some(match ret {
                TypeDecl::Self_ => TypeDecl::Generic(struct_name),
                other => other,
            }));
        }
        Ok(None)
    }

    pub fn visit_associated_function_call_impl(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Handle Container::function_name(args) type calls for any associated function

        // STDLIB-TRAIT-BASE B5: `T::assoc(...)` where `T` is a bounded
        // type parameter rather than a named type. The method-call
        // path has had this tier since B1; without the same one here,
        // `fn make<T: Default>() -> T { T::default() }` was reported
        // as `[E0003] Struct 'T' not found` -- and `T` really is not a
        // struct. What it is, is a name the enclosing signature bound
        // to a trait, and with no receiver to read a type off, that
        // trait is the only source of the signature.
        if let Some(ty) = self.resolve_type_param_associated_call(struct_name, function_name, args)? {
            return Ok(ty);
        }

        // Enum tuple-variant construction: `Enum::Variant(args)` syntactically
        // matches `Struct::assoc(args)`. Intercept when the left side is a
        // registered enum and the right side names one of its variants. For
        // generic enums, infer the type parameters from argument types.
        if let Some(variants) = self.context.enum_definitions.get(&struct_name).cloned()
            && let Some(variant_def) = variants.iter().find(|v| v.name == function_name) {
                if args.len() != variant_def.payload_types.len() {
                    let enum_str = self.resolve_symbol_name(struct_name);
                    let v_str = self.resolve_symbol_name(function_name);
                    return Err(TypeCheckError::generic_error(&format!(
                        "variant '{}::{}' expects {} argument(s), found {}",
                        enum_str, v_str, variant_def.payload_types.len(), args.len()
                    )));
                }
                let generic_params = self.context.enum_generic_params.get(&struct_name).cloned().unwrap_or_default();
                let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                // Seed substitutions from the outer type hint so nested
                // variant construction (`Option::Some(Option::None)` with
                // hint `Option<Option<i64>>`) can flow the inner type args
                // down to the payload expression.
                let outer_hint = self.type_inference.type_hint.clone();
                let hint_args: Vec<TypeDecl> = match &outer_hint {
                    Some(TypeDecl::Enum(hint_name, a)) if *hint_name == struct_name => a.clone(),
                    Some(TypeDecl::Struct(hint_name, a)) if *hint_name == struct_name => a.clone(),
                    _ => Vec::new(),
                };
                if hint_args.len() == generic_params.len() {
                    for (param, arg) in generic_params.iter().zip(hint_args.iter()) {
                        substitutions.insert(*param, arg.clone());
                    }
                }
                let saved_hint = outer_hint;
                for (arg_expr, expected_ty) in args.iter().zip(variant_def.payload_types.iter()) {
                    // Push a hint equal to the payload type with current
                    // substitutions applied so inner literals / nested
                    // variants see the concrete expected type.
                    let resolved_hint = expected_ty.substitute_generics(&substitutions);
                    self.type_inference.type_hint = Some(resolved_hint.clone());
                    let actual_ty = self.visit_expr(arg_expr)?;
                    // NUMBER-HINT: the declared payload type names what
                    // an unsuffixed literal in `E::V(5)` should become.
                    // A generic payload has nothing concrete to offer,
                    // so the coercion no-ops and the literal's own
                    // resolution decides `T`.
                    let actual_ty = self.coerce_number_expr(arg_expr, &actual_ty, &resolved_hint)?;
                    // When the declared payload references a generic parameter,
                    // record the argument's concrete type as that parameter.
                    if let TypeDecl::Generic(p) = expected_ty
                        && generic_params.contains(p) {
                            if let Some(prev) = substitutions.get(p) {
                                if !prev.is_equivalent(&actual_ty) {
                                    let enum_str = self.resolve_symbol_name(struct_name);
                                    let v_str = self.resolve_symbol_name(function_name);
                                    return Err(TypeCheckError::generic_error(&format!(
                                        "variant '{}::{}' generic parameter conflict: {} vs {}",
                                        enum_str, v_str,
                                        self.type_name_for_error(prev),
                                        self.type_name_for_error(&actual_ty)
                                    )));
                                }
                            } else {
                                substitutions.insert(*p, actual_ty.clone());
                            }
                            continue;
                        }
                    let expected_resolved = expected_ty.substitute_generics(&substitutions);
                    if !actual_ty.is_equivalent(&expected_resolved) && !matches!(actual_ty, TypeDecl::Unknown) {
                        let enum_str = self.resolve_symbol_name(struct_name);
                        let v_str = self.resolve_symbol_name(function_name);
                        return Err(TypeCheckError::generic_error(&format!(
                            "variant '{}::{}' payload type mismatch: expected {}, found {}",
                            enum_str, v_str,
                            self.type_name_for_error(&expected_resolved),
                            self.type_name_for_error(&actual_ty)
                        )));
                    }
                }
                self.type_inference.type_hint = saved_hint;
                let type_args: Vec<TypeDecl> = generic_params.iter()
                    .map(|p| substitutions.get(p).cloned().unwrap_or(TypeDecl::Generic(*p)))
                    .collect();
                return Ok(TypeDecl::Enum(struct_name, type_args));
            }

        // Module-qualified call: `module::func(args)` where `module`
        // is an imported module alias. The qualifier is matched
        // against the tail of each candidate's module path
        // (MODULE-SYSTEM P2), so `math::add(...)` finds
        // `std.math.add` and a user-defined `fn add(Point, Point)`
        // does not shadow it. The bare-name fallback covers older
        // flows where module integration left the entry unqualified.
        let module_alias = vec![struct_name];
        if self.imported_modules.contains_key(&module_alias) {
            let qualifier = [struct_name];
            let qualified = self
                .context
                .lookup_fn_detailed(Some(&qualifier), function_name);
            if !matches!(qualified, crate::type_checker::context::FnLookup::Missing) {
                return self.dispatch_module_function_call_with_qualifier(
                    Some(&qualifier),
                    function_name,
                    args,
                );
            }
            if self.context.lookup_fn(None, function_name).is_some() {
                return self.dispatch_module_function_call(function_name, args);
            }
            // Module name was recognised but the function isn't in the
            // (flat) function table — surface a targeted diagnostic
            // rather than falling through to the struct-not-found
            // path which would mention "Struct".
            let module_str = self.resolve_symbol_name(struct_name);
            let func_str = self.resolve_symbol_name(function_name);
            return Err(TypeCheckError::generic_error(&format!(
                "module '{}' has no exported function '{}'",
                module_str, func_str
            )));
        }

        // Verify the struct exists — generic and non-generic both count.
        // From/Into: an enum target is also allowed (`MyErr::from(...)`
        // for `impl From<str> for MyErr`); the enum's methods are
        // registered in the same `struct_methods` registry under the
        // enum's symbol, so the lookup below works unchanged.
        if !self.context.struct_definitions.contains_key(&struct_name)
            && !self.context.enum_definitions.contains_key(&struct_name)
        {
            return Err(TypeCheckError::not_found("Struct", &self.resolve_symbol_name(struct_name)));
        }

        let function_name_str = self.resolve_symbol_name(function_name);

        // CONCRETE-IMPL-Phase-2c: an associated call has no receiver,
        // so the spec is picked from the enclosing type hint
        // (`val v: Vec<u8> = Vec::from_str(...)` selects the
        // `impl Vec<u8>` spec). Without a matching hint the lookup
        // falls back to the lone-spec / generic-impl rules; several
        // concrete specs with no hint is ambiguous and reports the
        // plain "not found" diagnostic below.
        let hint_args: Vec<TypeDecl> = match &self.type_inference.type_hint {
            Some(TypeDecl::Struct(name, a)) if *name == struct_name => a.clone(),
            Some(TypeDecl::Enum(name, a)) if *name == struct_name => a.clone(),
            // Deliberately top-level only. This hint *selects a
            // concrete spec* (`impl Vec<u8>` over `impl<T> Vec<T>`),
            // so reading `u64` out of a nested `Option<Ptr<u64>>` here
            // would ask for an `impl Ptr<u64>` that does not exist and
            // report the generic impl as missing. The nested form is
            // evidence for *inference*, and is read there instead
            // (`handle_generic_associated_function_call`).
            _ => Vec::new(),
        };
        let method = self.context
            .get_struct_method(struct_name, function_name, &hint_args)
            .cloned()
            .ok_or_else(|| TypeCheckError::generic_error(&format!(
                "Associated function '{}' not found for struct '{}'",
                function_name_str, self.resolve_symbol_name(struct_name)
            )))?;

        if self.context.is_generic_struct(struct_name) {
            // Generic struct: delegate to the constraint-based inference path.
            return self.handle_generic_associated_function_call(struct_name, function_name, args, &method);
        }

        // Non-generic struct: type-check arguments directly against the method
        // parameter list (skipping any leading `self` since `Struct::fn(...)` is
        // called without an instance).
        let params: Vec<_> = if method.has_self_param {
            method.parameter.iter().skip(1).cloned().collect()
        } else {
            method.parameter.to_vec()
        };

        if args.len() != params.len() {
            return Err(TypeCheckError::generic_error(&format!(
                "Associated function '{}::{}' expects {} arguments, found {}",
                self.resolve_symbol_name(struct_name),
                function_name_str,
                params.len(),
                args.len()
            )));
        }

        for (arg_expr, (_, expected_ty)) in args.iter().zip(params.iter()) {
            let actual_ty = self.visit_expr(arg_expr)?;
            // NUMBER-HINT: same rule as a free function call.
            let actual_ty = self.coerce_number_expr(arg_expr, &actual_ty, expected_ty)?;
            if !self.is_arg_compatible_dyn_aware(&actual_ty, expected_ty) && !matches!(actual_ty, TypeDecl::Unknown) {
                return Err(TypeCheckError::type_mismatch(
                    expected_ty.clone(),
                    actual_ty,
                ).with_context(&format!(
                    "argument of associated function '{}::{}'",
                    self.resolve_symbol_name(struct_name),
                    function_name_str
                )));
            }
        }

        // Normalize the method's return type so downstream dispatch sees the
        // struct form. `Self` and bare `Identifier(struct_name)` both become
        // `Struct(struct_name, [])`. From/Into: an enum target's `Self`
        // return (`MyErr::from(...) -> Self`) becomes `Enum(MyErr, [])`
        // instead, so the caller's expected type matches.
        let return_ty = method.return_type.clone().unwrap_or(TypeDecl::Unit);
        let is_enum_target = self.context.enum_definitions.contains_key(&struct_name);
        // SELF-IN-TYPE-ARG: `Self` also appears *inside* a type
        // argument (`-> Option<Self>`), where a top-level-only match
        // left it unresolved and the caller compared
        // `Option<Win<u64>>` against a literal `Option<Self>`.
        let self_ty = if is_enum_target {
            TypeDecl::Enum(struct_name, vec![])
        } else {
            TypeDecl::Struct(struct_name, vec![])
        };
        let return_ty = match return_ty {
            TypeDecl::Identifier(name)
                if self.context.struct_definitions.contains_key(&name) =>
            {
                TypeDecl::Struct(name, vec![])
            }
            other => other.substitute_self(&self_ty),
        };
        Ok(return_ty)
    }
}

/// STDLIB-TEXT §3: the `str` methods that were moved to `String`
/// because their answer is a new buffer, paired with how the same
/// question is spelled there. `to_upper` / `to_lower` are listed
/// under their new names: the fold is ASCII-only, and the name is the
/// cheapest place to say so.
fn str_needs_owned_buffer(method: &str) -> Option<&'static str> {
    match method {
        "substring" => Some("substring(start, end)"),
        "trim" => Some("trim()"),
        "split" => Some("split(sep)"),
        "to_upper" | "to_ascii_upper" => Some("to_ascii_upper()"),
        "to_lower" | "to_ascii_lower" => Some("to_ascii_lower()"),
        _ => None,
    }
}
