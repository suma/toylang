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

        // DBC-LISKOV: reported after the impl is registered, so it is
        // the only thing the program is told about.
        let mut strengthened: Option<TypeCheckError> = None;

        for sig in &trait_methods {
            // ERROR_MODEL E1: an overloaded impl carries a suffixed
            // name (`from@IoError`); the trait declares the plain one.
            let provided = methods.iter().find(|m| {
                m.name == sig.name || self.overload_base_matches(m.name, sig.name)
            });
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
                        "impl {t_str} for {s_str}: method '{m_str}' parameter #{} type mismatch (expected {}, found {})",
                        i + 1,
                        self.type_name_for_error(&s_resolved),
                        self.type_name_for_error(&p_resolved)
                    )));
                }
            }
            // DBC-TRAIT-INHERIT: a contract is an expression written
            // over parameter *names*, so it can only be carried onto
            // an impl that spells them the same way. Renaming used to
            // drop the trait's clauses without a word; refusing it
            // keeps "the trait declares an obligation" true.
            if !sig.requires.is_empty() || !sig.ensures.is_empty() {
                for ((impl_name, _), (trait_name_sym, _)) in
                    m.parameter.iter().zip(sig.parameter.iter())
                {
                    if impl_name != trait_name_sym {
                        let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?");
                        let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?");
                        let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?");
                        let want = self.core.string_interner.resolve(*trait_name_sym).unwrap_or("?");
                        let got = self.core.string_interner.resolve(*impl_name).unwrap_or("?");
                        return Err(TypeCheckError::new(format!(
                            "impl {t_str} for {s_str}: method '{m_str}' renames parameter `{want}` to `{got}`, but {t_str} declares a contract over `{want}` — rename the parameter back so the trait's `requires` / `ensures` still resolve"
                        )));
                    }
                }
            }
            // DBC-LISKOV: an implementation may not demand more than
            // its trait promised. The clauses the trait declares were
            // prepended to `m.requires` by
            // `inherit_trait_contracts`, so anything left over is the
            // impl's own — and a caller holding `&dyn Trait` or a
            // `<T: Trait>` bound has no way to read it. Postconditions
            // are the other way round and stay free to strengthen:
            // promising more than the trait did breaks nobody.
            if strengthened.is_none()
                && let Some(extra) = m.requires.iter().find(|r| !sig.requires.contains(r))
            {
                let t_str = self.core.string_interner.resolve(trait_symbol).unwrap_or("?");
                let s_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("?");
                let m_str = self.core.string_interner.resolve(sig.name).unwrap_or("?");
                // Reported after the impl is recorded below, so the
                // one thing wrong with the program is this clause —
                // an unregistered impl would also fail every `&dyn`
                // coercion, burying it.
                let error = TypeCheckError::impl_precondition(
                    t_str.to_string(),
                    s_str.to_string(),
                    m_str.to_string(),
                    !sig.requires.is_empty(),
                );
                strengthened = Some(match self.get_expr_location(extra) {
                    Some(location) => error.with_location(location),
                    None => error,
                });
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
                    "impl {t_str} for {s_str}: method '{m_str}' return type mismatch (expected {}, found {})",
                    self.type_name_for_error(&s_ret),
                    self.type_name_for_error(&m_ret)
                )));
            }
        }

        self.context
            .struct_trait_impls
            .entry(struct_symbol)
            .or_default()
            .insert(trait_symbol);
        // TRAIT-BOUND: record the concrete type args of this impl so
        // call-site bound checks can distinguish `Iter<i64>` from
        // `Iter<str>`. The args are stored verbatim: a generic impl
        // (`impl<T> Iter<T> for Counter`) registers `[Generic(T)]`,
        // which the bound check treats as a wildcard match.
        self.context
            .trait_impl_type_args
            .entry((struct_symbol, trait_symbol))
            .or_default()
            .push(trait_type_args.clone());
        match strengthened {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
///
/// DBC-TRAIT-INHERIT: the same walk also copies a trait method's
/// `requires` / `ensures` onto the impl's own method. Without it a
/// contract written on the trait did nothing at all unless the impl
/// happened to repeat it — the clause read as an obligation the trait
/// imposed, and was in fact inert. Inherited clauses go *first*, so a
/// violation of the trait's own contract is reported before any the
/// impl adds.
pub fn expand_trait_defaults_in_pool(stmt_pool: &mut StmtPool) {
    inherit_trait_contracts(stmt_pool);
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

/// DBC-TRAIT-INHERIT: give every impl method the contract its trait
/// declared for it.
///
/// A method that carries a default body already inherited its clauses
/// through `synthesize_default_method`; this covers the other case —
/// the impl wrote the method itself — which is the common one, and
/// where the clauses used to vanish.
///
/// Two conditions have to hold, and both are about the clause meaning
/// the same thing in its new home:
///
/// * **Parameter names must match.** A clause is an expression over
///   parameter names, so a trait that says `requires by > 0u64` cannot
///   be attached to an impl that spells the parameter `amount` — the
///   name would resolve to nothing. The receiver is excluded from the
///   comparison since it is not a parameter in the implicit
///   `&self` / `&mut self` form.
/// * **At most one side may use `old(...)`.** The snapshots are
///   referenced positionally (`__old_0`, `__old_1`, ...), so merging
///   two lists would renumber one side's references. Concatenating is
///   only safe when one list is empty.
///
/// Where either fails the impl keeps exactly the contract it had,
/// which is the pre-existing behaviour rather than a regression.
/// Idempotent: re-running finds the clauses already present and
/// leaves them alone.
fn inherit_trait_contracts(stmt_pool: &mut StmtPool) {
    let mut contracts: std::collections::HashMap<
        DefaultSymbol,
        Vec<TraitMethodSignature>,
    > = std::collections::HashMap::new();
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        if let Some(Stmt::TraitDecl { name, methods, .. }) = stmt_pool.get(&stmt_ref) {
            let contracted: Vec<TraitMethodSignature> = methods
                .iter()
                .filter(|sig| !sig.requires.is_empty() || !sig.ensures.is_empty())
                .cloned()
                .collect();
            if !contracted.is_empty() {
                contracts.insert(name, contracted);
            }
        }
    }
    if contracts.is_empty() {
        return;
    }

    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock {
            target_type,
            target_type_args,
            methods,
            trait_name: Some(trait_name),
            trait_type_args,
        }) = stmt_pool.get(&stmt_ref)
        else {
            continue;
        };
        let Some(signatures) = contracts.get(&trait_name) else {
            continue;
        };
        let mut changed = false;
        let new_methods: Vec<Rc<MethodFunction>> = methods
            .iter()
            .map(|method| {
                let Some(sig) = signatures.iter().find(|s| s.name == method.name) else {
                    return method.clone();
                };
                match merge_trait_contract(sig, method) {
                    Some(merged) => {
                        changed = true;
                        Rc::new(merged)
                    }
                    None => method.clone(),
                }
            })
            .collect();
        if !changed {
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

/// The impl method with its trait's clauses prepended, or `None` when
/// nothing needs to change (or the merge would not be sound — see
/// `inherit_trait_contracts`).
fn merge_trait_contract(
    sig: &TraitMethodSignature,
    method: &Rc<MethodFunction>,
) -> Option<MethodFunction> {
    // Already carrying them (a second run, or `synthesize_default_method`
    // copied them verbatim).
    let already_there = sig
        .requires
        .iter()
        .all(|r| method.requires.contains(r))
        && sig.ensures.iter().all(|e| method.ensures.contains(e));
    if already_there {
        return None;
    }
    if !parameter_names_match(sig, method) {
        return None;
    }
    if !sig.old_exprs.is_empty() && !method.old_exprs.is_empty() {
        return None;
    }
    let mut merged = (**method).clone();
    merged.requires = sig
        .requires
        .iter()
        .copied()
        .chain(method.requires.iter().copied())
        .collect();
    merged.ensures = sig
        .ensures
        .iter()
        .copied()
        .chain(method.ensures.iter().copied())
        .collect();
    if method.old_exprs.is_empty() {
        merged.old_exprs = sig.old_exprs.clone();
    }
    Some(merged)
}

/// Whether the two signatures name their parameters identically, so a
/// clause written against one resolves in the other. Conformance
/// already requires the receiver kinds to match, so `self` is either
/// present in both parameter lists (the `self: Self` spelling) or in
/// neither (`&self` / `&mut self`, where the receiver is not a
/// parameter) — comparing the lists as they stand is enough.
fn parameter_names_match(sig: &TraitMethodSignature, method: &MethodFunction) -> bool {
    let trait_params: Vec<DefaultSymbol> = sig.parameter.iter().map(|(n, _)| *n).collect();
    let impl_params: Vec<DefaultSymbol> = method.parameter.iter().map(|(n, _)| *n).collect();
    trait_params == impl_params
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
        // POINTER P6: the default body is the signature's body, so it
        // is checked under the signature's own declaration. An impl
        // that writes an override declares `unsafe` for itself.
        is_unsafe: sig.is_unsafe,
        ensures_kinds: sig.ensures_kinds.clone(),
        never_allocates: sig.never_allocates,
        old_exprs: sig.old_exprs.clone(),
        code: body,
        has_self_param: sig.has_self_param,
        self_is_mut: sig.self_is_mut,
        visibility: Visibility::Public,
        // A1: the body being copied in is the *trait's*, so it
        // resolves in the trait's module, not the impl's.
        module_path: sig.module_path.clone(),
    })
}

fn resolve_self(t: &TypeDecl, struct_symbol: DefaultSymbol) -> TypeDecl {
    match t {
        TypeDecl::Self_ => TypeDecl::Struct(struct_symbol, vec![]),
        TypeDecl::Identifier(name) if *name == struct_symbol => TypeDecl::Struct(struct_symbol, vec![]),
        // `&Self` is the borrowing form of the same obligation, so it
        // has to resolve the same way. Without this arm a trait
        // declaring `fn lt(&self, other: &Self)` could not be
        // implemented at all: the impl spelling `&Self` compared
        // unequal to itself once one side was resolved, and spelling
        // the concrete type was reported as a mismatch too.
        TypeDecl::Ref { is_mut, inner } => TypeDecl::Ref {
            is_mut: *is_mut,
            inner: Box::new(resolve_self(inner, struct_symbol)),
        },
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
