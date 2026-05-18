//! Type checking for `trait` declarations and `impl <Trait> for <Type>`
//! conformance. Lives next to `impl_block.rs` because the two cooperate
//! closely: a trait records the required signatures, and the impl block
//! validates that a concrete struct provides them.
//!
//! Conformance check policy (initial implementation):
//!
//! - Each trait method named `m` must be provided by the impl with the
//!   same arity, parameter types (including `self: Self`), and return type.
//!   `Self` in trait signatures resolves to the impl's target struct.
//! - Extra methods on the impl that the trait doesn't declare are allowed
//!   (they become inherent methods on the struct).
//! - Generics on the trait, default methods, multiple bounds, and trait
//!   inheritance are out of scope.

use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::{MethodFunction, Stmt, StmtPool, StmtRef, TraitMethodSignature, Visibility};
use crate::type_decl::TypeDecl;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

impl<'a> TypeCheckerVisitor<'a> {
    /// Register a `trait` declaration in the context. Methods are stored
    /// verbatim for later conformance checking; we currently do not
    /// validate signatures further (e.g. that types referenced in
    /// parameters exist) — that happens implicitly when the conforming
    /// impl is checked.
    pub fn visit_trait_decl_impl(
        &mut self,
        name: DefaultSymbol,
        methods: &Vec<TraitMethodSignature>,
    ) -> Result<TypeDecl, TypeCheckError> {
        // Backward-compat entry: defer to the generic-aware variant
        // with no parameters. Non-generic traits keep working
        // exactly as before.
        self.visit_trait_decl_with_generic_params(name, &Vec::new(), methods)
    }

