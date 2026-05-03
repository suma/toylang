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

use string_interner::DefaultSymbol;
use crate::ast::TraitMethodSignature;
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
        Ok(TypeDecl::Unit)
    }

    /// Verify that an `impl <Trait> for <Struct>` block provides every
    /// method declared by the trait, with matching signatures. Extra
    /// methods are allowed. Records the conformance in the context.
    pub fn check_trait_conformance(
        &mut self,
        struct_symbol: DefaultSymbol,
        trait_symbol: DefaultSymbol,
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
                let s_resolved = resolve_self(s_ty, struct_symbol);
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
            // Compare return types. Both sides resolve `Self`.
            let m_ret = resolve_self(m.return_type.as_ref().unwrap_or(&TypeDecl::Unit), struct_symbol);
            let s_ret = resolve_self(sig.return_type.as_ref().unwrap_or(&TypeDecl::Unit), struct_symbol);
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
            .or_insert_with(std::collections::HashSet::new)
            .insert(trait_symbol);
        Ok(())
    }
}

fn resolve_self(t: &TypeDecl, struct_symbol: DefaultSymbol) -> TypeDecl {
    match t {
        TypeDecl::Self_ => TypeDecl::Struct(struct_symbol, vec![]),
        TypeDecl::Identifier(name) if *name == struct_symbol => TypeDecl::Struct(struct_symbol, vec![]),
        other => other.clone(),
    }
}
