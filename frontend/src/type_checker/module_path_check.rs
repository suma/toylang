//! MODULE-SYSTEM P3: a call's module path has to be one that exists.
//!
//! `a::b::f(..)` used to lose everything but `f` — the parser dropped
//! the qualifier and the call resolved as a bare name, so
//! `zzz::math::min_i64(3i64, 7i64)` compiled and ran. A path is the
//! one place a reader says *which* `f` they mean; accepting a wrong
//! one in silence is the opposite of what writing it out is for.
//!
//! The parser now records the whole qualifier
//! (`File::call_paths`). **Resolution still uses the nearest
//! segment** — that is P2's rule and it is unchanged — and this pass
//! checks that the rest of what was written is true: the module the
//! call resolves to must have a path ending in the segments the
//! author wrote.
//!
//! What it does *not* do yet is let extra segments pick between two
//! candidates ("write more segments to tell them apart", which P2's
//! ambiguity diagnostic suggests). That needs the qualifier to be
//! multi-segment through three function tables and the two lowering
//! paths; checking is the half that can be had on its own, and it is
//! the half that turns a wrong path from silence into an error.

use std::collections::HashMap;

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ast::{Expr, ExprRef, File};
use crate::type_checker::context::path_ends_with;
use crate::type_checker::error::TypeCheckError;

pub fn check_module_paths(
    program: &File,
    interner: &DefaultStringInterner,
) -> Vec<TypeCheckError> {
    let mut errors = check_call_paths(program, interner);
    errors.extend(check_const_paths(program, interner));
    errors.sort_by_key(|e| e.location.map(|l| (l.line, l.column)));
    errors
}

/// MODULE-CONST-PATH: a `const` named through a module that does not
/// have it.
///
/// The call form has been checked since P3, and a `const` is the
/// other thing a module exports — so `zzz::LIMIT` resolving quietly
/// to `LIMIT` was the same silence in the half nobody had looked at.
/// The written path is already in the tree (the parser keeps every
/// segment of a qualified identifier), so this is only the reading.
///
/// **Checking, not selecting.** The namespace is flat: a bare name
/// picks the first `const` of that name whatever module it came from,
/// and a qualifier cannot change which one. What it can do is be
/// wrong, and now it says so.
fn check_const_paths(
    program: &File,
    interner: &DefaultStringInterner,
) -> Vec<TypeCheckError> {
    if program.consts.is_empty() {
        return Vec::new();
    }
    let mut by_name: HashMap<DefaultSymbol, Vec<Vec<DefaultSymbol>>> = HashMap::new();
    for c in &program.consts {
        by_name
            .entry(c.name)
            .or_default()
            .push(c.module_path.clone().unwrap_or_default());
    }

    let mut errors = Vec::new();
    for i in 0..program.expression.len() {
        let expr_ref = ExprRef(i as u32);
        let Some(Expr::QualifiedIdentifier(path)) = program.expression.get(&expr_ref) else {
            continue;
        };
        if path.len() < 2 {
            continue;
        }
        let name = path[path.len() - 1];
        // An enum variant (`Color::Red`) reaches here too, and it is
        // not a const. Only a name some `const` declares is ours.
        let Some(known) = by_name.get(&name) else {
            continue;
        };
        let written: Vec<DefaultSymbol> = path[..path.len() - 1].to_vec();
        if known.iter().any(|p| path_ends_with(p, &written)) {
            continue;
        }
        // The user's own `const` has no module. Saying "no module is
        // called `foo`" about it is exactly right: there isn't one.
        let spelled_known: Vec<String> = known
            .iter()
            .filter(|p| !p.is_empty())
            .map(|p| spell(interner, p))
            .collect();
        if spelled_known.is_empty() {
            // Declared in the file being compiled. A qualifier on it
            // names a module that does not exist, but the useful
            // thing to say is that it is right here.
            let mut error = TypeCheckError::unknown_module_path(
                spell(interner, &written),
                interner.resolve(name).unwrap_or("?").to_string(),
                Vec::new(),
            );
            if let Some(loc) = program.location_pool.get_expr_location(&expr_ref) {
                error = error.with_location(*loc);
            }
            errors.push(error);
            continue;
        }
        let mut sorted = spelled_known;
        sorted.sort();
        sorted.dedup();
        let mut error = TypeCheckError::unknown_module_path(
            spell(interner, &written),
            interner.resolve(name).unwrap_or("?").to_string(),
            sorted,
        );
        if let Some(loc) = program.location_pool.get_expr_location(&expr_ref) {
            error = error.with_location(*loc);
        }
        errors.push(error);
    }
    errors
}

fn check_call_paths(
    program: &File,
    interner: &DefaultStringInterner,
) -> Vec<TypeCheckError> {
    if program.call_paths.is_empty() {
        return Vec::new();
    }
    // Every module path a function of each name lives under. Built
    // from the AST rather than the type checker's tables so this pass
    // needs nothing but the program.
    let mut by_name: HashMap<DefaultSymbol, Vec<Vec<DefaultSymbol>>> = HashMap::new();
    for (i, f) in program.function.iter().enumerate() {
        let Some(Some(path)) = program.function_module_paths.get(i) else {
            continue;
        };
        by_name.entry(f.name).or_default().push(path.clone());
    }

    let mut calls: Vec<(ExprRef, Vec<DefaultSymbol>)> = program
        .call_paths
        .iter()
        .map(|(e, p)| (*e, p.clone()))
        .collect();
    calls.sort_by_key(|(e, _)| e.0);

    let mut errors = Vec::new();
    for (expr_ref, segments) in calls {
        let Some(Expr::AssociatedFunctionCall(_, fn_name, _)) =
            program.expression.get(&expr_ref)
        else {
            continue;
        };
        let Some(paths) = by_name.get(&fn_name) else {
            // No module defines this name at all. The ordinary
            // "function not found" diagnostic says so, with the
            // position; saying it twice helps nobody.
            continue;
        };
        if paths.iter().any(|p| path_ends_with(p, &segments)) {
            continue;
        }
        // When the *nearest* segment is not a module either, the
        // ordinary resolution has already said "Type or module `up`
        // not found" at this very position. One mistake, one
        // message: this pass speaks when the near end resolves and
        // what was written in front of it does not.
        let nearest = segments[segments.len() - 1];
        if !paths.iter().any(|p| path_ends_with(p, &[nearest])) {
            continue;
        }
        let written = spell(interner, &segments);
        let name = interner.resolve(fn_name).unwrap_or("?").to_string();
        let mut known: Vec<String> = paths.iter().map(|p| spell(interner, p)).collect();
        known.sort();
        known.dedup();
        let mut error = TypeCheckError::unknown_module_path(written, name, known);
        if let Some(loc) = program.location_pool.get_expr_location(&expr_ref) {
            error = error.with_location(*loc);
        }
        errors.push(error);
    }
    errors
}

fn spell(interner: &DefaultStringInterner, path: &[DefaultSymbol]) -> String {
    path.iter()
        .map(|s| interner.resolve(*s).unwrap_or("?"))
        .collect::<Vec<_>>()
        .join("::")
}