    /// ITER-PROTOCOL-TRAIT: generic-aware trait registration. Stores
    /// the trait's generic params alongside its method signatures so
    /// `check_trait_conformance` can substitute them with the impl's
    /// concrete `trait_type_args` before comparing.
    pub fn visit_trait_decl_with_generic_params(
        &mut self,
        name: DefaultSymbol,
        generic_params: &Vec<DefaultSymbol>,
        methods: &Vec<TraitMethodSignature>,
    ) -> Result<TypeDecl, TypeCheckError> {
        if self.context.traits.contains_key(&name) {
            let trait_str = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
            return Err(TypeCheckError::new(format!(
                "trait '{trait_str}' is already defined"
            )));
        }
        // Reject duplicate method names within a single trait.
        let mut seen = std::collections::HashSet::new();
        for m in methods {
            if !seen.insert(m.name) {
                let trait_str = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
                let m_str = self.core.string_interner.resolve(m.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "trait '{trait_str}' has duplicate method '{m_str}'"
                )));
            }
        }
        self.context.traits.insert(name, methods.clone());
        self.context
            .trait_generic_params
            .insert(name, generic_params.clone());
        Ok(TypeDecl::Unit)
    }

    /// Verify that an `impl <Trait> for <Struct>` block provides every
    /// method declared by the trait, with matching signatures. Extra
    /// methods are allowed. Records the conformance in the context.
    /// Trait default bodies (A1) are pre-expanded into the impl's
    /// `methods` slice by the `expand_trait_defaults` pre-pass before
    /// this check runs, so any method declared in the trait must
    /// appear here.
    /// ITER-PROTOCOL-TRAIT-compat shim: forwards with empty
    /// `trait_type_args` so non-generic-trait callers stay unchanged.
    pub fn check_trait_conformance(
        &mut self,
        struct_symbol: DefaultSymbol,
        trait_symbol: DefaultSymbol,
        methods: &[std::rc::Rc<crate::ast::MethodFunction>],
    ) -> Result<(), TypeCheckError> {
        self.check_trait_conformance_with_args(
            struct_symbol,
            trait_symbol,
            &Vec::new(),
            methods,
        )
    }

    /// ITER-PROTOCOL-TRAIT: generic-aware conformance check. Substitutes
    /// the trait's declared generic params with the impl site's
    /// `trait_type_args` (e.g. `impl Iterator<i64> for Counter`
    /// substitutes `T -> i64`) in every trait method signature
    /// before comparing against the impl methods.
    pub fn check_trait_conformance_with_args(
        &mut self,
        struct_symbol: DefaultSymbol,
        trait_symbol: DefaultSymbol,
        trait_type_args: &Vec<TypeDecl>,
        methods: &[std::rc::Rc<crate::ast::MethodFunction>],
    ) -> Result<(), TypeCheckError> {
        let trait_methods = match self.context.traits.get(&trait_symbol).cloned() {
            Some(ms) => ms,
            None => {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "trait '{t_str}' is not defined"
                )));
            }
        };
        // Build the substitution map once for this impl. Empty for
        // non-generic traits; missing generic_params is treated as
        // the empty list (defensive).
        let trait_generic_params = self
            .context
            .trait_generic_params
            .get(&trait_symbol)
            .cloned()
            .unwrap_or_default();
        if !trait_type_args.is_empty()
            && trait_type_args.len() != trait_generic_params.len()
        {
            let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
            return Err(TypeCheckError::new(format!(
                "trait '{t_str}': expected {} type argument(s), found {}",
                trait_generic_params.len(),
                trait_type_args.len(),
            )));
        }
        let mut subst: std::collections::HashMap<DefaultSymbol, TypeDecl> =
            std::collections::HashMap::new();
        for (p, a) in trait_generic_params.iter().zip(trait_type_args.iter()) {
            subst.insert(*p, a.clone());
        }

        for sig in &trait_methods {
            let provided = methods.iter().find(|m| m.name == sig.name);
            let m = match provided {
                Some(m) => m,
                None => {
                    let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                    let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                    let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "impl {t_str} for {s_str}: missing method '{m_str}' required by trait"
                    )));
                }
            };
            if m.has_self_param != sig.has_self_param {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "impl {t_str} for {s_str}: method '{m_str}' self-parameter mismatch"
                )));
            }
            // Stage 1 of `&` references: receiver kind (`&mut self`
            // vs `self` / `&self`) must match exactly between the
            // trait declaration and its impl. The trait writes the
            // contract; an impl that promises less mutation
            // (`&self`) when the trait demands more (`&mut self`),
            // or vice versa, is rejected here so users can't
            // silently subvert the trait's mutability promise.
            if m.has_self_param && m.self_is_mut != sig.self_is_mut {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                let want = if sig.self_is_mut { "&mut self" } else { "self / &self" };
                let got = if m.self_is_mut { "&mut self" } else { "self / &self" };
                return Err(TypeCheckError::new(format!(
                    "impl {t_str} for {s_str}: method '{m_str}' receiver kind mismatch (trait expects {want}, impl uses {got})"
                )));
            }
            if m.parameter.len() != sig.parameter.len() {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "impl {t_str} for {s_str}: method '{m_str}' parameter count mismatch (expected {}, found {})",
                    sig.parameter.len(), m.parameter.len()
                )));
            }
            // Compare parameter types pairwise. Resolve `Self` (in either
            // signature) to the impl's target struct so a trait method
            // declared as `fn m(self: Self) -> Self` matches an impl
            // method spelled the same way (or with the explicit struct).
            for (i, ((_, p_ty), (_, s_ty))) in m.parameter.iter().zip(sig.parameter.iter()).enumerate() {
                let p_resolved = resolve_self(p_ty, struct_symbol);
                // ITER-PROTOCOL-TRAIT: apply trait-arg substitution
                // to the trait-side type before comparing so
                // `Option<T>` with `T -> i64` matches `Option<i64>`.
                let s_resolved =
                    substitute_generics(&resolve_self(s_ty, struct_symbol), &subst);
                if !p_resolved.is_equivalent(&s_resolved) {
                    let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                    let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                    let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "impl {t_str} for {s_str}: method '{m_str}' parameter #{} type mismatch (expected {:?}, found {:?})",
                        i + 1, s_resolved, p_resolved
                    )));
                }
            }
            // Compare return types. Both sides resolve `Self`; the
            // trait side also runs through `substitute_generics`
            // for `T -> trait_type_args[i]`.
            let m_ret = resolve_self(m.return_type.as_ref().unwrap_or(&TypeDecl::Unit), struct_symbol);
            let s_ret = substitute_generics(
                &resolve_self(sig.return_type.as_ref().unwrap_or(&TypeDecl::Unit), struct_symbol),
                &subst,
            );
            if !m_ret.is_equivalent(&s_ret) {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?").to_string();
                let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?").to_string();
                let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "impl {t_str} for {s_str}: method '{m_str}' return type mismatch (expected {:?}, found {:?})",
                    s_ret, m_ret
                )));
            }
        }

        self.context
            .struct_trait_impls
            .entry(struct_symbol)
            .or_default()
            .insert(trait_symbol);
        Ok(())
    }

    /// A1: shim that forwards to the free `expand_trait_defaults_in_pool`
    /// helper. Kept on the visitor for the `visit_program` call site so
    /// the public entry point lives in one place; the interpreter's
    /// `check_typing` calls the free function directly to avoid creating
    /// a `TypeCheckerVisitor` just for the AST mutation.
    pub fn expand_trait_defaults(&mut self) -> Result<(), TypeCheckError> {
        expand_trait_defaults_in_pool(self.core.stmt_pool);
        Ok(())
    }
}

