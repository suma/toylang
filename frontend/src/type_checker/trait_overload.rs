//! ERROR_MODEL E1: several impls of one generic trait on one type.
//!
//! `impl From<IoError> for AppError` and `impl From<ParseError> for
//! AppError` are two different conversions, but the method registry
//! keys a spec on `(type, method name, **the impl target's** type
//! args)` — and both blocks target a bare `AppError`, so the second
//! registration replaced the first. The aggregate error type, which
//! is the ordinary way to write a program that can fail in more than
//! one way, did not type-check.
//!
//! The fix is to give each such impl its own method name before
//! anything looks at the AST: `from` becomes `from@IoError`. Since
//! this is an AST mutation, every registry built from the AST — the
//! type checker's, the interpreter's, and `compiler_lower`'s — sees
//! the two methods as unrelated, and the three lanes need no
//! agreement beyond the one they already have (they read the same
//! tree). Call sites are rewritten to the winning name by the type
//! checker, which is the only pass that knows the argument's type.
//!
//! **Only methods that take the trait's type parameters are
//! renamed.** Overload resolution here is by argument type, so a
//! method that mentions the parameter only in its return type
//! (`Iterator<T>::next`) has nothing to resolve on; renaming it would
//! produce a name no call site could ever reach.

use std::collections::HashMap;
use std::rc::Rc;

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Stmt, StmtPool, StmtRef, TraitMethodSignature};
use crate::type_decl::TypeDecl;

/// Separates a method name from the trait type arguments that
/// distinguish its impl. Not a legal identifier character, so a
/// mangled name can never collide with one a user wrote.
pub const OVERLOAD_SEP: char = '@';

/// The name `method` takes in the impl that supplies `trait_type_args`
/// (`from` + `[IoError]` -> `from@IoError`). Both the pre-pass and the
/// call-site lookup spell it through here so they cannot drift.
pub fn overload_name(
    method: &str,
    trait_type_args: &[TypeDecl],
    interner: &DefaultStringInterner,
) -> String {
    let parts: Vec<String> = trait_type_args
        .iter()
        .map(|t| t.spell_with(Some(interner)))
        .collect();
    format!("{method}{OVERLOAD_SEP}{}", parts.join(","))
}

/// The name a method was written with, with any overload suffix
/// removed. Conformance checking compares against the trait's
/// declaration, which knows nothing about the renaming.
pub fn base_method_name(name: &str) -> &str {
    match name.split_once(OVERLOAD_SEP) {
        Some((base, _)) => base,
        None => name,
    }
}

/// Rename the methods of generic-trait impls that would otherwise
/// share a registry slot. Idempotent (an already-suffixed name is
/// left alone), so it is safe to call from every entry point that
/// prepares an AST for checking.
pub fn mangle_overloaded_trait_impls(
    stmt_pool: &mut StmtPool,
    interner: &mut DefaultStringInterner,
) {
    // Pass 1: which trait methods can be overloaded at all — those
    // whose *parameters* mention one of the trait's type parameters.
    let mut overloadable: HashMap<DefaultSymbol, Vec<DefaultSymbol>> = HashMap::new();
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::TraitDecl {
            name,
            generic_params,
            methods,
            ..
        }) = stmt_pool.get(&stmt_ref)
        else {
            continue;
        };
        if generic_params.is_empty() {
            continue;
        }
        let names: Vec<DefaultSymbol> = methods
            .iter()
            .filter(|sig| takes_trait_param(sig, &generic_params))
            .map(|sig| sig.name)
            .collect();
        if !names.is_empty() {
            overloadable.insert(name, names);
        }
    }
    if overloadable.is_empty() {
        return;
    }

    // Pass 2: group the impls that supply those methods by the slot
    // they would land in.
    struct Candidate {
        stmt: StmtRef,
        method_index: usize,
        trait_type_args: Vec<TypeDecl>,
    }
    let mut slots: HashMap<(DefaultSymbol, DefaultSymbol), Vec<Candidate>> = HashMap::new();
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock {
            target_type,
            methods,
            trait_name: Some(trait_sym),
            trait_type_args,
            ..
        }) = stmt_pool.get(&stmt_ref)
        else {
            continue;
        };
        if trait_type_args.is_empty() {
            continue;
        }
        let Some(names) = overloadable.get(&trait_sym) else {
            continue;
        };
        for (method_index, method) in methods.iter().enumerate() {
            if !names.contains(&method.name) {
                continue;
            }
            slots
                .entry((target_type, method.name))
                .or_default()
                .push(Candidate {
                    stmt: stmt_ref,
                    method_index,
                    trait_type_args: trait_type_args.clone(),
                });
        }
    }

    // Pass 3: rename, but only where the slot is actually contested —
    // a lone `impl From<str> for String` keeps the name it was
    // written with, so nothing about the single-impl case changes.
    let mut renames: Vec<(StmtRef, usize, DefaultSymbol)> = Vec::new();
    for ((_, method_sym), candidates) in &slots {
        if candidates.len() < 2 {
            continue;
        }
        let Some(method_str) = interner.resolve(*method_sym).map(str::to_string) else {
            continue;
        };
        let mut spelled: Vec<String> = candidates
            .iter()
            .map(|c| overload_name(&method_str, &c.trait_type_args, interner))
            .collect();
        // Two impls that spell the same trait args are a genuine
        // duplicate impl, not an overload set; renaming both to the
        // same name would only move the collision.
        let mut distinct = spelled.clone();
        distinct.sort();
        distinct.dedup();
        if distinct.len() != spelled.len() {
            continue;
        }
        for (candidate, name) in candidates.iter().zip(spelled.drain(..)) {
            let sym = interner.get_or_intern(name);
            renames.push((candidate.stmt, candidate.method_index, sym));
        }
    }
    if renames.is_empty() {
        return;
    }

    // Pass 4: write the renamed methods back into the pool.
    let mut by_stmt: HashMap<StmtRef, Vec<(usize, DefaultSymbol)>> = HashMap::new();
    for (stmt, method_index, sym) in renames {
        by_stmt.entry(stmt).or_default().push((method_index, sym));
    }
    for (stmt_ref, edits) in by_stmt {
        let Some(Stmt::ImplBlock {
            target_type,
            target_type_args,
            methods,
            trait_name,
            trait_type_args,
        }) = stmt_pool.get(&stmt_ref)
        else {
            continue;
        };
        let mut methods = methods.clone();
        for (method_index, sym) in edits {
            let old = &methods[method_index];
            let mut renamed = (**old).clone();
            renamed.name = sym;
            methods[method_index] = Rc::new(renamed);
        }
        stmt_pool.update(
            &stmt_ref,
            Stmt::ImplBlock {
                target_type,
                target_type_args,
                methods,
                trait_name,
                trait_type_args,
            },
        );
    }
}

