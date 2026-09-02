//! COLLECTIONS C0(a): `==` inside a generic body, checked against the
//! types the body is actually instantiated with.
//!
//! `impl<T> Bag<T> { fn contains(&self, needle: T) -> bool { e == needle } }`
//! type-checks with no `Eq` bound, and dispatches to the element's own
//! `eq` method when there is one — that is deliberate (there is no
//! `derive` in this language, so requiring a bound on every container
//! method would push a hand-written impl onto every user of one).
//!
//! What used to be missing is the other half: a `T` with no `eq` also
//! type-checked, and the program died at run time with
//! `evaluate_eq: Bad types` naming the same type on both sides. This
//! module closes that by recording which type parameters a body
//! compares, and joining that against each call site's type arguments
//! once the whole program has been checked. The join has to be
//! deferred: a call site can be checked before the callee's body
//! (stdlib bodies are integrated after user statements), so neither
//! end can do the work alone.

use string_interner::DefaultSymbol;

use crate::type_checker::context::{EqInstantiation, EqOwner};
use crate::type_checker::error::TypeCheckError;
use crate::type_decl::TypeDecl;
use crate::type_checker::TypeCheckerVisitor;

impl TypeCheckerVisitor<'_> {
    /// Record that the body being checked compares two values of type
    /// parameter `param` with `==` / `!=`.
    pub(crate) fn note_equality_requirement(&mut self, param: DefaultSymbol) {
        let Some(owner) = self.context.current_eq_owner else {
            return;
        };
        self.context
            .eq_required_params
            .entry(owner)
            .or_default()
            .insert(param);
    }

    /// Record a generic call site that already satisfied its declared
    /// bounds. Cheap on the common path: call sites into bodies that
    /// compare nothing are dropped by the post-pass.
    pub(crate) fn note_generic_instantiation(&mut self, inst: EqInstantiation) {
        self.context.eq_instantiations.push(inst);
    }

    /// Whether `==` between two values of this type has an answer.
    ///
    /// Conservative on purpose — everything that is not a struct or an
    /// enum passes, so a shape this function has not thought about
    /// cannot produce a false error. Structs answer through their `eq`
    /// method (the same predicate the operator dispatch uses), and
    /// enums never do: overloading is a struct feature, so an `eq`
    /// written in `impl SomeEnum` would type-check and then fail.
    fn type_supports_equality(&self, ty: &TypeDecl) -> bool {
        // An enum arrives either already canonicalised (`Enum`) or as
        // the bare name the parser could not tell from a struct's.
        if let TypeDecl::Enum(_, _) = ty {
            return false;
        }
        let name = match ty {
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) => *name,
            _ => return true,
        };
        if self.context.enum_definitions.contains_key(&name) {
            return false;
        }
        if !self.context.struct_definitions.contains_key(&name) {
            // Not a type this checker knows as a struct — an alias, or
            // a name that failed to resolve and is already reported.
            return true;
        }
        self.struct_method_compatible(ty, ty, "eq")
    }

    /// Join the recorded bodies against the recorded call sites, once
    /// everything has been checked. Pushes one error per offending
    /// call site.
    pub fn report_missing_equality_impls(&mut self) {
        if self.context.eq_required_params.is_empty() {
            self.context.eq_instantiations.clear();
            return;
        }
        let instantiations = std::mem::take(&mut self.context.eq_instantiations);
        let mut reported: Vec<(EqOwner, DefaultSymbol, String)> = Vec::new();
        for inst in &instantiations {
            let Some(required) = self.context.eq_required_params.get(&inst.owner) else {
                continue;
            };
            let required = required.clone();
            for (param, ty) in &inst.substitutions {
                if !required.contains(param) || self.type_supports_equality(ty) {
                    continue;
                }
                let type_name = self.named_type_for_error(ty);
                // One call site can be checked more than once (a body
                // pulled forward by a forward reference is checked
                // again); the same missing impl must not be reported
                // twice.
                let key = (inst.owner, *param, type_name.clone());
                if reported.contains(&key) {
                    continue;
                }
                reported.push(key);
                let param_name = self.resolve_symbol_name(*param);
                let is_enum = matches!(ty, TypeDecl::Enum(_, _))
                    || matches!(ty,
                        TypeDecl::Struct(n, _) | TypeDecl::Identifier(n)
                            if self.context.enum_definitions.contains_key(n));
                let problem = if is_enum {
                    format!(
                        "`{type_name}` is an enum, and an enum cannot define `eq` \
                         (match on the variants instead)"
                    )
                } else {
                    format!(
                        "`{type_name}` has no `eq` (define \
                         `fn eq(&self, other: &{type_name}) -> bool` in `impl {type_name}`)"
                    )
                };
                let mut error = TypeCheckError::generic_error(&format!(
                    "{} '{}' generic parameter '{}' compares its values with `==`, but {}",
                    inst.owner_kind, inst.owner_name, param_name, problem
                ));
                if let Some(loc) = inst.location {
                    error = error.with_location(loc);
                }
                self.errors.push(error);
            }
        }
    }
}
