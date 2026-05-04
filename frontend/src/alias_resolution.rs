//! Cross-module type-alias resolution.
//!
//! `core/std/string.t::type String = Vec<u8>` (or any user-side
//! `pub type Foo = Bar`) needs to be visible across module
//! boundaries. The parser already substitutes aliases inside the
//! declaring file's own body, but each auto-loaded module is parsed
//! by its own `Parser` instance with an independent `type_aliases`
//! map — so an alias defined in module A doesn't reach the type
//! position in module B.
//!
//! This pass closes that gap. After every module has been integrated
//! into the main `Program`, we:
//!
//!   1. Walk the program's statement pool, collect every
//!      `Stmt::TypeAlias { name, generic_params, target, .. }`
//!      into a single `HashMap`.
//!   2. Walk every `TypeDecl` position the AST exposes
//!      (function parameters / return types, struct fields, enum
//!      variant payloads, val/var annotations, const types, generic
//!      bounds, impl-target args, trait method signatures, expression
//!      casts) and substitute alias references via
//!      `resolve_in_type`.
//!
//! Substitution itself is recursive and handles alias chains
//! (`type A = u8; type B = A` collapses to `u8`). Generic aliases
//! (`type Pair<T> = Box<T>`) substitute via the existing
//! `TypeDecl::substitute_generics` helper after binding the parsed
//! type args to the alias's parameter symbols.
//!
//! The pass is idempotent: re-running it on a fully-substituted
//! program is a no-op (no `Identifier(name)` refers to an alias).
//! Bare uses of generic aliases (`Pair` without `<...>`) are left
//! as `TypeDecl::Identifier`; the type checker reports the missing
//! arity.

use std::collections::HashMap;
use std::rc::Rc;

use string_interner::DefaultSymbol;

use crate::ast::{Expr, ExprRef, Program, Stmt, StmtRef};
use crate::type_decl::TypeDecl;

type AliasMap = HashMap<DefaultSymbol, (Vec<DefaultSymbol>, TypeDecl)>;

/// Apply cross-module alias substitution to `program` in place.
/// Returns the number of aliases discovered (useful for testing /
/// telemetry; callers can ignore the value).
pub fn resolve_type_aliases(program: &mut Program) -> usize {
    let aliases = collect_aliases(program);
    if aliases.is_empty() {
        return 0;
    }
    rewrite_program(program, &aliases);
    aliases.len()
}

fn collect_aliases(program: &Program) -> AliasMap {
    let mut out: AliasMap = HashMap::new();
    let n = program.statement.len();
    for i in 0..n {
        let stmt_ref = StmtRef(i as u32);
        if let Some(Stmt::TypeAlias { name, generic_params, target, .. }) =
            program.statement.get(&stmt_ref)
        {
            out.insert(name, (generic_params, target));
        }
    }
    out
}

fn rewrite_program(program: &mut Program, aliases: &AliasMap) {
    // Functions (Vec<Rc<Function>>): clone-on-modify via Rc::make_mut.
    for f in program.function.iter_mut() {
        let func = Rc::make_mut(f);
        for (_, ty) in func.parameter.iter_mut() {
            *ty = resolve_in_type(aliases, ty);
        }
        if let Some(ret) = func.return_type.as_mut() {
            *ret = resolve_in_type(aliases, ret);
        }
        for bound in func.generic_bounds.values_mut() {
            *bound = resolve_in_type(aliases, bound);
        }
    }

    // Top-level const declarations.
    for c in program.consts.iter_mut() {
        c.type_decl = resolve_in_type(aliases, &c.type_decl);
    }

    // Statements: walk the pool and update via `StmtPool::update`.
    let n = program.statement.len();
    for i in 0..n {
        let stmt_ref = StmtRef(i as u32);
        let Some(stmt) = program.statement.get(&stmt_ref) else { continue };
        let new_stmt = match stmt {
            Stmt::Val(name, Some(ty), e) => Stmt::Val(name, Some(resolve_in_type(aliases, &ty)), e),
            Stmt::Var(name, Some(ty), e) => Stmt::Var(name, Some(resolve_in_type(aliases, &ty)), e),
            Stmt::StructDecl { name, generic_params, mut generic_bounds, mut fields, visibility } => {
                for bound in generic_bounds.values_mut() {
                    *bound = resolve_in_type(aliases, bound);
                }
                for f in fields.iter_mut() {
                    f.type_decl = resolve_in_type(aliases, &f.type_decl);
                }
                Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility }
            }
            Stmt::EnumDecl { name, generic_params, mut variants, visibility } => {
                for v in variants.iter_mut() {
                    for pt in v.payload_types.iter_mut() {
                        *pt = resolve_in_type(aliases, pt);
                    }
                }
                Stmt::EnumDecl { name, generic_params, variants, visibility }
            }
            Stmt::ImplBlock { target_type, mut target_type_args, methods, trait_name } => {
                for arg in target_type_args.iter_mut() {
                    *arg = resolve_in_type(aliases, arg);
                }
                let mut new_methods = Vec::with_capacity(methods.len());
                for m in &methods {
                    let mut nm = (**m).clone();
                    for (_, ty) in nm.parameter.iter_mut() {
                        *ty = resolve_in_type(aliases, ty);
                    }
                    if let Some(ret) = nm.return_type.as_mut() {
                        *ret = resolve_in_type(aliases, ret);
                    }
                    for bound in nm.generic_bounds.values_mut() {
                        *bound = resolve_in_type(aliases, bound);
                    }
                    new_methods.push(Rc::new(nm));
                }
                Stmt::ImplBlock {
                    target_type,
                    target_type_args,
                    methods: new_methods,
                    trait_name,
                }
            }
            Stmt::TraitDecl { name, mut methods, visibility } => {
                for m in methods.iter_mut() {
                    for (_, ty) in m.parameter.iter_mut() {
                        *ty = resolve_in_type(aliases, ty);
                    }
                    if let Some(ret) = m.return_type.as_mut() {
                        *ret = resolve_in_type(aliases, ret);
                    }
                    for bound in m.generic_bounds.values_mut() {
                        *bound = resolve_in_type(aliases, bound);
                    }
                }
                Stmt::TraitDecl { name, methods, visibility }
            }
            Stmt::TypeAlias { name, generic_params, target, visibility } => {
                let new_target = resolve_in_type(aliases, &target);
                Stmt::TypeAlias { name, generic_params, target: new_target, visibility }
            }
            other => other,
        };
        program.statement.update(&stmt_ref, new_stmt);
    }

    // Expression-level casts (`expr as Type`).
    let m = program.expression.len();
    for i in 0..m {
        let expr_ref = ExprRef(i as u32);
        let Some(expr) = program.expression.get(&expr_ref) else { continue };
        if let Expr::Cast(target, ty) = expr {
            let new_ty = resolve_in_type(aliases, &ty);
            if new_ty != ty {
                program.expression.update(&expr_ref, Expr::Cast(target, new_ty));
            }
        }
    }
}