/// Whether any of `generic_params` appears in the signature's
/// parameter types — the condition for the method being resolvable by
/// argument type at a call site.
fn takes_trait_param(sig: &TraitMethodSignature, generic_params: &[DefaultSymbol]) -> bool {
    sig.parameter
        .iter()
        .any(|(_, ty)| mentions_any(ty, generic_params))
}

fn mentions_any(ty: &TypeDecl, params: &[DefaultSymbol]) -> bool {
    match ty {
        TypeDecl::Generic(sym) | TypeDecl::Identifier(sym) => params.contains(sym),
        TypeDecl::Struct(sym, args) | TypeDecl::Enum(sym, args) => {
            params.contains(sym) || args.iter().any(|a| mentions_any(a, params))
        }
        TypeDecl::Ref { inner, .. } | TypeDecl::Range(inner) => mentions_any(inner, params),
        TypeDecl::Array(elements, _, _) | TypeDecl::Tuple(elements) => {
            elements.iter().any(|e| mentions_any(e, params))
        }
        TypeDecl::Dict(k, v) => mentions_any(k, params) || mentions_any(v, params),
        TypeDecl::Function(args, ret) => {
            args.iter().any(|a| mentions_any(a, params)) || mentions_any(ret, params)
        }
        _ => false,
    }
}

/// The methods `target` supplies for the overload set named `method`,
/// as `(mangled name, the trait type args it was spelled from)`.
/// Empty when the name was never contested, which is every method in
/// every program that does not write two impls of one generic trait.
pub fn overload_candidates(
    target_methods: &[DefaultSymbol],
    method: &str,
    interner: &DefaultStringInterner,
) -> Vec<DefaultSymbol> {
    let prefix = format!("{method}{OVERLOAD_SEP}");
    target_methods
        .iter()
        .filter(|sym| {
            interner
                .resolve(**sym)
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .copied()
        .collect()
}

/// STDLIB-TRAIT-BASE B0: two impl blocks supplying the same method for
/// the same type under the same target type arguments.
///
/// The registries *replace* on a matching key, so one of the two
/// bodies silently disappeared; the interpreter's own registry builder
/// caught it, but only when the program ran, and only after the type
/// check had said the program was fine. The shape that reaches this is
/// writing a method both inherently and in a trait impl -- which is
/// exactly what someone does when adding `impl Iterator<T> for X` to a
/// type that already has `next`.
///
/// Two impls with *different* concrete target args (`impl Vec<u8>`
/// beside `impl<T> Vec<T>`) are the specialisation the registry is
/// built for, and are left alone.
pub fn find_duplicate_impl_method(
    stmt_pool: &StmtPool,
    interner: &DefaultStringInterner,
) -> Option<String> {
    let mut seen: HashMap<(DefaultSymbol, DefaultSymbol, String), ()> = HashMap::new();
    for index in 0..stmt_pool.len() {
        let stmt_ref = StmtRef(index as u32);
        let Some(Stmt::ImplBlock {
            target_type,
            target_type_args,
            methods,
            ..
        }) = stmt_pool.get(&stmt_ref)
        else {
            continue;
        };
        let args_key = overload_name("", &target_type_args, interner);
        for method in &methods {
            let key = (target_type, method.name, args_key.clone());
            if seen.insert(key, ()).is_some() {
                let type_name = interner.resolve(target_type).unwrap_or("?");
                let method_name = interner.resolve(method.name).unwrap_or("?");
                return Some(format!(
                    "`{type_name}` has two impls of `{method_name}`; one of them would be \
                     silently discarded. Move the method into the trait impl rather than \
                     writing it in both places"
                ));
            }
        }
    }
    None
}