/// A1: walk every `Stmt::TraitDecl` to collect the trait method
/// signatures with default bodies, then walk every
/// `Stmt::ImplBlock { trait_name: Some(_), .. }` and append a
/// synthetic `MethodFunction` for each trait method the impl
/// omitted that has a default body. The mutated impl block is
/// written back through `stmt_pool.update` so the rest of the
/// type checker — and every backend that walks the AST after —
/// sees the synthesized methods as ordinary inherent methods.
/// Idempotent: a second call sees the synthesized methods already
/// in the impl and skips them, so it's safe to invoke from both
/// the interpreter's `check_typing` (before impl-block snapshot)
/// and `visit_program` (compiler_core entry).
/// Generic-trait defaults whose bodies depend on the trait generic
/// params are out of scope for this phase; users should still
/// write those impls explicitly.
pub fn expand_trait_defaults_in_pool(stmt_pool: &mut StmtPool) {
    // Pass 1: index trait default bodies by trait name.
    let mut defaults: std::collections::HashMap<
        DefaultSymbol,
        Vec<TraitMethodSignature>,
    > = std::collections::HashMap::new();
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        if let Some(Stmt::TraitDecl { name, methods, .. }) = stmt_pool.get(&stmt_ref) {
            let with_body: Vec<TraitMethodSignature> = methods
                .iter()
                .filter(|sig| sig.body.is_some())
                .cloned()
                .collect();
            if !with_body.is_empty() {
                defaults.insert(name, with_body);
            }
        }
    }
    if defaults.is_empty() {
        return;
    }

    // Pass 2: for each trait impl, append missing-default methods.
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        let stmt = match stmt_pool.get(&stmt_ref) {
            Some(s) => s,
            None => continue,
        };
        let (target_type, target_type_args, methods, trait_name, trait_type_args) = match stmt {
            Stmt::ImplBlock {
                target_type,
                target_type_args,
                methods,
                trait_name: Some(t),
                trait_type_args,
            } => (target_type, target_type_args, methods, t, trait_type_args),
            _ => continue,
        };
        let trait_defaults = match defaults.get(&trait_name) {
            Some(v) => v,
            None => continue,
        };
        // Append a synthesized MethodFunction for each default the
        // impl omitted. User methods stay first so registration
        // order is preserved (defaults that call user methods via
        // `self.foo()` see them already in scope at backend lookup).
        let mut new_methods = methods.clone();
        for sig in trait_defaults {
            if new_methods.iter().any(|m| m.name == sig.name) {
                continue;
            }
            let body = sig.body.expect("filtered by Pass 1");
            new_methods.push(synthesize_default_method(sig, body));
        }
        if new_methods.len() == methods.len() {
            continue;
        }
        stmt_pool.update(
            &stmt_ref,
            Stmt::ImplBlock {
                target_type,
                target_type_args,
                methods: new_methods,
                trait_name: Some(trait_name),
                trait_type_args,
            },
        );
    }
}

/// A1: build a synthetic `MethodFunction` for a trait method whose
/// default body the impl inherited. The body and signature are
/// borrowed verbatim from the trait declaration; `Self` and `self`
/// resolution happens later when the body is type-checked inside the
/// impl block's `current_impl_target` scope.
fn synthesize_default_method(sig: &TraitMethodSignature, body: StmtRef) -> Rc<MethodFunction> {
    Rc::new(MethodFunction {
        node: sig.node.clone(),
        name: sig.name,
        generic_params: sig.generic_params.clone(),
        generic_bounds: sig.generic_bounds.clone(),
        parameter: sig.parameter.clone(),
        return_type: sig.return_type.clone(),
        requires: sig.requires.clone(),
        ensures: sig.ensures.clone(),
        code: body,
        has_self_param: sig.has_self_param,
        self_is_mut: sig.self_is_mut,
        visibility: Visibility::Public,
    })
}

fn resolve_self(t: &TypeDecl, struct_symbol: DefaultSymbol) -> TypeDecl {
    match t {
        TypeDecl::Self_ => TypeDecl::Struct(struct_symbol, vec![]),
        TypeDecl::Identifier(name) if *name == struct_symbol => TypeDecl::Struct(struct_symbol, vec![]),
        other => other.clone(),
    }
}

/// ITER-PROTOCOL-TRAIT: substitute trait-declared generic params
/// (`Generic(T)` / `Identifier(T)`) with the impl site's concrete
/// type args. Uses the existing `TypeDecl::substitute_generics`
/// for the recursive walk; this thin wrapper exists so the
/// conformance code reads cleanly.
fn substitute_generics(
    t: &TypeDecl,
    subst: &std::collections::HashMap<DefaultSymbol, TypeDecl>,
) -> TypeDecl {
    if subst.is_empty() {
        return t.clone();
    }
    t.substitute_generics(subst)
}