/// Recursively rewrite every alias reference inside `ty`. Resolves
/// alias chains (one alias's target can itself name another alias)
/// by recursing into the substituted result. Generic aliases are
/// expanded via `TypeDecl::substitute_generics` after binding their
/// parameter symbols to the parsed type args.
///
/// Unknown identifiers and bare uses of generic aliases (no `<...>`
/// at the use site) are left as-is — the type checker reports them
/// as proper diagnostics rather than the resolution pass silently
/// dropping them.
pub fn resolve_in_type(aliases: &AliasMap, ty: &TypeDecl) -> TypeDecl {
    match ty {
        TypeDecl::Identifier(name) => {
            if let Some((params, target)) = aliases.get(name) {
                if params.is_empty() {
                    // Non-generic alias. Recurse to handle alias chains.
                    return resolve_in_type(aliases, target);
                }
                // Generic alias used bare — leave as Identifier; the
                // type checker reports the missing-arity error with
                // proper source-location context.
            }
            ty.clone()
        }
        TypeDecl::Struct(name, args) => {
            let new_args: Vec<TypeDecl> = args.iter().map(|a| resolve_in_type(aliases, a)).collect();
            if let Some((params, target)) = aliases.get(name) {
                if !params.is_empty() && params.len() == new_args.len() {
                    let mut subst: HashMap<DefaultSymbol, TypeDecl> = HashMap::new();
                    for (p, a) in params.iter().zip(new_args.iter()) {
                        subst.insert(*p, a.clone());
                    }
                    let expanded = target.substitute_generics(&subst);
                    return resolve_in_type(aliases, &expanded);
                }
                // Non-generic alias mentioned with type args is an
                // arity mismatch — leave intact for the type checker.
            }
            TypeDecl::Struct(*name, new_args)
        }
        TypeDecl::Enum(name, args) => {
            let new_args: Vec<TypeDecl> = args.iter().map(|a| resolve_in_type(aliases, a)).collect();
            TypeDecl::Enum(*name, new_args)
        }
        TypeDecl::Array(elems, n) => {
            let new_elems: Vec<TypeDecl> = elems.iter().map(|e| resolve_in_type(aliases, e)).collect();
            TypeDecl::Array(new_elems, *n)
        }
        TypeDecl::Tuple(elems) => {
            let new_elems: Vec<TypeDecl> = elems.iter().map(|e| resolve_in_type(aliases, e)).collect();
            TypeDecl::Tuple(new_elems)
        }
        TypeDecl::Dict(k, v) => TypeDecl::Dict(
            Box::new(resolve_in_type(aliases, k)),
            Box::new(resolve_in_type(aliases, v)),
        ),
        TypeDecl::Range(inner) => TypeDecl::Range(Box::new(resolve_in_type(aliases, inner))),
        TypeDecl::Ref { is_mut, inner } => TypeDecl::Ref {
            is_mut: *is_mut,
            inner: Box::new(resolve_in_type(aliases, inner)),
        },
        _ => ty.clone(),
    }
}
