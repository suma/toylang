//! NUM-W-ENUMERATION: every primitive in the canonical list survives
//! the projections that have to carry it.
//!
//! Four enums describe the same primitives — the lexer's `Kind`, the
//! frontend's `TypeDecl`, the IR's `Type`, the interpreter JIT's
//! `ScalarTy` — and each layer used to spell the list out for itself.
//! Six copies existed and they disagreed. The failure is always quiet:
//! an `impl <Trait> for u8` parses, type-checks and lowers, and is then
//! unreachable, reported with a message that mentions neither the width
//! nor the impl.
//!
//! `TypeDecl::PRIMITIVE_IMPL_TARGETS` is now the one list. These tests
//! pin the frontend end of it — that each name is spellable in source
//! and resolves to its primitive. The lowering and interpreter-JIT
//! projections are pinned by unit tests inside those crates, and the
//! four-lane behaviour by `compiler/tests/consistency/primitive_receivers.rs`.

use frontend::ast::{Stmt, StmtRef};
use frontend::type_checker::TypeCheckerVisitor;
use frontend::type_decl::TypeDecl;
use frontend::ParserWithInterner;

/// Declaring a trait and implementing it for `name`, with a method
/// whose signature is written in terms of `Self`.
///
/// `Self` is the load-bearing part. If the impl target does not resolve
/// to a primitive, it stays a `TypeDecl::Identifier(name)` and the body
/// fails to check against the declared return type — historically with
/// the memorable "expected f32, but got f32", one side being the
/// identifier and the other the primitive.
fn impl_for(name: &str) -> String {
    format!(
        "trait Echo {{ fn echo(self: Self) -> Self }}\n\
         impl Echo for {name} {{ fn echo(self: Self) -> Self {{ self }} }}\n\
         fn main() -> u64 {{ 0u64 }}\n"
    )
}

/// Parse and type-check, visiting the declarations first.
///
/// `common::type_check_with_declarations` visits only `StructDecl` and
/// `ImplBlock`, so a `trait` in the source is never registered and the
/// impl reports "trait 'Echo' is not defined". These programs are
/// specifically an impl of a trait on a primitive, so the trait has to
/// be visited too.
fn check(source: &str) -> Result<(), String> {
    let mut parser = ParserWithInterner::new(source);
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("parse error: {e:?}"))?;
    let functions = program.function.clone();
    let stmt_count = program.statement.len();
    let string_interner = parser.get_string_interner();
    let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);

    let mut errors = Vec::new();
    for i in 0..stmt_count {
        let stmt_ref = StmtRef(i as u32);
        let is_decl = tc
            .core
            .stmt_pool
            .get(&stmt_ref)
            .map(|stmt| {
                matches!(
                    stmt,
                    Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::TraitDecl { .. }
                )
            })
            .unwrap_or(false);
        if is_decl && let Err(e) = tc.visit_stmt(&stmt_ref) {
            errors.push(e.message_with(Some(tc.core.string_interner)));
        }
    }
    for f in functions.iter() {
        if let Err(e) = tc.type_check(f.clone()) {
            errors.push(e.message_with(Some(tc.core.string_interner)));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[test]
fn the_canonical_list_is_not_empty_and_has_no_duplicates() {
    // Guards the guard: every test below iterates this list, so an
    // empty or degenerate list would make them all vacuous.
    let entries = TypeDecl::PRIMITIVE_IMPL_TARGETS;
    assert!(entries.len() >= 13, "list shrank unexpectedly: {entries:?}");

    let mut names: Vec<&str> = entries.iter().map(|(_, n)| *n).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate canonical name in the list");

    for (ty, name) in entries {
        assert!(
            entries.iter().filter(|(t, _)| t == ty).count() == 1,
            "`{name}` shares its TypeDecl with another entry"
        );
    }
}

#[test]
fn every_primitive_round_trips_through_its_name() {
    for (ty, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
        assert_eq!(
            ty.primitive_canonical_name(),
            Some(*name),
            "{ty:?} does not report its own name"
        );
        assert_eq!(
            TypeDecl::from_primitive_canonical_name(name).as_ref(),
            Some(ty),
            "`{name}` does not resolve back to {ty:?}"
        );
    }
}

#[test]
fn an_alias_resolves_but_is_not_a_canonical_name() {
    for (alias, ty) in TypeDecl::PRIMITIVE_NAME_ALIASES {
        assert_eq!(
            TypeDecl::from_primitive_canonical_name(alias).as_ref(),
            Some(ty),
            "alias `{alias}` does not resolve"
        );
        // An alias must never be what a type calls itself, or the
        // forward and reverse directions would disagree.
        assert_ne!(
            ty.primitive_canonical_name(),
            Some(*alias),
            "`{alias}` is being reported as a canonical name"
        );
    }
}

#[test]
fn a_name_that_is_not_a_primitive_resolves_to_nothing() {
    for name in ["MyStruct", "Vec", "Self", "usize64", "", "u7"] {
        assert_eq!(
            TypeDecl::from_primitive_canonical_name(name),
            None,
            "`{name}` was mistaken for a primitive"
        );
    }
}

#[test]
fn every_canonical_name_is_an_impl_target_the_parser_and_checker_accept() {
    // The parser's `Kind` -> name table is the one projection that
    // cannot be derived from the list (`Kind` has ~100 variants, so an
    // exhaustive match is no guard). This is that guard: a width added
    // to the canonical list and not to the parser fails here.
    for (_, name) in TypeDecl::PRIMITIVE_IMPL_TARGETS {
        let source = impl_for(name);
        if let Err(errors) = check(&source) {
            panic!("`impl Echo for {name}` did not check:\n{errors}\n--- source ---\n{source}");
        }
    }
}

#[test]
fn an_alias_works_as_an_impl_target_too() {
    for (alias, _) in TypeDecl::PRIMITIVE_NAME_ALIASES {
        let source = impl_for(alias);
        if let Err(errors) = check(&source) {
            panic!("`impl Echo for {alias}` did not check:\n{errors}");
        }
    }
}
