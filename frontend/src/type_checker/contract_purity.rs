//! COMPILE-TIME-EVAL C4: warn when a contract predicate can do
//! something other than answer a question.
//!
//! A contract is supposed to be a statement about the program, not a
//! part of it: switching the checks off with `INTERPRETER_CONTRACTS`
//! or `--release` must not change what the program does. Nothing was
//! asking for that, and `COMPILE_TIME_EVAL.md` 実測 6 is what it cost
//! — a `requires noisy(n)` whose predicate printed kept printing with
//! the contracts switched off, because the switch is read by the
//! tree-walker while the default engine has the checks lowered into
//! the IR.
//!
//! A pure predicate makes the switch honest, and it is also the
//! precondition for C4's other half: a predicate the compiler can run
//! is a predicate it can check against constant arguments.
//!
//! ## Which effects
//!
//! The ones that *change* something: `Alloc`, `Free`, `RawWrite`, `Io`.
//! Reading is fine, which is why `RawRead` and `AllocCtx` are absent —
//! `ensures __builtin_live_bytes() == old(__builtin_live_bytes())` is
//! the whole point of ALLOC-CONTRACT, and a counter read changes
//! nothing.
//!
//! ## Why this is a warning
//!
//! Because existing programs wrote impure predicates and the compiler
//! never objected. `COMPILE_TIME_EVAL.md` plans one release of warning
//! before this becomes an error, and that is the release.
//!
//! ## Why purity rather than "`const fn` only"
//!
//! The design doc proposed requiring every callee in a predicate to be
//! declared `const fn`. That would mean annotating the stdlib method
//! by method — the same annotation burden `never_allocates` and
//! `const fn` were both deliberately built to avoid — while giving no
//! guarantee the reachability walk does not already give. So the rule
//! is the guarantee itself: nothing reachable from a clause may
//! allocate, free, write through a pointer, or print.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{ExprRef, File, Stmt, StmtRef};
use crate::type_decl::TypeDecl;
use crate::type_checker::effects::{render_path, Effect, EffectSet, EffectTable};
use crate::type_checker::error::TypeCheckError;

/// What makes a predicate more than a question.
const FORBIDDEN: EffectSet =
    EffectSet::of(&[Effect::Alloc, Effect::Free, Effect::RawWrite, Effect::Io]);

pub fn check_contract_purity(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut roots: Vec<(String, ExprRef)> = Vec::new();
    for function in &program.function {
        let name = interner.resolve(function.name).unwrap_or("?");
        collect(&mut roots, name, "requires", &function.requires);
        collect(&mut roots, name, "ensures", &function.ensures);
    }
    for index in 0..program.statement.len() {
        let stmt_ref = StmtRef(index as u32);
        if let Some(Stmt::ImplBlock { target_type, methods, .. }) = program.statement.get(&stmt_ref)
        {
            let owner = interner.resolve(target_type).unwrap_or("?");
            for method in &methods {
                let name = format!("{owner}::{}", interner.resolve(method.name).unwrap_or("?"));
                collect(&mut roots, &name, "requires", &method.requires);
                collect(&mut roots, &name, "ensures", &method.ensures);
            }
        }
    }
    if roots.is_empty() {
        return Vec::new();
    }

    let mut table = EffectTable::new(program, interner, expr_types);
    let mut errors = Vec::new();
    for (name, clause) in &roots {
        let effects = table.of_expr(clause);
        let Some((_, witness)) = effects.first(FORBIDDEN) else {
            continue;
        };
        let path = render_path(name, &witness.path);
        let error =
            TypeCheckError::contract_purity(name.clone(), path, witness.what(), witness.is_opaque());
        // Point at the clause itself: the effect is usually several
        // calls away, and the path in the message is what covers the
        // distance.
        errors.push(match program.location_pool.get_expr_location(clause) {
            Some(location) => error.with_location(*location),
            None => error,
        });
    }
    errors
}

fn collect(roots: &mut Vec<(String, ExprRef)>, owner: &str, kind: &str, clauses: &[ExprRef]) {
    for (index, clause) in clauses.iter().enumerate() {
        roots.push((format!("`{kind}` clause #{} of `{owner}`", index + 1), *clause));
    }
}
